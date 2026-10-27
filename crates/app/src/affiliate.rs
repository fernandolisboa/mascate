//! Afiliação (#77): the commissions dashboard of the period (#81), every
//! Commission with its Platform, Program, product, sale date, amount and
//! status, filtered by status, reversals and cancelled sales listed apart,
//! and the form to record a Commission by hand for Platforms without an API
//! or a file. The Commissions Sync (#78) reads each connected Platform at
//! start, every hour and on "Sincronizar agora"; the Amazon earnings report
//! comes in by file (#83). The dashboard groups by Promotion Channel too
//! (#101), named from the Marketing, and the clicks on Short Links, read in
//! the same Sync (#159). Below them, the products promoted (#86), their
//! Affiliate Links per Promotion Channel (#99), the Programs' promotion
//! rules (#98) and the Payouts (#79). The rules live in Affiliate (ADR
//! 0030).

use std::sync::Arc;
use std::time::Duration;

use chrono::Local;
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectState};
use gpui_kit::component::{
    Disableable as _, IndexPath, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Entity, Global, Hsla, SharedString, Subscription, Task, Window, div, px,
};
use mascate_affiliate::{
    Affiliate, AffiliateError, AffiliatePlatform, AffiliateSync, ClicksRead, Commission,
    CommissionFilter, CommissionSource, CommissionStatus, LastSync, ManualCommission, Recorded,
    TagRefusal,
};
use mascate_integrations::{Connection, ConnectionState};
use mascate_kernel::{Currency, Money, PlatformError, RecordId, parse_amount};
use mascate_marketing::PromotionChannel;
use mascate_platform::Flags;

use crate::affiliate_links::{AffiliateLinkCreated, AffiliateLinksSection, ProgramAdded};
use crate::affiliate_panels::{self, AffiliatePanelsSection, PanelRead, Panels};
use crate::amazon_import::{AmazonImportSection, CommissionsImported};
use crate::appearance::{Tokens, look};
use crate::catalog::{NO_DATABASE, day, day_and_time};
use crate::commission_dashboard::{Clicks, CommissionDashboard};
use crate::connections::AppConnections;
use crate::forms::{Outcome, amount_text, day_text, input, notice, parse_day};
use crate::kit;
use crate::layout;
use crate::parts::ScreenParts;
use crate::payouts::{PayoutsChanged, PayoutsSection};
use crate::preferences;
use crate::product_links::{CreateLinkFor, ProductLinksChanged, ProductLinksSection};
use crate::program_joins::{ProgramJoinsSection, ProgramsJoined};
use crate::promotion_channels::{ChannelChanges, promotion_channels};
use crate::promotion_rules::PromotionRulesSection;
use crate::restricted::AppFlags;
use crate::sales::day_start;
use crate::short_io;
use crate::span::{SpanChanged, SpanPicker};

/// The app's Commissions; absent when the database did not open.
pub struct AppAffiliate(pub Arc<Affiliate>);

impl Global for AppAffiliate {}

pub(crate) fn affiliate(cx: &App) -> Option<Arc<Affiliate>> {
    cx.try_global::<AppAffiliate>().map(|app| app.0.clone())
}

/// Counts the changes to the Commissions, so what depends on them (the
/// income Reminder, the Painel) reads them again.
#[derive(Default)]
pub struct CommissionChanges(pub u64);

impl Global for CommissionChanges {}

/// The Platforms the Commissions Sync reads, each behind its Connection.
/// Each Platform's slice adds its adapter here.
pub struct CommissionSources(pub Vec<(Connection, Arc<dyn CommissionSource>)>);

impl Global for CommissionSources {}

fn sources(cx: &App) -> Vec<(Connection, Arc<dyn CommissionSource>)> {
    cx.try_global::<CommissionSources>()
        .map(|sources| sources.0.clone())
        .unwrap_or_default()
}

/// The Commissions Syncs since the app opened; the screen reads the
/// Commissions again after each.
#[derive(Default)]
pub struct CommissionSyncs {
    pub finished: u64,
    pub running: bool,
    /// Why the last Sync could not read a Platform or the clicks, one line
    /// each.
    pub failures: Vec<String>,
}

impl Global for CommissionSyncs {}

/// How often the Commissions Sync runs with the app open or in the tray:
/// Commissions change slowly and Platforms limit requests (ADR 0030).
const SYNC_EVERY: Duration = Duration::from_secs(60 * 60);

/// What one Platform's part of a Sync did.
pub struct SyncOutcome {
    platform: AffiliatePlatform,
    result: Result<AffiliateSync, String>,
}

/// What one Sync did: each connected Platform, then the clicks on Short
/// Links when Short.io is connected.
pub struct SyncRound {
    outcomes: Vec<SyncOutcome>,
    clicks: Option<Result<ClicksRead, AffiliateError>>,
}

impl SyncRound {
    fn read_nothing(&self) -> bool {
        self.outcomes.is_empty() && self.clicks.is_none()
    }

    fn all_went_through(&self) -> bool {
        self.outcomes.iter().all(|outcome| outcome.result.is_ok())
            && self.clicks.as_ref().is_none_or(Result::is_ok)
    }
}

/// Syncs the Commissions now and then every hour, for as long as the app
/// runs.
pub fn start_syncing(cx: &mut App) {
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        loop {
            if let Some(round) = cx.update(sync_now) {
                let round = round.await;
                cx.update(|cx| finished(round, cx));
            }
            executor.timer(SYNC_EVERY).await;
        }
    })
    .detach();
}

