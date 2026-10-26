//! Ad Campaigns through marketing's public interface, with a real temporary
//! database and an in-memory channel: every change is checked first and
//! goes once, only after the owner confirms it; a lost answer to a create
//! never makes two campaigns; the Target ROAS is suggested from the
//! Products' Break-even ROAS, and one below it needs a second confirmation;
//! a daily budget above the Maximum Daily Budget is refused.

use std::str::FromStr;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use chrono::{NaiveDate, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{BreakEven, Currency, Margin, Money, PlatformError, Roas};
use mascate_marketing::{
    AdCampaigns, AdsError, BudgetSpend, CampaignChange, CampaignPlan, CampaignProduct,
    CampaignStatus, CampaignStrategy, ChannelAd, ChannelAdCampaigns, ChannelAds, ChannelCampaign,
    DEFAULT_MAXIMUM_DAILY_BUDGET, MIGRATIONS, ProductAds, suggested_target,
};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

fn roas(ratio: &str) -> Roas {
    Roas::new(Decimal::from_str(ratio).unwrap()).unwrap()
}

/// A Product whose ads break even at `ratio`.
fn breaking_even_at(name: &str, ratio: &str) -> CampaignProduct {
    CampaignProduct {
        name: name.into(),
        break_even: Some(BreakEven::At(roas(ratio))),
    }
}

/// A Product that loses money before any ad.
fn losing_before_ads(name: &str) -> CampaignProduct {
    CampaignProduct {
        name: name.into(),
        break_even: Some(BreakEven::of(Margin::of(brl("50"), &[brl("55")]).unwrap())),
    }
}

fn plan(name: &str, budget: &str, target: &str) -> CampaignPlan {
    CampaignPlan {
        name: name.into(),
        status: CampaignStatus::Active,
        budget: brl(budget),
        strategy: CampaignStrategy::Profitability,
        target: roas(target),
    }
}

fn fones() -> ChannelCampaign {
    ChannelCampaign {
        id: "351".into(),
        name: "Fones".into(),
        status: CampaignStatus::Active,
        budget: Some(brl("30")),
        strategy: Some(CampaignStrategy::Profitability),
        target: Some(roas("5")),
    }
}

/// What the channel was asked to write.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Write {
    Create(CampaignPlan),
    Change(String, CampaignPlan),
    Status(String, CampaignStatus),
}

/// A channel answering from memory.
struct Channel {
    campaigns: Mutex<Vec<ChannelCampaign>>,
    writes: Mutex<Vec<Write>>,
    /// The next create is made on the channel but its answer never
    /// arrives.
    loses_answer: Mutex<bool>,
    /// Every write answers this instead.
    refuses: Mutex<Option<PlatformError>>,
    /// Holds a create until the test lets it go, saying when it started.
    holds: Mutex<Option<(Sender<()>, Receiver<()>)>>,
}

impl Channel {
    fn new() -> Self {
        Self {
            campaigns: Mutex::new(vec![fones()]),
            writes: Mutex::default(),
            loses_answer: Mutex::default(),
            refuses: Mutex::default(),
            holds: Mutex::default(),
        }
    }

    fn writes(&self) -> Vec<Write> {
        self.writes.lock().unwrap().clone()
    }

    fn refused(&self) -> Result<(), PlatformError> {
        match self.refuses.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// The campaign as the channel keeps it after `plan`.
    fn keep(&self, id: &str, plan: &CampaignPlan) -> ChannelCampaign {
        let kept = ChannelCampaign {
            id: id.into(),
            name: plan.name.clone(),
            status: plan.status,
            budget: Some(plan.budget.rounded()),
            strategy: Some(plan.strategy),
            target: Some(plan.target),
        };
        let mut campaigns = self.campaigns.lock().unwrap();
        campaigns.retain(|campaign| campaign.id != id);
        campaigns.push(kept.clone());
        kept
    }
}

impl ChannelAds for Channel {
    fn account(&self) -> Result<Option<String>, PlatformError> {
        Ok(Some("8823".into()))
    }

