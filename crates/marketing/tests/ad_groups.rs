//! Ad Groups in Ad Campaigns through marketing's public interface, with a
//! real temporary database and an in-memory channel: the daily Sync keeps
//! every Ad Group with its Listings; placing, moving, pausing and taking
//! out an ad goes once, only after the owner confirms it; a family moves
//! all its Listings together; a lost answer is never sent twice.

use std::sync::{Arc, Mutex};

use chrono::{NaiveDate, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::PlatformError;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_marketing::{
    AdCampaigns, AdChange, AdChangeKind, AdGroupKind, AdState, AdStatus, AdsError, CampaignPlan,
    CampaignStatus, ChannelAd, ChannelAdCampaigns, ChannelAdGroup, ChannelAds, ChannelCampaign,
    MIGRATIONS, ProductAds,
};
use mascate_platform::{Database, migrate};

fn group(
    id: &str,
    kind: AdGroupKind,
    listings: &[&str],
    campaign: Option<&str>,
    status: AdStatus,
) -> ChannelAdGroup {
    ChannelAdGroup {
        id: id.into(),
        kind,
        title: None,
        listings: listings
            .iter()
            .map(|listing| (*listing).to_owned())
            .collect(),
        campaign: campaign.map(str::to_owned),
        status,
    }
}

/// A listing advertised in "Fones".
fn cabo() -> ChannelAdGroup {
    group(
        "1406300001",
        AdGroupKind::Item,
        &["MLB4100000001"],
        Some("351"),
        AdStatus::Active,
    )
}

/// A family of two variations advertised in "Fones".
fn fone_family() -> ChannelAdGroup {
    ChannelAdGroup {
        title: Some("Fone Bluetooth".into()),
        ..group(
            "1406300002",
            AdGroupKind::Family,
            &["MLB4100000004", "MLB4100000005"],
            Some("351"),
            AdStatus::Active,
        )
    }
}

/// A listing never advertised.
fn capa() -> ChannelAdGroup {
    group(
        "1406300003",
        AdGroupKind::Item,
        &["MLB4100000003"],
        None,
        AdStatus::Paused,
    )
}

fn campaigns() -> Vec<ChannelCampaign> {
    vec![
        ChannelCampaign::named("351", "Fones", CampaignStatus::Active),
        ChannelCampaign::named("352", "Cabos", CampaignStatus::Paused),
    ]
}

fn into(campaign: &str, status: AdStatus) -> AdChange {
    AdChange::Place {
        campaign: campaign.into(),
        status,
    }
}

/// What the channel was asked to write.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Write {
    Place(String, String, AdStatus),
    Remove(String, String),
}

/// A channel answering from memory.
struct Channel {
    groups: Mutex<Vec<ChannelAdGroup>>,
    writes: Mutex<Vec<Write>>,
    /// The next write is made on the channel but its answer never arrives.
    loses_answer: Mutex<bool>,
    /// Every write answers this instead.
    refuses: Mutex<Option<PlatformError>>,
}

impl Channel {
    fn new() -> Self {
        Self {
            groups: Mutex::new(vec![cabo(), fone_family(), capa()]),
            writes: Mutex::default(),
            loses_answer: Mutex::default(),
            refuses: Mutex::default(),
        }
    }

    fn writes(&self) -> Vec<Write> {
        self.writes.lock().unwrap().clone()
    }

    fn write(&self, write: Write) -> Result<(), PlatformError> {
        self.writes.lock().unwrap().push(write);
        match self.refuses.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn lost(&self) -> Result<(), PlatformError> {
        if std::mem::take(&mut *self.loses_answer.lock().unwrap()) {
            return Err(PlatformError::Failed("connection reset".into()));
        }
        Ok(())
    }

    fn change(&self, id: &str, change: impl FnOnce(&mut ChannelAdGroup)) {
        let mut groups = self.groups.lock().unwrap();
        if let Some(group) = groups.iter_mut().find(|group| group.id == id) {
            change(group);
        }
    }
}

impl ChannelAds for Channel {
    fn account(&self) -> Result<Option<String>, PlatformError> {
        Ok(Some("8823".into()))
    }

    fn campaigns(&self, _: &str) -> Result<Vec<ChannelCampaign>, PlatformError> {
        Ok(campaigns())
    }

    fn ads_on(&self, _: &str, _: NaiveDate) -> Result<Vec<ChannelAd>, PlatformError> {
        Ok(Vec::new())
    }

    fn ad_groups(&self, account: &str) -> Result<Vec<ChannelAdGroup>, PlatformError> {
        assert_eq!(account, "8823");
        Ok(self.groups.lock().unwrap().clone())
    }
}

impl ChannelAdCampaigns for Channel {
    fn create_campaign(&self, _: &str, _: &CampaignPlan) -> Result<ChannelCampaign, PlatformError> {
        unreachable!("campaigns are tested in tests/campaigns.rs")
    }