/// Runs a Sync of every connected Platform off the UI thread, and then reads
/// the clicks on Short Links; `None` while one is already running or without
/// the database. A Platform without its Connection is skipped, not failed.
pub fn sync_now(cx: &mut App) -> Option<Task<SyncRound>> {
    let affiliate = affiliate(cx)?;
    if cx.default_global::<CommissionSyncs>().running {
        return None;
    }
    let sources = sources(cx);
    let panels = affiliate_panels::panels(cx);
    let flags = cx.try_global::<AppFlags>().map(|app| app.flags.clone());
    let connections = cx.global::<AppConnections>().0.clone();
    let shortener = short_io::adapter(cx)
        .filter(|_| connections.state(Connection::ShortIo) == ConnectionState::Connected);
    cx.default_global::<CommissionSyncs>().running = true;
    Some(cx.background_executor().spawn(async move {
        let mut outcomes = Vec::new();
        for (connection, source) in sources {
            if connections.state(connection) != ConnectionState::Connected {
                continue;
            }
            let platform = source.platform();
            let result = match affiliate.sync(source.as_ref()).await {
                Err(AffiliateError::Platform(PlatformError::NotConnected)) => continue,
                synced => synced.map_err(|error| sync_failure(platform, &error)),
            };
            outcomes.push(SyncOutcome { platform, result });
        }
        if let (Some(panels), Some(flags)) = (panels, flags) {
            outcomes.extend(panel_outcomes(&affiliate, &panels, &flags).await);
        }
        let clicks = match shortener {
            Some(shortener) => match affiliate.read_clicks(shortener.as_ref()).await {
                Err(AffiliateError::Platform(PlatformError::NotConnected)) => None,
                read => Some(read),
            },
            None => None,
        };
        SyncRound { outcomes, clicks }
    }))
}

/// Each panel that is on and can be read, read; one waiting for the owner
/// (no browser, a login) is left for its section to show.
async fn panel_outcomes(affiliate: &Affiliate, panels: &Panels, flags: &Flags) -> Vec<SyncOutcome> {
    let on = match panels.on(flags).await {
        Ok(on) => on,
        Err(error) => {
            return vec![SyncOutcome {
                platform: AffiliatePlatform::MercadoLivre,
                result: Err(format!(
                    "Não consegui ler as funcionalidades restritas para ler os painéis: {error}"
                )),
            }];
        }
    };
    let mut outcomes = Vec::new();
    for (site, panel) in on {
        if !panel.state().can_read() {
            continue;
        }
        let result = match affiliate.sync(panel.as_ref()).await {
            Err(AffiliateError::Platform(PlatformError::NotConnected)) => continue,
            synced => synced.map_err(|error| affiliate_panels::failure_text(site, &error)),
        };
        outcomes.push(SyncOutcome {
            platform: panel.platform(),
            result,
        });
    }
    outcomes
}

/// Tells the screen and the income Reminder that a Sync ran.
pub fn finished(round: SyncRound, cx: &mut App) {
    if let Some(Err(AffiliateError::Platform(PlatformError::Expired))) = &round.clicks {
        short_io::expired(cx);
    }
    let SyncRound { outcomes, clicks } = round;
    let changed = outcomes.iter().any(|outcome| {
        outcome
            .result
            .as_ref()
            .is_ok_and(AffiliateSync::changed_anything)
    });
    let syncs = cx.default_global::<CommissionSyncs>();
    syncs.running = false;
    syncs.finished += 1;
    syncs.failures = outcomes
        .into_iter()
        .filter_map(|outcome| outcome.result.err())
        .chain(
            clicks
                .and_then(Result::err)
                .map(|error| short_io::clicks_failure(&error)),
        )
        .collect();
    if changed {
        cx.update_default_global::<CommissionChanges, _>(|changes, _| changes.0 += 1);
    }
}

/// Why a Platform's part of the Sync failed, as the owner reads it.
fn sync_failure(platform: AffiliatePlatform, error: &AffiliateError) -> String {
    let name = platform.name();
    match error {
        AffiliateError::Platform(PlatformError::Expired) => {
            format!("A {name} não aceitou as chaves: confira cada uma em Configurações › Conexões.")
        }
        AffiliateError::Platform(PlatformError::RateLimited) => format!(
            "A {name} pediu para esperar e continuou pedindo; o próximo Sync tenta de novo."
        ),
        AffiliateError::Platform(PlatformError::Refused(why)) => {
            format!("A {name} recusou a consulta: {why}")
        }
        AffiliateError::Platform(PlatformError::NotFound) => {
            format!("A {name} não encontrou as vendas pedidas.")
        }
        AffiliateError::Platform(PlatformError::Failed(why)) => {
            format!("Não consegui falar com a {name}: {why}")
        }
        error => failure(error),
    }
}

/// "Hotmart: 2 novas e 1 mudou de status", "Eduzz: nada novo, 1 repasse novo",
/// "Cliques do Short.io: nada novo".
fn synced_text(round: &SyncRound) -> String {
    round
        .outcomes
        .iter()
        .filter_map(|outcome| {
            let sync = outcome.result.as_ref().ok()?;
            let payouts = match sync.payouts.added + sync.payouts.changed {
                0 => String::new(),
                1 => ", 1 repasse novo ou mudado".into(),
                count => format!(", {count} repasses novos ou mudados"),
            };
            Some(format!(
                "{}: {}{payouts}",
                outcome.platform.name(),
                recorded_text(&sync.recorded)
            ))
        })
        .chain(
            round
                .clicks
                .as_ref()
                .and_then(|read| read.as_ref().ok())
                .map(short_io::clicks_text),
        )
        .collect::<Vec<_>>()
        .join(" · ")
}

/// "nada novo", "2 novas e 1 mudou de status".
pub(crate) fn recorded_text(recorded: &Recorded) -> String {
    let added = match recorded.added {
        0 => None,
        1 => Some("1 nova".to_owned()),
        count => Some(format!("{count} novas")),
    };
    let changed = match recorded.changed {
        0 => None,
        1 => Some("1 mudou de status".to_owned()),
        count => Some(format!("{count} mudaram de status")),
    };
    match (added, changed) {
        (None, None) => "nada novo".into(),
        (Some(text), None) | (None, Some(text)) => text,
        (Some(added), Some(changed)) => format!("{added} e {changed}"),
    }
}