    fn campaigns(&self, account: &str) -> Result<Vec<ChannelCampaign>, PlatformError> {
        assert_eq!(account, "8823");
        Ok(self.campaigns.lock().unwrap().clone())
    }

    fn ads_on(&self, _: &str, _: NaiveDate) -> Result<Vec<ChannelAd>, PlatformError> {
        Ok(Vec::new())
    }
}

impl ChannelAdCampaigns for Channel {
    fn create_campaign(
        &self,
        account: &str,
        plan: &CampaignPlan,
    ) -> Result<ChannelCampaign, PlatformError> {
        assert_eq!(account, "8823");
        if let Some((started, release)) = self.holds.lock().unwrap().take() {
            started.send(()).unwrap();
            release.recv().unwrap();
        }
        self.writes
            .lock()
            .unwrap()
            .push(Write::Create(plan.clone()));
        self.refused()?;
        let created = self.keep("360", plan);
        if std::mem::take(&mut *self.loses_answer.lock().unwrap()) {
            return Err(PlatformError::Failed("connection reset".into()));
        }
        Ok(created)
    }

    fn change_campaign(
        &self,
        campaign: &str,
        plan: &CampaignPlan,
    ) -> Result<ChannelCampaign, PlatformError> {
        self.writes
            .lock()
            .unwrap()
            .push(Write::Change(campaign.into(), plan.clone()));
        self.refused()?;
        Ok(self.keep(campaign, plan))
    }

    fn set_campaign_status(
        &self,
        campaign: &str,
        status: CampaignStatus,
    ) -> Result<ChannelCampaign, PlatformError> {
        self.writes
            .lock()
            .unwrap()
            .push(Write::Status(campaign.into(), status));
        self.refused()?;
        let mut campaigns = self.campaigns.lock().unwrap();
        let kept = campaigns
            .iter_mut()
            .find(|kept| kept.id == campaign)
            .ok_or(PlatformError::NotFound)?;
        kept.status = status;
        Ok(kept.clone())
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    ads: ProductAds,
    campaigns: Arc<AdCampaigns>,
    channel: Arc<Channel>,
}

impl Fixture {
    /// The owner's Product Ads read once, with the campaign "Fones".
    async fn synced() -> Self {
        let fx = Self::unsynced().await;
        fx.ads.sync(fx.channel.as_ref()).await.unwrap();
        fx
    }

    async fn unsynced() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 26, 12, 0, 0).unwrap(),
        ));
        let ids = Arc::new(SequentialIds::default());
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        Self {
            ads: ProductAds::new(database.clone(), clock.clone(), ids.clone()),
            campaigns: Arc::new(AdCampaigns::new(database, clock.clone(), ids)),
            _dir: dir,
            clock,
            channel: Arc::new(Channel::new()),
        }
    }

    async fn create(&self, plan: CampaignPlan) -> Result<ChannelCampaign, AdsError> {
        let request = self.campaigns.prepare_campaign(plan, &[], true).await?;
        self.campaigns
            .send(self.channel.as_ref(), request.confirm().unwrap())
            .await
    }
}

#[test]
fn a_new_campaign_goes_once_after_confirmation_and_is_kept_as_the_channel_answers() {
    block_on(async {
        let fx = Fixture::synced().await;
        let products = [breaking_even_at("Fone Bluetooth", "4.32")];

        let request = fx
            .campaigns
            .prepare_campaign(plan("  Cabos  ", "20", "4.4"), &products, true)
            .await
            .unwrap();

        assert!(fx.channel.writes().is_empty());
        assert_eq!(request.change(), CampaignChange::Create);
        assert_eq!(request.name(), "Cabos");
        assert_eq!(request.suggested(), Some(roas("4.4")));
        assert!(request.losing().is_empty());
        assert_eq!(
            request.spend(),
            Some(BudgetSpend {
                daily: brl("20"),
                most_in_a_day: brl("40"),
                about_a_month: brl("600"),
            })
        );

        let created = fx
            .campaigns
            .send(fx.channel.as_ref(), request.confirm().unwrap())
            .await
            .unwrap();

        assert_eq!(
            fx.channel.writes(),
            [Write::Create(plan("Cabos", "20", "4.4"))]
        );
        assert_eq!(created.id, "360");
        assert_eq!(
            fx.campaigns.campaigns().await.unwrap(),
            [
                ChannelCampaign {
                    id: "360".into(),
                    name: "Cabos".into(),
                    status: CampaignStatus::Active,
                    budget: Some(brl("20")),
                    strategy: Some(CampaignStrategy::Profitability),
                    target: Some(roas("4.4")),
                },
                fones(),
            ]
        );
    });
}

