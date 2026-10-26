//! Ad Campaigns (#180, ADR 0036): the owner creates, pauses, activates and
//! changes Product Ads campaigns through a port a Platform's adapter
//! implements. Each change is checked and shown first, and goes to the
//! channel once, only after the owner confirms it (as Promotions, ADR 0024).
//! The Target ROAS is suggested from the Break-even ROAS of the campaign's
//! Products, which the caller hands in (ADR 0021), and a daily budget above
//! the Maximum Daily Budget is refused.

use std::sync::Arc;

use libsql::Value;
use mascate_kernel::{BreakEven, Clock, Currency, IdGenerator, Money, PlatformError, Roas};
use mascate_platform::{
    Database, InFlight, Migration, StoredRow, load_single_row, save_single_row,
};
use rust_decimal::{Decimal, RoundingStrategy};

use crate::ads::{
    AdsError, CampaignStatus, ChannelAds, ChannelCampaign, ads_account, read_campaigns,
    write_campaign,
};

pub(crate) const MANAGE_AD_CAMPAIGNS: Migration = Migration {
    version: 13,
    name: "keep each Ad Campaign's budget, strategy and Target ROAS",
    risky: false,
    // New columns stay empty on the campaigns already kept until the next
    // Sync reads them.
    sql: "ALTER TABLE marketing_ad_campaigns ADD COLUMN daily_budget TEXT;
    ALTER TABLE marketing_ad_campaigns ADD COLUMN currency TEXT;
    ALTER TABLE marketing_ad_campaigns ADD COLUMN strategy TEXT;
    ALTER TABLE marketing_ad_campaigns ADD COLUMN roas_target TEXT;
    CREATE TABLE marketing_ads_settings (
        id                   TEXT PRIMARY KEY,
        maximum_daily_budget TEXT NOT NULL,
        currency             TEXT NOT NULL,
        created_at           TEXT NOT NULL,
        updated_at           TEXT NOT NULL,
        deleted_at           TEXT
    );",
};

const SETTINGS_TABLE: &str = "marketing_ads_settings";
const SETTINGS_COLUMNS: &[&str] = &["maximum_daily_budget", "currency"];

/// The Maximum Daily Budget until the owner sets another: R$ 50,00.
pub const DEFAULT_MAXIMUM_DAILY_BUDGET: Money =
    Money::new(Decimal::from_parts(50, 0, 0, false, 0), Currency::Brl);

/// The lowest Target ROAS the channel takes: 1x.
pub const MIN_TARGET_ROAS: Decimal = Decimal::from_parts(1, 0, 0, false, 0);

/// The highest Target ROAS the channel takes: 35x.
pub const MAX_TARGET_ROAS: Decimal = Decimal::from_parts(35, 0, 0, false, 0);

/// Mercado Livre may spend up to twice the daily budget in one day, using
/// what earlier days left.
pub const MOST_DAILY_BUDGETS_IN_A_DAY: Decimal = Decimal::from_parts(2, 0, 0, false, 0);

/// Days of daily budget Mercado Livre bills in a month.
pub const DAILY_BUDGETS_IN_A_MONTH: Decimal = Decimal::from_parts(30, 0, 0, false, 0);

/// How the channel spends a campaign's budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CampaignStrategy {
    /// Fewer ads, to buyers more likely to buy.
    Profitability,
    /// Between profit and reach.
    Increase,
    /// As many people as possible, as for a new listing.
    Visibility,
}

impl CampaignStrategy {
    pub const ALL: [CampaignStrategy; 3] = [
        CampaignStrategy::Profitability,
        CampaignStrategy::Increase,
        CampaignStrategy::Visibility,
    ];

    pub fn code(self) -> &'static str {
        match self {
            CampaignStrategy::Profitability => "profitability",
            CampaignStrategy::Increase => "increase",
            CampaignStrategy::Visibility => "visibility",
        }
    }

    /// Any case: Mercado Livre takes it in lower case and answers in upper.
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|strategy| strategy.code().eq_ignore_ascii_case(code))
    }

    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            CampaignStrategy::Profitability => "Rentabilidade",
            CampaignStrategy::Increase => "Crescimento",
            CampaignStrategy::Visibility => "Visibilidade",
        }
    }
}

/// An Ad Campaign as the owner wants it on the channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignPlan {
    pub name: String,
    /// Active or paused.
    pub status: CampaignStatus,
    /// What it may spend a day, on average.
    pub budget: Money,
    pub strategy: CampaignStrategy,
    pub target: Roas,
}