    fn change_campaign(&self, _: &str, _: &CampaignPlan) -> Result<ChannelCampaign, PlatformError> {
        unreachable!("campaigns are tested in tests/campaigns.rs")
    }

    fn set_campaign_status(
        &self,
        _: &str,
        _: CampaignStatus,
    ) -> Result<ChannelCampaign, PlatformError> {
        unreachable!("campaigns are tested in tests/campaigns.rs")
    }

    fn ad_group_of(
        &self,
        account: &str,
        listing: &str,
    ) -> Result<Option<ChannelAdGroup>, PlatformError> {
        assert_eq!(account, "8823");
        Ok(self
            .groups
            .lock()
            .unwrap()
            .iter()
            .find(|group| group.listings.iter().any(|kept| kept == listing))
            // Mercado Livre's search by item says nothing of the title.
            .map(|group| ChannelAdGroup {
                title: None,
                ..group.clone()
            }))
    }

    fn place_ad_group(
        &self,
        group: &str,
        campaign: &str,
        status: AdStatus,
    ) -> Result<AdState, PlatformError> {
        self.write(Write::Place(group.into(), campaign.into(), status))?;
        self.change(group, |kept| {
            kept.campaign = Some(campaign.into());
            kept.status = status;
        });
        self.lost()?;
        Ok(AdState {
            campaign: Some(campaign.into()),
            status,
        })
    }

    fn remove_ad_group(&self, campaign: &str, group: &str) -> Result<(), PlatformError> {
        self.write(Write::Remove(campaign.into(), group.into()))?;
        self.change(group, |kept| kept.campaign = None);
        self.lost()
    }
}

/// A channel without a Product Ads account for the owner.
struct NoAccount;

impl ChannelAds for NoAccount {
    fn account(&self) -> Result<Option<String>, PlatformError> {
        Ok(None)
    }

    fn campaigns(&self, _: &str) -> Result<Vec<ChannelCampaign>, PlatformError> {
        unreachable!("no account, no campaigns")
    }

    fn ads_on(&self, _: &str, _: NaiveDate) -> Result<Vec<ChannelAd>, PlatformError> {
        unreachable!("no account, no ads")
    }

    fn ad_groups(&self, _: &str) -> Result<Vec<ChannelAdGroup>, PlatformError> {
        unreachable!("no account, no Ad Groups")
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    ads: ProductAds,
    campaigns: AdCampaigns,
    channel: Channel,
}

impl Fixture {
    /// The owner's Product Ads read once.
    async fn synced() -> Self {
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
        let fx = Self {
            ads: ProductAds::new(database.clone(), clock.clone(), ids.clone()),
            campaigns: AdCampaigns::new(database, clock.clone(), ids),
            _dir: dir,
            clock,
            channel: Channel::new(),
        };
        fx.sync().await;
        fx
    }

    async fn sync(&self) {
        self.clock.advance(TimeDelta::minutes(1));
        self.ads.sync(&self.channel).await.unwrap();
    }

    async fn kept(&self, id: &str) -> Option<ChannelAdGroup> {
        self.campaigns
            .ad_groups()
            .await
            .unwrap()
            .into_iter()
            .find(|group| group.id == id)
    }