#[test]
fn a_create_whose_answer_was_lost_finds_the_campaign_by_name_and_makes_no_second() {
    block_on(async {
        let fx = Fixture::synced().await;
        *fx.channel.loses_answer.lock().unwrap() = true;
        let request = fx
            .campaigns
            .prepare_campaign(plan("Cabos", "20", "4"), &[], true)
            .await
            .unwrap();

        let lost = fx
            .campaigns
            .send(fx.channel.as_ref(), request.clone().confirm().unwrap())
            .await;
        assert!(matches!(
            lost,
            Err(AdsError::Platform(PlatformError::Failed(_)))
        ));

        let again = fx
            .campaigns
            .send(fx.channel.as_ref(), request.confirm().unwrap())
            .await
            .unwrap();

        assert_eq!(again.id, "360");
        assert_eq!(fx.channel.writes().len(), 1);
        assert_eq!(
            fx.channel.campaigns.lock().unwrap().len(),
            2,
            "Fones and one Cabos"
        );
        assert!(
            fx.campaigns
                .campaigns()
                .await
                .unwrap()
                .iter()
                .any(|campaign| campaign.id == "360")
        );
    });
}

#[test]
fn a_second_click_while_a_change_is_on_its_way_sends_nothing_more() {
    let fx = block_on(Fixture::synced());
    let (started, wait_started) = channel();
    let (release, wait_release) = channel();
    *fx.channel.holds.lock().unwrap() = Some((started, wait_release));
    let request = block_on(
        fx.campaigns
            .prepare_campaign(plan("Cabos", "20", "4"), &[], true),
    )
    .unwrap();
    let (campaigns, on_channel, first) = (
        fx.campaigns.clone(),
        fx.channel.clone(),
        request.clone().confirm().unwrap(),
    );
    let sending = std::thread::spawn(move || block_on(campaigns.send(on_channel.as_ref(), first)));
    if wait_started
        .recv_timeout(std::time::Duration::from_secs(5))
        .is_err()
    {
        panic!("never reached the channel: {:?}", sending.join());
    }

    let second = block_on(
        fx.campaigns
            .send(fx.channel.as_ref(), request.confirm().unwrap()),
    );

    assert!(matches!(second, Err(AdsError::Sending)));
    release.send(()).unwrap();
    assert!(sending.join().unwrap().is_ok());
    assert_eq!(fx.channel.writes().len(), 1);
}

#[test]
fn pausing_activating_and_editing_send_one_request_each() {
    block_on(async {
        let fx = Fixture::synced().await;
        let products = [breaking_even_at("Fone Bluetooth", "4.32")];

        let pause = fx
            .campaigns
            .prepare_status("351", CampaignStatus::Paused, &products, true)
            .await
            .unwrap();
        assert_eq!(pause.change(), CampaignChange::Pause);
        assert_eq!(pause.name(), "Fones");
        assert_eq!(pause.spend(), None);
        fx.campaigns
            .send(fx.channel.as_ref(), pause.confirm().unwrap())
            .await
            .unwrap();
        assert_eq!(
            fx.campaigns.campaigns().await.unwrap()[0].status,
            CampaignStatus::Paused
        );

        let activate = fx
            .campaigns
            .prepare_status("351", CampaignStatus::Active, &products, true)
            .await
            .unwrap();
        assert_eq!(activate.change(), CampaignChange::Activate);
        assert_eq!(activate.spend().map(|spend| spend.daily), Some(brl("30")));
        fx.campaigns
            .send(fx.channel.as_ref(), activate.confirm().unwrap())
            .await
            .unwrap();

        let edit = fx
            .campaigns
            .prepare_change("351", plan("Fones", "35.50", "4.4"), &products, true)
            .await
            .unwrap();
        assert_eq!(edit.change(), CampaignChange::Edit);
        fx.campaigns
            .send(fx.channel.as_ref(), edit.confirm().unwrap())
            .await
            .unwrap();

        assert_eq!(
            fx.channel.writes(),
            [
                Write::Status("351".into(), CampaignStatus::Paused),
                Write::Status("351".into(), CampaignStatus::Active),
                Write::Change("351".into(), plan("Fones", "35.50", "4.4")),
            ]
        );
        let kept = &fx.campaigns.campaigns().await.unwrap()[0];
        assert_eq!(kept.budget, Some(brl("35.50")));
        assert_eq!(kept.target, Some(roas("4.4")));
    });
}