/// The Reminders Affiliate raises now. A failure raises nothing.
pub(crate) async fn raised_reminders(affiliate: Option<Arc<Affiliate>>) -> Vec<&'static str> {
    let Some(affiliate) = affiliate else {
        return Vec::new();
    };
    affiliate.raised_reminders().await.unwrap_or_else(|error| {
        eprintln!("could not check the Commissions for the income Reminder: {error}");
        Vec::new()
    })
}

/// The statuses the main list shows. Reversals are listed apart, because
/// the money went back to the buyer, and so are cancelled sales, because
/// there never was a Commission.
const LISTED: [CommissionStatus; 3] = [
    CommissionStatus::Pending,
    CommissionStatus::Approved,
    CommissionStatus::Paid,
];

/// One Platform in a form's picker.
#[derive(Clone)]
pub(crate) struct PlatformChoice {
    value: AffiliatePlatform,
    title: SharedString,
}

impl SearchableListItem for PlatformChoice {
    type Value = AffiliatePlatform;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &AffiliatePlatform {
        &self.value
    }
}

pub(crate) type PlatformSelect = Entity<SelectState<SearchableVec<PlatformChoice>>>;

/// Platforms without an API or a file come first: they are why the forms
/// exist.
const FORM_PLATFORMS: [AffiliatePlatform; 9] = [
    AffiliatePlatform::MercadoLivre,
    AffiliatePlatform::Magalu,
    AffiliatePlatform::Amazon,
    AffiliatePlatform::Shopee,
    AffiliatePlatform::Hotmart,
    AffiliatePlatform::Kiwify,
    AffiliatePlatform::Monetizze,
    AffiliatePlatform::Eduzz,
    AffiliatePlatform::Other,
];

/// A picker of the Platforms, the first one picked.
pub(crate) fn platform_select<V: 'static>(
    window: &mut Window,
    cx: &mut Context<V>,
) -> PlatformSelect {
    let choices = SearchableVec::new(
        FORM_PLATFORMS
            .map(|value| PlatformChoice {
                value,
                title: value.name().into(),
            })
            .to_vec(),
    );
    cx.new(|cx| SelectState::new(choices, Some(IndexPath::new(0)), window, cx))
}

/// The Platform a form records for: the Work Context's, fixed, or the one
/// picked in All.
pub(crate) fn form_platform(select: &PlatformSelect, cx: &App) -> Option<AffiliatePlatform> {
    preferences::work_context(cx)
        .affiliate_platform()
        .or_else(|| select.read(cx).selected_value().copied())
}

/// A form's Platform field: the picker in All, the Work Context's Platform,
/// fixed, in one.
pub(crate) fn platform_field(select: &PlatformSelect, cx: &App) -> AnyElement {
    match preferences::work_context(cx).affiliate_platform() {
        Some(platform) => {
            let t = look(cx).tokens;
            div()
                .h(px(24.))
                .flex()
                .items_center()
                .child(kit::tag(platform.name(), t.accent_text, cx))
                .into_any_element()
        }
        None => Select::new(select).small().into_any_element(),
    }
}

/// What one read of the screen brings.
struct Snapshot {
    commissions: Vec<Commission>,
    clicks: Clicks,
    synced: Vec<(AffiliatePlatform, Option<LastSync>)>,
    /// The Promotion Channels' names, from the Marketing.
    channels: Result<Vec<PromotionChannel>, String>,
}

pub struct AffiliateScreen {
    span: Entity<SpanPicker>,
    /// `None` lists every status of [`LISTED`].
    status: Option<CommissionStatus>,
    /// The period's Commissions, of every status.
    commissions: Vec<Commission>,
    dashboard: Entity<CommissionDashboard>,
    /// Each Platform the Sync reads, with its last Sync that went through.
    synced: Vec<(AffiliatePlatform, Option<LastSync>)>,
    /// What the last Sync this screen started brought.
    sync_outcome: Option<Outcome>,
    platform: PlatformSelect,
    program: Entity<InputState>,
    product: Entity<InputState>,
    sold_on: Entity<InputState>,
    amount: Entity<InputState>,
    currency: Currency,
    form_status: CommissionStatus,
    /// The Commission the form is editing, if any.
    editing: Option<RecordId>,
    /// The Commission asking to confirm its deletion.
    deleting: Option<RecordId>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    form: Option<Outcome>,
    outcome: Option<Outcome>,
    import: Entity<AmazonImportSection>,
    payouts: Entity<PayoutsSection>,
    links: Entity<ProductLinksSection>,
    pub(crate) affiliate_links: Entity<AffiliateLinksSection>,
    joins: Entity<ProgramJoinsSection>,
    rules: Entity<PromotionRulesSection>,
    panels: Entity<AffiliatePanelsSection>,
    _subscriptions: Vec<Subscription>,
}