    async fn send(&self, listing: &str, change: AdChange) -> Result<ChannelAdGroup, AdsError> {
        let request = self.campaigns.prepare_ad(listing, change, true).await?;
        self.clock.advance(TimeDelta::seconds(1));
        self.campaigns
            .send_ad(&self.channel, request.confirm())
            .await
    }
}

#[test]
fn a_sync_keeps_every_ad_group_with_its_listings_campaign_and_status() {
    block_on(async {
        let fx = Fixture::synced().await;

        assert_eq!(
            fx.campaigns.ad_groups().await.unwrap(),
            [cabo(), fone_family(), capa()]
        );

        fx.sync().await;
        assert_eq!(
            fx.campaigns.ad_groups().await.unwrap(),
            [cabo(), fone_family(), capa()]
        );
    });
}

#[test]
fn a_sync_follows_the_channel_when_a_group_or_a_variation_is_gone() {
    block_on(async {
        let fx = Fixture::synced().await;
        fx.channel
            .groups
            .lock()
            .unwrap()
            .retain(|group| group.id != capa().id);
        fx.channel
            .change(&fone_family().id, |family| family.listings.truncate(1));

        fx.sync().await;

        let mut family = fone_family();
        family.listings.truncate(1);
        assert_eq!(fx.campaigns.ad_groups().await.unwrap(), [cabo(), family]);
    });
}

#[test]
fn putting_a_listing_never_advertised_into_a_campaign_goes_once_after_confirmation() {
    block_on(async {
        let fx = Fixture::synced().await;

        let request = fx
            .campaigns
            .prepare_ad("MLB4100000003", into("352", AdStatus::Active), true)
            .await
            .unwrap();

        assert!(fx.channel.writes().is_empty());
        assert_eq!(request.kind(), AdChangeKind::Place);
        assert_eq!(request.listings(), ["MLB4100000003"]);
        assert_eq!(request.from(), None);
        assert_eq!(request.to().map(|to| to.name.as_str()), Some("Cabos"));
        assert_eq!(request.status(), Some(AdStatus::Active));

        let placed = fx
            .campaigns
            .send_ad(&fx.channel, request.confirm())
            .await
            .unwrap();

        assert_eq!(
            fx.channel.writes(),
            [Write::Place(capa().id, "352".into(), AdStatus::Active)]
        );
        let expected = ChannelAdGroup {
            campaign: Some("352".into()),
            status: AdStatus::Active,
            ..capa()
        };
        assert_eq!(placed, expected);
        assert_eq!(fx.kept(&capa().id).await, Some(expected));
    });
}

#[test]
fn a_family_lists_every_listing_it_moves_and_moves_them_together() {
    block_on(async {
        let fx = Fixture::synced().await;

        let request = fx
            .campaigns
            .prepare_ad("MLB4100000005", into("352", AdStatus::Active), true)
            .await
            .unwrap();

        assert_eq!(request.kind(), AdChangeKind::Move);
        assert_eq!(request.group().kind, AdGroupKind::Family);
        assert_eq!(request.listings(), ["MLB4100000004", "MLB4100000005"]);
        assert_eq!(request.from().map(|from| from.name.as_str()), Some("Fones"));
        assert_eq!(request.to().map(|to| to.name.as_str()), Some("Cabos"));

        fx.campaigns
            .send_ad(&fx.channel, request.confirm())
            .await
            .unwrap();

        assert_eq!(
            fx.channel.writes(),
            [Write::Place(
                fone_family().id,
                "352".into(),
                AdStatus::Active
            )]
        );
        let moved = fx.kept(&fone_family().id).await.unwrap();
        assert_eq!(moved.campaign.as_deref(), Some("352"));
        assert_eq!(moved.listings, ["MLB4100000004", "MLB4100000005"]);
        assert_eq!(moved.title.as_deref(), Some("Fone Bluetooth"));
    });
}

#[test]
fn pausing_activating_and_taking_out_send_one_request_each() {
    block_on(async {
        let fx = Fixture::synced().await;
        let cabo_id = cabo().id;

        let pause = fx
            .campaigns
            .prepare_ad("MLB4100000001", into("351", AdStatus::Paused), true)
            .await
            .unwrap();
        assert_eq!(pause.kind(), AdChangeKind::Pause);
        fx.campaigns
            .send_ad(&fx.channel, pause.confirm())
            .await
            .unwrap();
        assert_eq!(fx.kept(&cabo_id).await.unwrap().status, AdStatus::Paused);

        let activate = fx
            .campaigns
            .prepare_ad("MLB4100000001", into("351", AdStatus::Active), true)
            .await
            .unwrap();
        assert_eq!(activate.kind(), AdChangeKind::Activate);
        fx.campaigns
            .send_ad(&fx.channel, activate.confirm())
            .await
            .unwrap();

        let remove = fx
            .campaigns
            .prepare_ad("MLB4100000001", AdChange::Remove, true)
            .await
            .unwrap();
        assert_eq!(remove.kind(), AdChangeKind::Remove);
        assert_eq!(remove.to(), None);
        assert_eq!(remove.status(), None);
        fx.campaigns
            .send_ad(&fx.channel, remove.confirm())
            .await
            .unwrap();

        assert_eq!(
            fx.channel.writes(),
            [
                Write::Place(cabo_id.clone(), "351".into(), AdStatus::Paused),
                Write::Place(cabo_id.clone(), "351".into(), AdStatus::Active),
                Write::Remove("351".into(), cabo_id.clone()),
            ]
        );
        assert_eq!(fx.kept(&cabo_id).await.unwrap().campaign, None);
    });
}

#[test]
fn a_placement_whose_answer_was_lost_is_found_where_it_went_and_not_sent_again() {
    block_on(async {
        let fx = Fixture::synced().await;
        *fx.channel.loses_answer.lock().unwrap() = true;
        let request = fx
            .campaigns
            .prepare_ad("MLB4100000001", into("352", AdStatus::Active), true)
            .await
            .unwrap();

        let lost = fx
            .campaigns
            .send_ad(&fx.channel, request.clone().confirm())
            .await;
        assert!(matches!(
            lost,
            Err(AdsError::Platform(PlatformError::Failed(_)))
        ));

        let again = fx
            .campaigns
            .send_ad(&fx.channel, request.confirm())
            .await
            .unwrap();

        assert_eq!(fx.channel.writes().len(), 1);
        assert_eq!(again.campaign.as_deref(), Some("352"));
        assert_eq!(fx.kept(&cabo().id).await, Some(again));
    });
}

#[test]
fn taking_out_again_an_ad_the_channel_already_took_out_ends_the_same() {
    block_on(async {
        let fx = Fixture::synced().await;
        *fx.channel.loses_answer.lock().unwrap() = true;
        let request = fx
            .campaigns
            .prepare_ad("MLB4100000001", AdChange::Remove, true)
            .await
            .unwrap();

        assert!(
            fx.campaigns
                .send_ad(&fx.channel, request.clone().confirm())
                .await
                .is_err()
        );
        let again = fx
            .campaigns
            .send_ad(&fx.channel, request.confirm())
            .await
            .unwrap();

        assert_eq!(again.campaign, None);
        assert_eq!(fx.kept(&cabo().id).await.unwrap().campaign, None);
        fx.sync().await;
        assert_eq!(fx.kept(&cabo().id).await.unwrap().campaign, None);
    });
}

#[test]
fn a_family_that_changed_since_the_owner_read_it_is_kept_and_nothing_is_sent() {
    block_on(async {
        let fx = Fixture::synced().await;
        let request = fx
            .campaigns
            .prepare_ad("MLB4100000004", into("352", AdStatus::Active), true)
            .await
            .unwrap();
        fx.channel.change(&fone_family().id, |family| {
            family.listings.push("MLB4100000007".into())
        });

        let sent = fx.campaigns.send_ad(&fx.channel, request.confirm()).await;

        assert!(matches!(sent, Err(AdsError::AdGroupChanged)));
        assert!(fx.channel.writes().is_empty());
        assert_eq!(
            fx.kept(&fone_family().id).await.unwrap().listings,
            ["MLB4100000004", "MLB4100000005", "MLB4100000007"]
        );
    });
}

#[test]
fn a_listing_without_an_ad_group_cannot_be_advertised_yet() {
    block_on(async {
        let fx = Fixture::synced().await;

        assert!(matches!(
            fx.campaigns
                .prepare_ad("MLB4199999999", into("351", AdStatus::Active), true)
                .await,
            Err(AdsError::NoAdGroup)
        ));
    });
}

#[test]
fn a_change_that_changes_nothing_or_goes_nowhere_is_refused_before_the_channel() {
    block_on(async {
        let fx = Fixture::synced().await;
        let prepare = |listing: &'static str, change: AdChange| {
            let campaigns = &fx.campaigns;
            async move { campaigns.prepare_ad(listing, change, true).await }
        };

        assert!(matches!(
            prepare("MLB4100000001", into("351", AdStatus::Active)).await,
            Err(AdsError::Unchanged)
        ));
        assert!(matches!(
            prepare("MLB4100000003", AdChange::Remove).await,
            Err(AdsError::Unchanged)
        ));
        assert!(matches!(
            prepare("MLB4100000001", into("999", AdStatus::Active)).await,
            Err(AdsError::UnknownCampaign)
        ));
        assert!(matches!(
            prepare("MLB4100000001", into("352", AdStatus::Other)).await,
            Err(AdsError::NotRunnable)
        ));
        assert!(matches!(
            fx.campaigns
                .prepare_ad("MLB4100000001", into("352", AdStatus::Active), false)
                .await,
            Err(AdsError::Locked)
        ));
        assert!(fx.channel.writes().is_empty());
    });
}

#[test]
fn a_write_the_channel_does_not_allow_says_so_and_keeps_the_ad_as_it_was() {
    block_on(async {
        let fx = Fixture::synced().await;
        *fx.channel.refuses.lock().unwrap() = Some(PlatformError::NotFound);

        let sent = fx
            .send("MLB4100000001", into("352", AdStatus::Active))
            .await;

        assert!(matches!(sent, Err(AdsError::NotManaged)));
        assert_eq!(fx.kept(&cabo().id).await, Some(cabo()));
    });
}

#[test]
fn the_account_a_sync_read_is_needed_to_place_an_ad() {
    block_on(async {
        let fx = Fixture::synced().await;
        let request = fx
            .campaigns
            .prepare_ad("MLB4100000003", into("351", AdStatus::Active), true)
            .await
            .unwrap();
        // A later Sync finds no account.
        fx.ads.sync(&NoAccount).await.unwrap();

        assert!(matches!(
            fx.campaigns.send_ad(&fx.channel, request.confirm()).await,
            Err(AdsError::NoAccount)
        ));
        assert!(fx.channel.writes().is_empty());
    });
}