#[test]
fn the_suggested_target_is_the_worst_break_even_rounded_up_and_kept_from_1x_to_35x() {
    let suggested = |products: &[CampaignProduct]| suggested_target(products);

    assert_eq!(
        suggested(&[
            breaking_even_at("Cabo", "2.5"),
            breaking_even_at("Fone", "4.32"),
            losing_before_ads("Capa"),
            CampaignProduct {
                name: "Sem custo".into(),
                break_even: None,
            },
        ]),
        Some(roas("4.4"))
    );
    assert_eq!(
        suggested(&[breaking_even_at("Fone", "4.3")]),
        Some(roas("4.3"))
    );
    assert_eq!(
        suggested(&[breaking_even_at("Fone", "0.8")]),
        Some(roas("1"))
    );
    assert_eq!(
        suggested(&[breaking_even_at("Fone", "40")]),
        Some(roas("35"))
    );
    assert_eq!(suggested(&[losing_before_ads("Capa")]), None);
    assert_eq!(suggested(&[]), None);
}

proptest! {
    #[test]
    fn a_suggested_target_pays_for_every_product_it_can_with_one_decimal(
        hundredths in proptest::collection::vec(1u32..5_000, 1..6),
    ) {
        let products: Vec<CampaignProduct> = hundredths
            .iter()
            .map(|cents| CampaignProduct {
                name: cents.to_string(),
                break_even: Some(BreakEven::At(Roas::new(Decimal::new(i64::from(*cents), 2)).unwrap())),
            })
            .collect();
        let worst = Decimal::new(i64::from(*hundredths.iter().max().unwrap()), 2);

        let target = suggested_target(&products).unwrap().ratio();

        prop_assert!(target >= Decimal::ONE && target <= Decimal::from(35));
        prop_assert_eq!(target.round_dp(1), target);
        if worst <= Decimal::from(35) {
            prop_assert!(target >= worst);
            prop_assert!(target - worst < Decimal::new(1, 1) || worst < Decimal::ONE);
        } else {
            prop_assert_eq!(target, Decimal::from(35));
        }
    }
}

#[test]
fn a_target_below_a_products_break_even_needs_the_owner_to_accept_the_loss() {
    block_on(async {
        let fx = Fixture::synced().await;
        let fone = breaking_even_at("Fone Bluetooth", "4.32");
        let products = [fone.clone(), breaking_even_at("Cabo USB", "2.5")];

        let request = fx
            .campaigns
            .prepare_campaign(plan("Cabos", "20", "3"), &products, true)
            .await
            .unwrap();
        assert_eq!(request.losing(), [fone]);

        let loss = request.confirm().unwrap_err();
        assert!(fx.channel.writes().is_empty());
        fx.campaigns
            .send(fx.channel.as_ref(), loss.accept_loss())
            .await
            .unwrap();
        assert_eq!(fx.channel.writes().len(), 1);
    });
}