impl AffiliateScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let platform = platform_select(window, cx);
        let sold_on = input("dd/mm/aaaa", window, cx);
        sold_on.update(cx, |field, cx| field.set_value(today(), window, cx));
        let payouts = cx.new(|cx| PayoutsSection::new(window, cx));
        let span = cx.new(|cx| SpanPicker::new(window, cx));
        let import = cx.new(|_| AmazonImportSection::default());
        let links = cx.new(|cx| ProductLinksSection::new(window, cx));
        let affiliate_links = cx.new(|cx| AffiliateLinksSection::new(window, cx));
        let joins = cx.new(|cx| ProgramJoinsSection::new(window, cx));
        let rules = cx.new(|cx| PromotionRulesSection::new(window, cx));
        let panels = cx.new(|_| AffiliatePanelsSection::default());
        let subscriptions = vec![
            // A Sync in the background brings Commissions this screen has not read.
            cx.observe_global_in::<CommissionSyncs>(window, Self::refresh),
            // A channel renamed or deleted in Divulgação names the dashboard's
            // groups.
            cx.observe_global_in::<ChannelChanges>(window, Self::read_commissions),
            // A Payout recorded or deleted changes what is paid, here and on
            // the Painel.
            cx.subscribe_in(
                &payouts,
                window,
                |this, _, _: &PayoutsChanged, window, cx| {
                    cx.update_default_global::<CommissionChanges, _>(|changes, _| changes.0 += 1);
                    this.read_commissions(window, cx)
                },
            ),
            cx.subscribe_in(&span, window, |this, _, _: &SpanChanged, window, cx| {
                this.refresh(window, cx)
            }),
            // A first read brings its Program and its products along.
            cx.subscribe_in(&panels, window, |this, _, _: &PanelRead, window, cx| {
                cx.update_default_global::<CommissionChanges, _>(|changes, _| changes.0 += 1);
                this.refresh(window, cx)
            }),
            cx.subscribe_in(
                &import,
                window,
                |this, _, _: &CommissionsImported, window, cx| {
                    cx.update_default_global::<CommissionChanges, _>(|changes, _| changes.0 += 1);
                    this.refresh(window, cx)
                },
            ),
            // A link moves Commissions between Products on the sheet and the
            // Painel.
            cx.subscribe(&links, |_, _, _: &ProductLinksChanged, cx| {
                cx.update_default_global::<CommissionChanges, _>(|changes, _| changes.0 += 1);
            }),
            // The sales carrying the new link's marking join its channel.
            cx.subscribe_in(
                &affiliate_links,
                window,
                |this, _, _: &AffiliateLinkCreated, window, cx| this.read_commissions(window, cx),
            ),
            cx.subscribe_in(
                &affiliate_links,
                window,
                |this, _, _: &ProgramAdded, window, cx| {
                    this.rules.update(cx, |rules, cx| rules.refresh(window, cx))
                },
            ),
            // A sale joined to a Program added by hand changes its Program
            // everywhere.
            cx.subscribe_in(&joins, window, |this, _, _: &ProgramsJoined, window, cx| {
                cx.update_default_global::<CommissionChanges, _>(|changes, _| changes.0 += 1);
                this.refresh(window, cx)
            }),
            cx.subscribe_in(
                &links,
                window,
                |this, _, wanted: &CreateLinkFor, window, cx| {
                    let product = Some(wanted.product.clone());
                    this.affiliate_links.update(cx, |links, cx| {
                        links.open(wanted.program, product, window, cx)
                    });
                },
            ),
        ];
        let mut screen = Self {
            span,
            status: None,
            commissions: Vec::new(),
            dashboard: cx.new(|_| CommissionDashboard::default()),
            synced: Vec::new(),
            sync_outcome: None,
            platform,
            program: input("Igual à plataforma", window, cx),
            product: input("Fone Bluetooth", window, cx),
            sold_on,
            amount: input("12,40", window, cx),
            currency: Currency::Brl,
            form_status: CommissionStatus::Pending,
            editing: None,
            deleting: None,
            busy: false,
            reads: 0,
            form: None,
            outcome: None,
            import,
            payouts,
            links,
            affiliate_links,
            joins,
            rules,
            panels,
            _subscriptions: subscriptions,
        };
        screen.refresh(window, cx);
        screen
    }

    /// Reads the Commissions and the Payouts again, as when the screen
    /// comes into view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.read_commissions(window, cx);
        let paid = self.span.read(cx).during();
        self.payouts
            .update(cx, |payouts, cx| payouts.refresh(paid, window, cx));
        self.links.update(cx, |links, cx| links.refresh(window, cx));
        self.affiliate_links
            .update(cx, |links, cx| links.refresh(window, cx));
        self.joins.update(cx, |joins, cx| joins.refresh(window, cx));
        self.rules.update(cx, |rules, cx| rules.refresh(window, cx));
        self.panels
            .update(cx, |panels, cx| panels.refresh(window, cx));
    }

    /// Follows a new Work Context: the form stops editing a Commission of a
    /// Platform it does not show, each section closes what it had open on
    /// one without sending it, and everything is read again for the
    /// context.
    pub fn follow_context(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let context = preferences::work_context(cx);
        let editing = self.editing.and_then(|id| {
            self.commissions
                .iter()
                .find(|commission| commission.id == id)
        });
        if editing.is_some_and(|commission| !context.shows_platform(commission.platform)) {
            self.clear_form(window, cx);
            self.form = None;
        }
        self.deleting = None;
        self.payouts.update(cx, PayoutsSection::follow_context);
        self.links.update(cx, ProductLinksSection::follow_context);
        self.affiliate_links
            .update(cx, AffiliateLinksSection::follow_context);
        self.rules.update(cx, PromotionRulesSection::follow_context);
        self.joins.update(cx, ProgramJoinsSection::follow_context);
        self.panels.update(cx, |_, cx| cx.notify());
        if !context.shows_platform(AffiliatePlatform::Amazon) {
            self.import.update(cx, AmazonImportSection::cancel);
        }
        self.refresh(window, cx);
    }

    fn read_commissions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx, |_| async { Ok(None) }, |_, _: (), _, _| {});
    }

    /// Runs `change` off the UI thread, then reads the Commissions of the
    /// period again and hands what the change returned to `then`.
    fn run<T, F, Fut>(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        change: F,
        then: impl FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    ) where
        T: Send + 'static,
        F: FnOnce(Arc<Affiliate>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<Option<T>, AffiliateError>> + Send,
    {
        let Some(affiliate) = affiliate(cx) else {
            return;
        };
        let channels = promotion_channels(cx);
        let sold = self.span.read(cx).during();
        let mut platforms: Vec<AffiliatePlatform> = sources(cx)
            .iter()
            .map(|(_, source)| source.platform())
            .collect();
        let panels = affiliate_panels::panels(cx);
        let flags = cx.try_global::<AppFlags>().map(|app| app.flags.clone());
        self.reads += 1;
        let read = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(affiliate.clone()).await;
            let snapshot = async {
                let commissions = affiliate
                    .commissions(&CommissionFilter {
                        statuses: Vec::new(),
                        sold: sold.clone(),
                    })
                    .await?;
                if let (Some(panels), Some(flags)) = (panels, flags) {
                    // A panel's flag that cannot be read leaves it out of
                    // the line; its section says why.
                    for (_, panel) in panels.on(&flags).await.unwrap_or_default() {
                        platforms.push(panel.platform());
                    }
                }
                let mut synced = Vec::new();
                for platform in platforms {
                    synced.push((platform, affiliate.last_sync(platform).await?));
                }
                let clicks = Clicks {
                    links: affiliate.link_clicks(sold.clone()).await?,
                    affiliate_links: affiliate.links().await?,
                    read_at: affiliate.clicks_read_at().await?,
                };
                let channels = match channels {
                    Some(channels) => channels.channels().await.map_err(|error| error.to_string()),
                    None => Ok(Vec::new()),
                };
                Ok::<_, AffiliateError>(Snapshot {
                    commissions,
                    clicks,
                    synced,
                    channels,
                })
            }
            .await;
            (changed, snapshot)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (changed, snapshot) = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reads == read {
                    this.busy = false;
                    match snapshot {
                        Ok(mut snapshot) => {
                            let context = preferences::work_context(cx);
                            snapshot
                                .commissions
                                .retain(|commission| context.shows_platform(commission.platform));
                            let channels = match snapshot.channels {
                                Ok(channels) => Some(channels),
                                Err(error) => {
                                    this.outcome = Some(Outcome::Failed(
                                        format!(
                                            "Não consegui ler os canais de divulgação: {error}"
                                        )
                                        .into(),
                                    ));
                                    None
                                }
                            };
                            this.dashboard.update(cx, |dashboard, cx| {
                                dashboard.show(
                                    snapshot.commissions.clone(),
                                    channels,
                                    snapshot.clicks.in_context(context),
                                    cx,
                                )
                            });
                            this.commissions = snapshot.commissions;
                            this.synced = snapshot.synced;
                        }
                        Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                    }
                }
                match changed {
                    Ok(Some(value)) => {
                        cx.update_default_global::<CommissionChanges, _>(|changes, _| {
                            changes.0 += 1
                        });
                        then(this, value, window, cx);
                    }
                    Ok(None) => {}
                    Err(error) => this.form = Some(Outcome::Failed(failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(round) = sync_now(cx) else {
            return;
        };
        self.sync_outcome = None;
        cx.spawn(async move |this, cx| {
            let round = round.await;
            let _ = this.update(cx, |this, cx| {
                // Failures show apart, from the last Sync.
                this.sync_outcome = if round.read_nothing() {
                    Some(Outcome::Failed(
                        "Nenhuma plataforma de afiliado está conectada; conecte em \
                         Configurações › Conexões."
                            .into(),
                    ))
                } else if round.all_went_through() {
                    Some(Outcome::Done(synced_text(&round).into()))
                } else {
                    None
                };
                // Reads the Commissions again, through the screen's observer.
                finished(round, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn render_sync(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let running = cx
            .try_global::<CommissionSyncs>()
            .is_some_and(|syncs| syncs.running);
        let context = preferences::work_context(cx);
        let when = self
            .synced
            .iter()
            .filter(|(platform, _)| context.shows_platform(*platform))
            .map(|(platform, last)| match last {
                Some(last) => format!(
                    "{} sincronizada em {}",
                    platform.name(),
                    day_and_time(last.finished_at)
                ),
                None => format!("{} ainda não sincronizada", platform.name()),
            })
            .collect::<Vec<_>>()
            .join(" · ");
        h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .text_xs()
                    .text_color(t.text2)
                    .child(format!("{when} · de novo a cada hora")),
            )
            .child(
                Button::new("sync-commissions")
                    .label("Sincronizar agora")
                    .icon(IconName::RefreshCw)
                    .primary()
                    .small()
                    .loading(running)
                    .disabled(running)
                    .on_click(cx.listener(|this, _, _, cx| this.sync(cx))),
            )
            .into_any_element()
    }

    /// Why a Platform was not read in the last Sync, one notice each.
    fn sync_failures(&self, cx: &App) -> Vec<AnyElement> {
        cx.try_global::<CommissionSyncs>()
            .map(|syncs| {
                syncs
                    .failures
                    .iter()
                    .map(|why| kit::error_notice(why.clone(), cx).into_any_element())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn pick_status(&mut self, status: Option<CommissionStatus>, cx: &mut Context<Self>) {
        self.status = status;
        cx.notify();
    }

    /// The period's Commissions in `statuses`, latest sale first.
    fn of_status(&self, statuses: &[CommissionStatus]) -> Vec<&Commission> {
        self.commissions
            .iter()
            .filter(|commission| statuses.contains(&commission.status))
            .collect()
    }

    /// The Commission the form describes, or why it cannot be one yet.
    fn written(&self, cx: &App) -> Result<ManualCommission, &'static str> {
        let platform = form_platform(&self.platform, cx).ok_or("Escolha a plataforma.")?;
        let product = self.product.read(cx).value().trim().to_owned();
        if product.is_empty() {
            return Err("Digite o produto da venda.");
        }
        let sold_on = parse_day(&self.sold_on.read(cx).value())
            .ok_or("Digite a data da venda como 01/10/2026.")?;
        let amount = parse_amount(&self.amount.read(cx).value())
            .filter(|amount| !amount.is_sign_negative())
            .ok_or("Digite a comissão como 12,40.")?;
        Ok(ManualCommission {
            platform,
            program: self.program.read(cx).value().trim().to_owned(),
            product,
            sold_at: day_start(sold_on),
            amount: Money::new(amount, self.currency),
            status: self.form_status,
        })
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let manual = match self.written(cx) {
            Ok(manual) => manual,
            Err(why) => {
                self.form = Some(Outcome::Failed(why.into()));
                cx.notify();
                return;
            }
        };
        self.form = None;
        self.outcome = None;
        let editing = self.editing;
        self.run(
            window,
            cx,
            move |affiliate| async move {
                match editing {
                    Some(id) => affiliate.edit_manual(id, manual).await.map(Some),
                    None => affiliate.record_manual(manual).await.map(Some),
                }
            },
            move |this, _: Commission, window, cx| {
                this.clear_form(window, cx);
                this.form = Some(Outcome::Done(
                    match editing {
                        Some(_) => "Comissão atualizada.",
                        None => "Comissão lançada.",
                    }
                    .into(),
                ));
            },
        );
    }

    fn clear_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        self.form_status = CommissionStatus::Pending;
        self.sold_on
            .update(cx, |field, cx| field.set_value(today(), window, cx));
        for field in [&self.program, &self.product, &self.amount] {
            field.update(cx, |field, cx| field.set_value("", window, cx));
        }
    }

    /// Loads a manual Commission into the form to change it.
    fn edit(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(commission) = self
            .commissions
            .iter()
            .find(|commission| commission.id == id)
            .cloned()
        else {
            return;
        };
        self.deleting = None;
        self.form = None;
        self.outcome = None;
        self.editing = Some(id);
        self.currency = commission.amount.currency();
        self.form_status = commission.status;
        self.platform.update(cx, |select, cx| {
            select.set_selected_value(&commission.platform, window, cx)
        });
        let program = if commission.program_name == commission.platform.name() {
            String::new()
        } else {
            commission.program_name.clone()
        };
        let sold_on = day_text(commission.sold_at.with_timezone(&Local).date_naive());
        let amount = amount_text(commission.amount);
        for (field, value) in [
            (&self.program, program),
            (&self.product, commission.product.clone()),
            (&self.sold_on, sold_on),
            (&self.amount, amount),
        ] {
            field.update(cx, |field, cx| field.set_value(value, window, cx));
        }
        cx.notify();
    }

    fn delete(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.outcome = None;
        self.run(
            window,
            cx,
            move |affiliate| async move { affiliate.delete_manual(id).await.map(Some) },
            move |this, (), window, cx| {
                this.deleting = None;
                if this.editing == Some(id) {
                    this.clear_form(window, cx);
                }
                this.outcome = Some(Outcome::Done("Comissão excluída.".into()));
            },
        );
    }

    fn render_filters(&self, cx: &mut Context<Self>) -> AnyElement {
        let chip = |id: (&'static str, usize), label: &'static str, on: bool| {
            let button = Button::new(id).label(label).small();
            if on { button.primary() } else { button.ghost() }
        };
        let statuses =
            [None]
                .into_iter()
                .chain(LISTED.map(Some))
                .enumerate()
                .map(|(index, status)| {
                    chip(
                        ("status", index),
                        status_filter_name(status),
                        self.status == status,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_status(status, cx)))
                });
        h_flex().gap_1().children(statuses).into_any_element()
    }

    fn render_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, input: AnyElement| {
            v_flex()
                .gap_1()
                .child(div().text_sm().font_medium().child(label))
                .child(input)
        };
        let currency = |value: Currency| {
            let button = Button::new(("commission-currency", value as usize))
                .label(value.symbol())
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.currency = value;
                    cx.notify();
                }));
            if self.currency == value {
                button.primary()
            } else {
                button.outline()
            }
        };
        let status = |value: CommissionStatus| {
            let button = Button::new(("commission-status", value as usize))
                .label(value.name())
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.form_status = value;
                    cx.notify();
                }));
            if self.form_status == value {
                button.primary()
            } else {
                button.outline()
            }
        };
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if self.editing.is_some() {
                t.accent_edge
            } else {
                t.frame
            })
            .bg(t.surface)
            .child(kit::section_heading(if self.editing.is_some() {
                "Editar comissão"
            } else {
                "Lançar comissão"
            }))
            .child(div().text_sm().text_color(t.text2).child(
                "Para plataformas sem API nem arquivo, como Mercado Livre e Magalu. Só as \
                 comissões lançadas à mão podem ser editadas.",
            ))
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_end()
                    .child(
                        div()
                            .w(px(180.))
                            .child(field("Plataforma", platform_field(&self.platform, cx))),
                    )
                    .child(div().w(px(200.)).child(field(
                        "Programa",
                        Input::new(&self.program).small().into_any_element(),
                    )))
                    .child(div().w(px(260.)).child(field(
                        "Produto",
                        Input::new(&self.product).small().into_any_element(),
                    ))),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_end()
                    .child(div().w(px(130.)).child(field(
                        "Data da venda",
                        Input::new(&self.sold_on).small().into_any_element(),
                    )))
                    .child(div().w(px(130.)).child(field(
                        "Comissão",
                        Input::new(&self.amount).small().into_any_element(),
                    )))
                    .child(field(
                        "Moeda",
                        h_flex()
                            .gap_1()
                            .child(currency(Currency::Brl))
                            .child(currency(Currency::Usd))
                            .into_any_element(),
                    ))
                    .child(field(
                        "Status",
                        h_flex()
                            .gap_1()
                            .children(CommissionStatus::ALL.map(status))
                            .into_any_element(),
                    )),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("save-commission")
                            .label(if self.editing.is_some() {
                                "Salvar alterações"
                            } else {
                                "Lançar comissão"
                            })
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .when(self.editing.is_some(), |row| {
                        row.child(
                            Button::new("stop-editing-commission")
                                .label("Descartar alterações")
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.clear_form(window, cx);
                                    this.form = None;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .children(self.form.as_ref().map(|outcome| notice(outcome, cx)))
            .into_any_element()
    }

    /// The period's Commissions in `status`, listed apart under its own
    /// heading; nothing without one.
    fn render_apart(
        &self,
        status: CommissionStatus,
        why: &'static str,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let apart = self.of_status(&[status]);
        if apart.is_empty() {
            return None;
        }
        let t = look(cx).tokens;
        let heading = format!("{} ({})", status_filter_name(Some(status)), apart.len());
        let rows: Vec<AnyElement> = apart
            .into_iter()
            .map(|commission| self.render_commission(commission, cx))
            .collect();
        Some(
            v_flex()
                .gap_3()
                .child(kit::section_heading(heading))
                .child(div().text_sm().text_color(t.text2).child(why))
                .children(rows)
                .into_any_element(),
        )
    }

    fn render_commission(&self, commission: &Commission, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let id = commission.id;
        let reversed = commission.status == CommissionStatus::Reversed;
        let mut facts = vec![
            format!("venda em {}", day(commission.sold_at)),
            format!(
                "{} desde {}",
                commission.status.name(),
                day(commission.status_since)
            ),
        ];
        if !commission.platform_status.is_empty() {
            facts.push(format!("na plataforma: {}", commission.platform_status));
        }
        if let Some(tracking) = &commission.tracking {
            facts.push(format!("marcação {tracking}"));
        }
        if commission.payout.is_some() {
            facts.push("coberta por um repasse".into());
        }
        // Infoproduct Programs are the product itself: naming both repeats it.
        let program = if commission.program_name == commission.platform.name()
            || commission.program_name == commission.product
        {
            String::new()
        } else {
            format!("{} · ", commission.program_name)
        };
        // One a Payout covers changes only after that Payout is deleted.
        let actions = (commission.manual && commission.payout.is_none()).then(|| {
            let button = |name: &'static str, label: &'static str| {
                Button::new(SharedString::from(format!("{name}-{id}")))
                    .label(label)
                    .ghost()
                    .small()
                    .disabled(self.busy)
            };
            if self.deleting == Some(id) {
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().text_sm().child("Excluir esta comissão?"))
                    .child(
                        button("confirm-delete-commission", "Excluir")
                            .danger()
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.delete(id, window, cx)),
                            ),
                    )
                    .child(button("keep-commission", "Voltar").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.deleting = None;
                            cx.notify();
                        },
                    )))
            } else {
                h_flex()
                    .gap_1()
                    .child(button("edit-commission", "Editar").on_click(
                        cx.listener(move |this, _, window, cx| this.edit(id, window, cx)),
                    ))
                    .child(button("delete-commission", "Excluir").on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.deleting = Some(id);
                            cx.notify();
                        },
                    )))
            }
        });
        h_flex()
            .gap_3()
            .items_center()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if reversed {
                t.danger
            } else if self.editing == Some(id) {
                t.accent_edge
            } else {
                t.frame
            })
            .bg(t.surface)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(kit::tag(commission.platform.name(), t.accent_text, cx))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .font_medium()
                                    .child(format!("{program}{}", commission.product)),
                            )
                            .child(kit::tag(
                                commission.status.name(),
                                status_ink(commission.status, &t),
                                cx,
                            ))
                            .when(commission.manual, |row| {
                                row.child(kit::tag("à mão", t.text2, cx))
                            }),
                    )
                    .child(div().text_xs().text_color(t.text2).child(facts.join(" · "))),
            )
            .children(actions)
            .child(
                div()
                    .font_semibold()
                    .whitespace_nowrap()
                    .when(reversed, |amount| {
                        amount.text_color(t.danger).line_through()
                    })
                    .child(commission.amount.to_pt_br()),
            )
            .into_any_element()
    }
}