/// The owner's Ad Campaigns, as a Platform's adapter changes them. A write
/// the channel does not allow the account by its API answers
/// [`PlatformError::NotFound`]. Calls block on the network, so they run off
/// the UI thread.
pub trait ChannelAdCampaigns: ChannelAds {
    fn create_campaign(
        &self,
        account: &str,
        plan: &CampaignPlan,
    ) -> Result<ChannelCampaign, PlatformError>;

    /// Sends the whole plan over the campaign's.
    fn change_campaign(
        &self,
        campaign: &str,
        plan: &CampaignPlan,
    ) -> Result<ChannelCampaign, PlatformError>;

    fn set_campaign_status(
        &self,
        campaign: &str,
        status: CampaignStatus,
    ) -> Result<ChannelCampaign, PlatformError>;
}

/// A Product a campaign advertises, as the caller knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CampaignProduct {
    pub name: String,
    /// `None` when the caller could not work it out, as for a Product
    /// never stocked.
    pub break_even: Option<BreakEven>,
}

/// The Target ROAS the Products ask for: the highest of their Break-even
/// ROAS, rounded up to one decimal and kept within what the channel takes.
/// `None` when no Product has one.
pub fn suggested_target(products: &[CampaignProduct]) -> Option<Roas> {
    let worst = products
        .iter()
        .filter_map(|product| match product.break_even {
            Some(BreakEven::At(roas)) => Some(roas.ratio()),
            _ => None,
        })
        .max()?;
    let rounded = worst
        .round_dp_with_strategy(1, RoundingStrategy::AwayFromZero)
        .clamp(MIN_TARGET_ROAS, MAX_TARGET_ROAS);
    Roas::new(rounded)
}

/// What a daily budget may cost: the day's, the most Mercado Livre spends in
/// one day with what earlier days left, and about a month.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetSpend {
    pub daily: Money,
    pub most_in_a_day: Money,
    pub about_a_month: Money,
}

impl BudgetSpend {
    pub fn of(daily: Money) -> Self {
        Self {
            daily,
            most_in_a_day: daily.times(MOST_DAILY_BUDGETS_IN_A_DAY),
            about_a_month: daily.times(DAILY_BUDGETS_IN_A_MONTH),
        }
    }
}

/// What the owner asks of a campaign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CampaignChange {
    Create,
    Edit,
    Pause,
    Activate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Action {
    Create(CampaignPlan),
    Change {
        campaign: String,
        plan: CampaignPlan,
    },
    Status {
        campaign: String,
        status: CampaignStatus,
    },
}

/// A campaign change checked and ready for the owner to read. Only
/// [`CampaignRequest::confirm`] makes it something [`AdCampaigns::send`]
/// takes; dropping it sends nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "a campaign change does nothing until confirmed and passed to AdCampaigns::send"]
pub struct CampaignRequest {
    action: Action,
    name: String,
    budget: Option<Money>,
    target: Option<Roas>,
    suggested: Option<Roas>,
    losing: Vec<CampaignProduct>,
}

impl CampaignRequest {
    pub fn change(&self) -> CampaignChange {
        match &self.action {
            Action::Create(_) => CampaignChange::Create,
            Action::Change { .. } => CampaignChange::Edit,
            Action::Status {
                status: CampaignStatus::Active,
                ..
            } => CampaignChange::Activate,
            Action::Status { .. } => CampaignChange::Pause,
        }
    }

    /// The campaign's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What goes to the channel when the owner creates or edits it.
    pub fn plan(&self) -> Option<&CampaignPlan> {
        match &self.action {
            Action::Create(plan) | Action::Change { plan, .. } => Some(plan),
            Action::Status { .. } => None,
        }
    }

    /// What the campaign may spend once it runs; `None` for a pause, or a
    /// campaign whose budget was never read.
    pub fn spend(&self) -> Option<BudgetSpend> {
        match self.change() {
            CampaignChange::Pause => None,
            _ => self.budget.map(BudgetSpend::of),
        }
    }

    /// The Target ROAS the campaign runs at once the change goes; `None`
    /// for a campaign whose target was never read.
    pub fn target(&self) -> Option<Roas> {
        self.target
    }

    /// The Target ROAS its Products ask for.
    pub fn suggested(&self) -> Option<Roas> {
        self.suggested
    }

    /// The Products each sale through the ad loses money on: the target is
    /// below their Break-even ROAS, or no ROAS pays for them.
    pub fn losing(&self) -> &[CampaignProduct] {
        &self.losing
    }

    /// The owner read what goes to the channel and confirmed it. A change
    /// that loses money on a Product still needs the owner to accept that.
    pub fn confirm(self) -> Result<ConfirmedCampaign, LossToAccept> {
        if self.losing.is_empty() {
            Ok(ConfirmedCampaign {
                action: self.action,
            })
        } else {
            Err(LossToAccept(Box::new(self)))
        }
    }
}