#[test]
fn a_product_no_roas_pays_for_needs_the_loss_accepted_at_any_target() {
    block_on(async {
        let fx = Fixture::synced().await;
        let capa = losing_before_ads("Capa");
        let products = [capa.clone(), breaking_even_at("Fone Bluetooth", "4.32")];

        let request = fx
            .campaigns
            .prepare_campaign(plan("Capas", "10", "35"), &products, true)
            .await
            .unwrap();
        assert_eq!(request.losing(), std::slice::from_ref(&capa));
        assert!(request.confirm().is_err());

        let activating = fx
            .campaigns
            .prepare_status("351", CampaignStatus::Active, &products, true)
            .await
            .unwrap();
        assert_eq!(
            activating.losing(),
            [capa],
            "Fones runs at 5x, above the Fone's 4.32x"
        );
        let pausing = fx
            .campaigns
            .prepare_status("351", CampaignStatus::Paused, &products, true)
            .await
            .unwrap();
        assert!(pausing.confirm().is_ok(), "a pause spends nothing");
    });
}

#[test]
fn a_target_at_the_suggestion_needs_no_second_confirmation() {
    block_on(async {
        let fx = Fixture::synced().await;
        let products = [breaking_even_at("Fone Bluetooth", "4.32")];
        let suggested = suggested_target(&products).unwrap();

        let request = fx
            .campaigns
            .prepare_campaign(
                plan("Cabos", "20", &suggested.ratio().to_string()),
                &products,
                true,
            )
            .await
            .unwrap();

        assert!(request.confirm().is_ok());
    });
}

#[test]
fn a_budget_above_the_maximum_daily_budget_is_refused_until_the_maximum_goes_up() {
    block_on(async {
        let fx = Fixture::synced().await;
        assert_eq!(
            fx.campaigns.maximum_daily_budget().await.unwrap(),
            DEFAULT_MAXIMUM_DAILY_BUDGET
        );
        assert_eq!(DEFAULT_MAXIMUM_DAILY_BUDGET, brl("50"));

        let refused = fx.create(plan("Cabos", "50.01", "4")).await;
        assert!(
            matches!(refused, Err(AdsError::AboveMaximumBudget(maximum)) if maximum == brl("50"))
        );
        assert!(fx.channel.writes().is_empty());

        fx.campaigns
            .save_maximum_daily_budget(brl("80"))
            .await
            .unwrap();
        assert_eq!(
            fx.campaigns.maximum_daily_budget().await.unwrap(),
            brl("80")
        );
        fx.create(plan("Cabos", "50.01", "4")).await.unwrap();

        assert!(matches!(
            fx.campaigns.save_maximum_daily_budget(brl("0")).await,
            Err(AdsError::InvalidMaximum)
        ));
    });
}

#[test]
fn a_budget_stays_exact_until_the_channel_and_one_of_no_cents_is_no_budget() {
    block_on(async {
        let fx = Fixture::synced().await;
        for nothing in ["0", "0.004", "-5"] {
            assert!(
                matches!(
                    fx.create(plan("Cabos", nothing, "4")).await,
                    Err(AdsError::InvalidBudget)
                ),
                "{nothing}"
            );
        }
        let dollars = CampaignPlan {
            budget: Money::new(Decimal::from(10), Currency::Usd),
            ..plan("Cabos", "10", "4")
        };
        assert!(matches!(
            fx.create(dollars).await,
            Err(AdsError::InvalidBudget)
        ));

        fx.create(plan("Cabos", "49.995", "4")).await.unwrap();

        assert_eq!(
            fx.channel.writes(),
            [Write::Create(plan("Cabos", "49.995", "4"))],
            "the plan goes exact; the adapter rounds it for the channel"
        );
    });
}

#[test]
fn a_target_the_channel_does_not_take_is_refused() {
    block_on(async {
        let fx = Fixture::synced().await;
        for outside in ["0.9", "35.1"] {
            assert!(
                matches!(
                    fx.create(plan("Cabos", "10", outside)).await,
                    Err(AdsError::TargetOutOfRange)
                ),
                "{outside}"
            );
        }
        fx.create(plan("Cabos", "10", "35")).await.unwrap();
    });
}