fn status_filter_name(status: Option<CommissionStatus>) -> &'static str {
    match status {
        None => "Todas",
        Some(CommissionStatus::Pending) => "Pendentes",
        Some(CommissionStatus::Approved) => "Aprovadas",
        Some(CommissionStatus::Paid) => "Pagas",
        Some(CommissionStatus::Reversed) => "Estornadas",
        Some(CommissionStatus::Cancelled) => "Canceladas",
    }
}

fn status_ink(status: CommissionStatus, t: &Tokens) -> Hsla {
    match status {
        CommissionStatus::Pending => t.text2,
        CommissionStatus::Approved => t.accent_text,
        CommissionStatus::Paid => t.success,
        CommissionStatus::Reversed => t.danger,
        CommissionStatus::Cancelled => t.text2,
    }
}

fn today() -> String {
    Local::now().format("%d/%m/%Y").to_string()
}

/// "1 comissão", "3 comissões".
fn commissions_text(count: usize) -> String {
    match count {
        1 => "1 comissão".into(),
        count => format!("{count} comissões"),
    }
}

/// Why an action on the Commissions failed, as the owner reads it.
fn tag_refusal(refusal: &TagRefusal) -> String {
    match refusal {
        TagRefusal::NotALink => {
            "Cole o link inteiro, começando com https://, como a plataforma mostra.".into()
        }
        TagRefusal::ForeignDomain(platform) => format!(
            "Esse link não é da {}: cole o link de afiliado que ela dá na sua área de \
             afiliado. O app só marca links da própria plataforma.",
            platform.name()
        ),
        TagRefusal::BadTag(AffiliatePlatform::Amazon) => {
            "O tracking id da Amazon termina em -20, como aparece no painel do Associados \
             (por exemplo, mascate-tg-20)."
                .into()
        }
        TagRefusal::BadTag(AffiliatePlatform::Shopee) => {
            "Na Shopee, a marcação vai num subId, que aceita só letras e números (até 20), sem _ \
             nem -. Troque a marcação deste canal em Divulgação, por exemplo canaltelegram."
                .into()
        }
        TagRefusal::BadTag(platform) => {
            format!("A {} não aceita essa marcação.", platform.name())
        }
        TagRefusal::Platform(error) => format!("A plataforma não respondeu: {error}"),
    }
}