/// A confirmed campaign change whose Target ROAS loses money on a Product:
/// it goes only once the owner confirms that too.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "a campaign change at a loss goes only once the owner accepts the loss"]
pub struct LossToAccept(Box<CampaignRequest>);

impl LossToAccept {
    pub fn request(&self) -> &CampaignRequest {
        &self.0
    }

    /// The owner read that each sale through the ad loses money, and sends
    /// it anyway.
    pub fn accept_loss(self) -> ConfirmedCampaign {
        ConfirmedCampaign {
            action: self.0.action,
        }
    }
}

/// A campaign change the owner confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedCampaign {
    action: Action,
}

/// The owner's Ad Campaigns and the Maximum Daily Budget that guards them.
pub struct AdCampaigns {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    /// What is on its way to the channel, so a second click never sends it
    /// again.
    sending: InFlight<String>,
}

impl AdCampaigns {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            database,
            clock,
            ids,
            sending: InFlight::default(),
        }
    }

    /// The campaigns as the last Sync or change left them, the active ones
    /// first, then by name.
    pub async fn campaigns(&self) -> Result<Vec<ChannelCampaign>, AdsError> {
        let mut campaigns = read_campaigns(self.database.connection()).await?;
        campaigns.sort_by(|a, b| {
            (a.status, a.name.to_lowercase()).cmp(&(b.status, b.name.to_lowercase()))
        });
        Ok(campaigns)
    }

    /// The most a campaign's daily budget may be: R$ 50,00 until the owner
    /// sets another.
    pub async fn maximum_daily_budget(&self) -> Result<Money, AdsError> {
        let Some(row) =
            load_single_row(self.database.connection(), SETTINGS_TABLE, SETTINGS_COLUMNS).await?
        else {
            return Ok(DEFAULT_MAXIMUM_DAILY_BUDGET);
        };
        Ok(Money::new(row.decimal_at(0)?, row.currency_at(1)?))
    }

    pub async fn save_maximum_daily_budget(&self, maximum: Money) -> Result<(), AdsError> {
        if maximum.amount() <= Decimal::ZERO {
            return Err(AdsError::InvalidMaximum);
        }
        save_single_row(
            self.database.connection(),
            self.clock.as_ref(),
            self.ids.as_ref(),
            SETTINGS_TABLE,
            SETTINGS_COLUMNS,
            vec![
                Value::Text(maximum.amount().to_string()),
                Value::Text(maximum.currency().code().into()),
            ],
        )
        .await?;
        Ok(())
    }

    /// Checks a new campaign: a name no live campaign has, a daily budget
    /// within the Maximum Daily Budget and a Target ROAS the channel takes,
    /// against the Products it is meant for.
    pub async fn prepare_campaign(
        &self,
        plan: CampaignPlan,
        products: &[CampaignProduct],
        unlocked: bool,
    ) -> Result<CampaignRequest, AdsError> {
        let plan = self.checked(plan, None, unlocked).await?;
        Ok(CampaignRequest {
            name: plan.name.clone(),
            budget: Some(plan.budget),
            target: Some(plan.target),
            suggested: suggested_target(products),
            losing: losing(Some(plan.target), products),
            action: Action::Create(plan),
        })
    }

    /// Checks new settings for a campaign, the same way.
    pub async fn prepare_change(
        &self,
        campaign: &str,
        plan: CampaignPlan,
        products: &[CampaignProduct],
        unlocked: bool,
    ) -> Result<CampaignRequest, AdsError> {
        let kept = self.campaign(campaign).await?;
        let plan = self.checked(plan, Some(&kept.id), unlocked).await?;
        Ok(CampaignRequest {
            name: plan.name.clone(),
            budget: Some(plan.budget),
            target: Some(plan.target),
            suggested: suggested_target(products),
            losing: losing(Some(plan.target), products),
            action: Action::Change {
                campaign: kept.id,
                plan,
            },
        })
    }

    /// Checks pausing or activating a campaign. Activating one at a Target
    /// ROAS below its Products' Break-even ROAS loses money as any change
    /// would.
    pub async fn prepare_status(
        &self,
        campaign: &str,
        status: CampaignStatus,
        products: &[CampaignProduct],
        unlocked: bool,
    ) -> Result<CampaignRequest, AdsError> {
        if !unlocked {
            return Err(AdsError::Locked);
        }
        let kept = self.campaign(campaign).await?;
        let losing = match status {
            CampaignStatus::Active => losing(kept.target, products),
            CampaignStatus::Paused => Vec::new(),
            CampaignStatus::Other => return Err(AdsError::NotRunnable),
        };
        Ok(CampaignRequest {
            name: kept.name,
            budget: kept.budget,
            target: kept.target,
            suggested: suggested_target(products),
            losing,
            action: Action::Status {
                campaign: kept.id,
                status,
            },
        })
    }

    /// Sends what the owner confirmed, once, and keeps what the channel
    /// answers. Before creating, the channel is asked whether it already
    /// has a running campaign by that name, from an earlier send whose
    /// answer never arrived: then that one is kept and nothing is created
    /// twice.
    pub async fn send(
        &self,
        channel: &dyn ChannelAdCampaigns,
        confirmed: ConfirmedCampaign,
    ) -> Result<ChannelCampaign, AdsError> {
        let key = match &confirmed.action {
            Action::Create(plan) => format!("create:{}", plan.name.to_lowercase()),
            Action::Change { campaign, .. } | Action::Status { campaign, .. } => {
                format!("campaign:{campaign}")
            }
        };
        let _sending = self.sending.start(key).ok_or(AdsError::Sending)?;
        let sent = match &confirmed.action {
            Action::Create(plan) => {
                let account = ads_account(self.database.connection())
                    .await?
                    .ok_or(AdsError::NoAccount)?;
                let existing = channel.campaigns(&account)?.into_iter().find(|campaign| {
                    runs(campaign.status) && same_name(&campaign.name, &plan.name)
                });
                match existing {
                    Some(campaign) => campaign,
                    None => channel.create_campaign(&account, plan).map_err(written)?,
                }
            }
            Action::Change { campaign, plan } => {
                channel.change_campaign(campaign, plan).map_err(written)?
            }
            Action::Status { campaign, status } => channel
                .set_campaign_status(campaign, *status)
                .map_err(written)?,
        };
        write_campaign(
            self.database.connection(),
            self.ids.as_ref(),
            &sent,
            self.clock.now(),
        )
        .await?;
        Ok(sent)
    }

    async fn campaign(&self, id: &str) -> Result<ChannelCampaign, AdsError> {
        read_campaigns(self.database.connection())
            .await?
            .into_iter()
            .find(|campaign| campaign.id == id)
            .ok_or(AdsError::UnknownCampaign)
    }

    /// The plan with its name trimmed, once every rule holds; `changing` is
    /// the campaign it replaces, whose own name it may keep.
    async fn checked(
        &self,
        mut plan: CampaignPlan,
        changing: Option<&str>,
        unlocked: bool,
    ) -> Result<CampaignPlan, AdsError> {
        if !unlocked {
            return Err(AdsError::Locked);
        }
        plan.name = plan.name.trim().to_owned();
        if plan.name.is_empty() {
            return Err(AdsError::NoName);
        }
        if !runs(plan.status) {
            return Err(AdsError::NotRunnable);
        }
        if read_campaigns(self.database.connection())
            .await?
            .iter()
            .any(|kept| Some(kept.id.as_str()) != changing && same_name(&kept.name, &plan.name))
        {
            return Err(AdsError::NameTaken);
        }
        let maximum = self.maximum_daily_budget().await?;
        // Mercado Livre takes the budget in cents: one that rounds to
        // nothing is no budget.
        if plan.budget.currency() != maximum.currency()
            || plan.budget.rounded().amount() <= Decimal::ZERO
        {
            return Err(AdsError::InvalidBudget);
        }
        if plan.budget.amount() > maximum.amount() {
            return Err(AdsError::AboveMaximumBudget(maximum));
        }
        if !(MIN_TARGET_ROAS..=MAX_TARGET_ROAS).contains(&plan.target.ratio()) {
            return Err(AdsError::TargetOutOfRange);
        }
        Ok(plan)
    }
}

/// The Products a campaign at `target` loses money on; with no target
/// known, those no ROAS pays for.
fn losing(target: Option<Roas>, products: &[CampaignProduct]) -> Vec<CampaignProduct> {
    products
        .iter()
        .filter(|product| match (product.break_even, target) {
            (Some(BreakEven::Never), _) => true,
            (Some(BreakEven::At(least)), Some(target)) => target < least,
            _ => false,
        })
        .cloned()
        .collect()
}

/// Active or paused: what the owner may set a campaign to.
fn runs(status: CampaignStatus) -> bool {
    matches!(status, CampaignStatus::Active | CampaignStatus::Paused)
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// A write the channel does not allow is the account without campaign
/// management by API.
fn written(error: PlatformError) -> AdsError {
    match error {
        PlatformError::NotFound => AdsError::NotManaged,
        error => AdsError::Platform(error),
    }
}