#[test]
fn a_new_campaign_needs_a_name_no_live_campaign_has() {
    block_on(async {
        let fx = Fixture::synced().await;

        assert!(matches!(
            fx.create(plan("  ", "10", "4")).await,
            Err(AdsError::NoName)
        ));
        assert!(matches!(
            fx.create(plan(" fones ", "10", "4")).await,
            Err(AdsError::NameTaken)
        ));
        let paused = CampaignPlan {
            status: CampaignStatus::Other,
            ..plan("Cabos", "10", "4")
        };
        assert!(matches!(
            fx.create(paused).await,
            Err(AdsError::NotRunnable)
        ));

        let renamed = fx
            .campaigns
            .prepare_change("351", plan("Fones", "10", "4"), &[], true)
            .await
            .expect("a campaign keeps its own name");
        assert_eq!(renamed.name(), "Fones");
        assert!(matches!(
            fx.campaigns
                .prepare_change("999", plan("Cabos", "10", "4"), &[], true)
                .await,
            Err(AdsError::UnknownCampaign)
        ));
    });
}

#[test]
fn nothing_is_prepared_while_the_reputation_does_not_unlock_product_ads() {
    block_on(async {
        let fx = Fixture::synced().await;

        assert!(matches!(
            fx.campaigns
                .prepare_campaign(plan("Cabos", "10", "4"), &[], false)
                .await,
            Err(AdsError::Locked)
        ));
        assert!(matches!(
            fx.campaigns
                .prepare_status("351", CampaignStatus::Paused, &[], false)
                .await,
            Err(AdsError::Locked)
        ));
    });
}

#[test]
fn a_write_the_channel_does_not_allow_says_so_and_the_read_goes_on() {
    block_on(async {
        let fx = Fixture::synced().await;
        *fx.channel.refuses.lock().unwrap() = Some(PlatformError::NotFound);

        assert!(matches!(
            fx.create(plan("Cabos", "10", "4")).await,
            Err(AdsError::NotManaged)
        ));
        let pause = fx
            .campaigns
            .prepare_status("351", CampaignStatus::Paused, &[], true)
            .await
            .unwrap();
        assert!(matches!(
            fx.campaigns
                .send(fx.channel.as_ref(), pause.confirm().unwrap())
                .await,
            Err(AdsError::NotManaged)
        ));

        *fx.channel.refuses.lock().unwrap() = Some(PlatformError::RateLimited);
        assert!(matches!(
            fx.create(plan("Cabos", "10", "4")).await,
            Err(AdsError::Platform(PlatformError::RateLimited))
        ));

        assert!(fx.ads.sync(fx.channel.as_ref()).await.unwrap().account);
        assert_eq!(fx.campaigns.campaigns().await.unwrap(), [fones()]);
    });
}

#[test]
fn a_campaign_needs_the_account_a_sync_read_first() {
    block_on(async {
        let fx = Fixture::unsynced().await;

        assert!(matches!(
            fx.create(plan("Cabos", "10", "4")).await,
            Err(AdsError::NoAccount)
        ));
        assert!(fx.channel.writes().is_empty());
    });
}

#[test]
fn a_sync_keeps_each_campaigns_budget_strategy_and_target_as_the_channel_has_them() {
    block_on(async {
        let fx = Fixture::synced().await;
        assert_eq!(fx.campaigns.campaigns().await.unwrap(), [fones()]);

        fx.channel.keep("351", &plan("Fones", "12", "6"));
        fx.clock.advance(TimeDelta::days(1));
        fx.ads.sync(fx.channel.as_ref()).await.unwrap();

        let kept = &fx.campaigns.campaigns().await.unwrap()[0];
        assert_eq!(kept.budget, Some(brl("12")));
        assert_eq!(kept.target, Some(roas("6")));

        fx.channel.campaigns.lock().unwrap().clear();
        fx.clock.advance(TimeDelta::days(1));
        fx.ads.sync(fx.channel.as_ref()).await.unwrap();
        assert!(fx.campaigns.campaigns().await.unwrap().is_empty());
    });
}