pub(crate) fn failure(error: &AffiliateError) -> String {
    match error {
        AffiliateError::NotFound => "Essa comissão mudou; a lista foi atualizada.".into(),
        AffiliateError::NotManual => {
            "Só comissões lançadas à mão podem ser editadas ou excluídas; as outras seguem a \
             plataforma."
                .into()
        }
        AffiliateError::Covered => {
            "Esta comissão está num repasse: exclua o repasse antes de mudá-la.".into()
        }
        AffiliateError::NoSuchPayout => "Esse repasse mudou; a lista foi atualizada.".into(),
        AffiliateError::NotManualPayout => {
            "Só repasses registrados à mão podem ser excluídos; os outros seguem a plataforma."
                .into()
        }
        AffiliateError::NoPayoutAmount => "Digite o valor do repasse, maior que zero.".into(),
        AffiliateError::NotPayable => {
            "Uma das comissões escolhidas já não está aprovada ou entrou em outro repasse; \
             proponha de novo."
                .into()
        }
        AffiliateError::NotCoverable => {
            "As comissões deste repasse já foram conferidas; a lista foi atualizada.".into()
        }
        AffiliateError::NoProduct => "Digite o produto.".into(),
        AffiliateError::NoSuchProgram => "Esse programa mudou; a lista foi atualizada.".into(),
        AffiliateError::NotJoinable => {
            "Essa venda já foi respondida ou mudou; a lista foi atualizada.".into()
        }
        AffiliateError::NoSuchLink => "Esse link mudou; a lista foi atualizada.".into(),
        AffiliateError::BadTag => "A marcação tem letras minúsculas, números, _ e -, até 40 \
                                   caracteres."
            .into(),
        AffiliateError::TagTaken(tag) => format!(
            "Outro canal já usa a marcação {tag} nesta plataforma, e as vendas dela iriam \
             para ele. Use outra marcação neste canal."
        ),
        AffiliateError::WrongTagger => "O app ainda não sabe marcar links desta plataforma.".into(),
        AffiliateError::Tag(refusal) => tag_refusal(refusal),
        AffiliateError::Negative => "A comissão não pode ser negativa.".into(),
        AffiliateError::Restricted(flag) => format!(
            "{} Para usar mesmo assim, ligue \"{}\" em Configurações › Funcionalidades \
             restritas.",
            flag.reason, flag.name
        ),
        AffiliateError::Flag(error) => {
            format!("Não consegui ler as funcionalidades restritas: {error}")
        }
        AffiliateError::Platform(error) => format!("A plataforma não respondeu: {error}"),
        AffiliateError::Unreadable(_) | AffiliateError::Sql(_) => {
            format!("Não consegui ler ou gravar as comissões no banco: {error}")
        }
    }
}

impl Render for AffiliateScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let mut parts = ScreenParts::new("Afiliação");
        if affiliate(cx).is_none() {
            parts
                .notices
                .push(kit::error_notice(NO_DATABASE, cx).into_any_element());
            return layout::screen(parts, cx);
        }
        let context = preferences::work_context(cx);
        if self
            .synced
            .iter()
            .any(|(platform, _)| context.shows_platform(*platform))
        {
            parts.actions.push(self.render_sync(cx));
        }
        parts.actions.push(self.span.clone().into_any_element());
        parts.notices.extend(self.sync_failures(cx));
        parts.notices.extend(
            self.sync_outcome
                .as_ref()
                .map(|outcome| notice(outcome, cx)),
        );
        parts
            .notices
            .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        parts.content.push(self.joins.clone().into_any_element());
        parts
            .content
            .push(self.dashboard.clone().into_any_element());
        let listed = self.of_status(&self.status.map_or(LISTED.to_vec(), |status| vec![status]));
        let count = listed.len();
        let listed: Vec<AnyElement> = listed
            .into_iter()
            .map(|commission| self.render_commission(commission, cx))
            .collect();
        parts.content.push(
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_4()
                        .items_center()
                        .child(kit::section_heading(commissions_text(count)))
                        .child(self.render_filters(cx)),
                )
                .when(listed.is_empty(), |list| {
                    list.child(div().text_sm().text_color(t.text2).child(
                        "Nenhuma comissão neste período. As da Hotmart, da Kiwify, da Eduzz, \
                         da Monetizze e da Shopee chegam sozinhas com a conexão em Configurações › Conexões; as da \
                         Amazon, pelo relatório importado abaixo; lance à mão as de plataformas \
                         sem API nem arquivo.",
                    ))
                })
                .children(listed)
                .into_any_element(),
        );
        parts.content.extend(self.render_apart(
            CommissionStatus::Reversed,
            "O dinheiro voltou ao comprador (reembolso, chargeback, fraude): ficam fora do \
             ganho.",
            cx,
        ));
        parts.content.extend(self.render_apart(
            CommissionStatus::Cancelled,
            "Vendas que não se concretizaram, como um boleto vencido: nunca houve comissão, \
             então não contam como estorno nem entram nos totais.",
            cx,
        ));
        parts.content.push(self.links.clone().into_any_element());
        parts
            .content
            .push(self.affiliate_links.clone().into_any_element());
        parts.content.push(self.rules.clone().into_any_element());
        parts.content.push(self.panels.clone().into_any_element());
        if context.shows_platform(AffiliatePlatform::Amazon) {
            parts.content.push(self.import.clone().into_any_element());
        }
        parts.content.push(self.render_form(cx));
        parts.content.push(self.payouts.clone().into_any_element());
        layout::screen(parts, cx)
    }
}
