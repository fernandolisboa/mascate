//! Ad Groups in Ad Campaigns (#181, ADR 0036): the owner puts a Listing's
//! ad into a campaign, moves it to another, pauses, activates or takes it
//! out, through the same port that changes campaigns. The Ad Groups are
//! kept as the daily Sync and each change leave them, one row per Ad Group
//! and Listing, so a family of variations or a catalog product moves all
//! its Listings together and the confirmation lists them. Nothing goes to
//! the channel without the owner's confirmation; a Product whose ads lose
//! money is only suggested for a pause ([`crate::ProductRoas::suggests_pausing`]).

use libsql::{Connection, Value, params};
use mascate_kernel::{IdGenerator, Timestamp};
use mascate_platform::{Migration, stored};

use crate::ads::{AdsError, ChannelCampaign, ads_account, read_campaigns};
use crate::campaigns::{AdCampaigns, ChannelAdCampaigns, written};

pub(crate) const KEEP_AD_GROUPS: Migration = Migration {
    version: 14,
    name: "keep each Ad Group's Listings, campaign and status",
    risky: false,
    sql: "CREATE TABLE marketing_ad_groups (
        id                  TEXT PRIMARY KEY,
        sales_channel       TEXT NOT NULL DEFAULT 'mercado_livre',
        channel_ad_group_id TEXT NOT NULL,
        channel_item_id     TEXT NOT NULL,
        kind                TEXT NOT NULL,
        title               TEXT,
        channel_campaign_id TEXT,
        status              TEXT NOT NULL,
        created_at          TEXT NOT NULL,
        updated_at          TEXT NOT NULL,
        deleted_at          TEXT
    );
    CREATE UNIQUE INDEX marketing_ad_groups_by_group_and_item
        ON marketing_ad_groups (sales_channel, channel_ad_group_id, channel_item_id)
        WHERE deleted_at IS NULL;",
};

/// What a channel advertises as one: a Listing, a family of variations or
/// a catalog product.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdGroupKind {
    Item,
    Family,
    Catalog,
    /// Any other kind the channel reports.
    Other,
}

impl AdGroupKind {
    const ALL: [AdGroupKind; 4] = [
        AdGroupKind::Item,
        AdGroupKind::Family,
        AdGroupKind::Catalog,
        AdGroupKind::Other,
    ];

    pub fn code(self) -> &'static str {
        match self {
            AdGroupKind::Item => "item",
            AdGroupKind::Family => "family",
            AdGroupKind::Catalog => "catalog",
            AdGroupKind::Other => "other",
        }
    }

    /// Any case: Mercado Livre answers in upper case.
    pub fn from_code(code: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|kind| kind.code().eq_ignore_ascii_case(code))
            .unwrap_or(AdGroupKind::Other)
    }

    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            AdGroupKind::Item => "Anúncio",
            AdGroupKind::Family => "Família de variações",
            AdGroupKind::Catalog => "Produto de catálogo",
            AdGroupKind::Other => "Outro grupo",
        }
    }
}

/// How the channel runs an ad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdStatus {
    Active,
    Paused,
    /// Any other state the channel reports, such as one under review.
    Other,
}

impl AdStatus {
    const ALL: [AdStatus; 3] = [AdStatus::Active, AdStatus::Paused, AdStatus::Other];

    pub fn code(self) -> &'static str {
        match self {
            AdStatus::Active => "active",
            AdStatus::Paused => "paused",
            AdStatus::Other => "other",
        }
    }

    /// Any case: Mercado Livre answers in upper case.
    pub fn from_code(code: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|status| status.code().eq_ignore_ascii_case(code))
            .unwrap_or(AdStatus::Other)
    }

    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            AdStatus::Active => "Ativo",
            AdStatus::Paused => "Pausado",
            AdStatus::Other => "Outro estado",
        }
    }
}

/// An Ad Group as the channel reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelAdGroup {
    /// The channel's id.
    pub id: String,
    pub kind: AdGroupKind,
    /// What the channel calls it; `None` when it says nothing.
    pub title: Option<String>,
    /// The channel's ids of the Listings it advertises: one for an `Item`.
    pub listings: Vec<String>,
    /// The channel's id of its campaign; `None` outside any.
    pub campaign: Option<String>,
    pub status: AdStatus,
}

/// Where an ad stands once the channel took a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdState {
    pub campaign: Option<String>,
    pub status: AdStatus,
}

/// What the owner asks of an ad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdChange {
    /// Into `campaign`, running or paused: puts the ad in a campaign,
    /// moves it from another, or pauses or activates it where it is.
    Place { campaign: String, status: AdStatus },
    /// Out of its campaign.
    Remove,
}

/// What a change does to the ad, as the owner reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdChangeKind {
    Place,
    Move,
    Pause,
    Activate,
    Remove,
}

/// An ad change checked and ready for the owner to read. Only
/// [`AdRequest::confirm`] makes it something [`AdCampaigns::send_ad`]
/// takes; dropping it sends nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "an ad change does nothing until confirmed and passed to AdCampaigns::send_ad"]
pub struct AdRequest {
    group: ChannelAdGroup,
    change: AdChange,
    from: Option<ChannelCampaign>,
    to: Option<ChannelCampaign>,
}

impl AdRequest {
    pub fn kind(&self) -> AdChangeKind {
        match &self.change {
            AdChange::Remove => AdChangeKind::Remove,
            AdChange::Place { campaign, status } => match &self.group.campaign {
                None => AdChangeKind::Place,
                Some(now) if now != campaign => AdChangeKind::Move,
                Some(_) if *status == AdStatus::Paused => AdChangeKind::Pause,
                Some(_) => AdChangeKind::Activate,
            },
        }
    }

    /// The Ad Group as kept when the change was checked.
    pub fn group(&self) -> &ChannelAdGroup {
        &self.group
    }

    /// Every Listing the change takes along: all of a family's or catalog
    /// product's.
    pub fn listings(&self) -> &[String] {
        &self.group.listings
    }

    /// The campaign the ad is in; `None` outside any.
    pub fn from(&self) -> Option<&ChannelCampaign> {
        self.from.as_ref()
    }

    /// The campaign the ad goes to; `None` when it comes out.
    pub fn to(&self) -> Option<&ChannelCampaign> {
        self.to.as_ref()
    }

    /// How the ad runs once the change goes; `None` when it comes out.
    pub fn status(&self) -> Option<AdStatus> {
        match &self.change {
            AdChange::Place { status, .. } => Some(*status),
            AdChange::Remove => None,
        }
    }

    /// The owner read what goes to the channel and confirmed it.
    pub fn confirm(self) -> ConfirmedAd {
        ConfirmedAd {
            group: self.group,
            change: self.change,
        }
    }
}

/// An ad change the owner confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfirmedAd {
    group: ChannelAdGroup,
    change: AdChange,
}

impl AdCampaigns {
    /// The Ad Groups as the last Sync or change left them, by the
    /// channel's id.
    pub async fn ad_groups(&self) -> Result<Vec<ChannelAdGroup>, AdsError> {
        read_ad_groups(self.database.connection()).await
    }

    /// Checks a change to the ad of `listing`: a Listing the channel keeps
    /// no Ad Group for cannot be advertised yet; it goes into a live
    /// campaign, active or paused, or out of the one it is in.
    pub async fn prepare_ad(
        &self,
        listing: &str,
        change: AdChange,
        unlocked: bool,
    ) -> Result<AdRequest, AdsError> {
        if !unlocked {
            return Err(AdsError::Locked);
        }
        let group = self
            .ad_groups()
            .await?
            .into_iter()
            .find(|group| group.listings.iter().any(|kept| kept == listing))
            .ok_or(AdsError::NoAdGroup)?;
        let campaigns = read_campaigns(self.database.connection()).await?;
        let find = |id: &str| campaigns.iter().find(|campaign| campaign.id == id).cloned();
        let from = group.campaign.as_deref().and_then(find);
        let to = match &change {
            AdChange::Place { campaign, status } => {
                if !matches!(status, AdStatus::Active | AdStatus::Paused) {
                    return Err(AdsError::NotRunnable);
                }
                if group.campaign.as_ref() == Some(campaign) && group.status == *status {
                    return Err(AdsError::Unchanged);
                }
                Some(find(campaign).ok_or(AdsError::UnknownCampaign)?)
            }
            AdChange::Remove if group.campaign.is_none() => return Err(AdsError::Unchanged),
            AdChange::Remove => None,
        };
        Ok(AdRequest {
            group,
            change,
            from,
            to,
        })
    }

    /// Sends what the owner confirmed, once, and keeps where the ad now
    /// stands. A placement first asks the channel for the Listing's Ad
    /// Group: one already where the owner wants it, from an earlier send
    /// whose answer never arrived, is kept with nothing sent again; one
    /// whose Listings changed since the owner read them is kept and sent
    /// back for a fresh look. Taking out an ad the channel no longer has
    /// counts as taken out.
    pub async fn send_ad(
        &self,
        channel: &dyn ChannelAdCampaigns,
        confirmed: ConfirmedAd,
    ) -> Result<ChannelAdGroup, AdsError> {
        let ConfirmedAd { group, change } = confirmed;
        let _sending = self
            .sending
            .start(format!("ad_group:{}", group.id))
            .ok_or(AdsError::Sending)?;
        let connection = self.database.connection();
        let sent = match change {
            AdChange::Place { campaign, status } => {
                let account = ads_account(connection).await?.ok_or(AdsError::NoAccount)?;
                let listing = group.listings.first().ok_or(AdsError::NoAdGroup)?;
                let found = channel
                    .ad_group_of(&account, listing)
                    .map_err(written)?
                    .ok_or(AdsError::NoAdGroup)?;
                if found.id != group.id || !same_listings(&found.listings, &group.listings) {
                    write_ad_group(connection, self.ids.as_ref(), &found, self.clock.now()).await?;
                    return Err(AdsError::AdGroupChanged);
                }
                if found.campaign.as_ref() == Some(&campaign) && found.status == status {
                    found
                } else {
                    let state = channel
                        .place_ad_group(&found.id, &campaign, status)
                        .map_err(written)?;
                    ChannelAdGroup {
                        campaign: state.campaign,
                        status: state.status,
                        ..found
                    }
                }
            }
            AdChange::Remove => {
                if let Some(campaign) = &group.campaign {
                    channel
                        .remove_ad_group(campaign, &group.id)
                        .map_err(written)?;
                }
                ChannelAdGroup {
                    campaign: None,
                    ..group
                }
            }
        };
        write_ad_group(connection, self.ids.as_ref(), &sent, self.clock.now()).await?;
        Ok(sent)
    }
}

fn same_listings(a: &[String], b: &[String]) -> bool {
    let mut a = a.to_vec();
    let mut b = b.to_vec();
    a.sort();
    b.sort();
    a == b
}

/// Writes the channel's Ad Groups over the app's: each by its id and
/// Listing, and those the channel no longer reports gone.
pub(crate) async fn write_ad_groups(
    on: &Connection,
    ids: &dyn IdGenerator,
    groups: &[ChannelAdGroup],
    now: Timestamp,
) -> Result<(), AdsError> {
    for group in groups {
        write_ad_group(on, ids, group, now).await?;
    }
    on.execute(
        "UPDATE marketing_ad_groups SET deleted_at = ?1, updated_at = ?1
         WHERE deleted_at IS NULL AND updated_at < ?1",
        params![stored(now)],
    )
    .await?;
    Ok(())
}

/// Writes one Ad Group over the app's, a row per Listing; a Listing that
/// left it is gone from it.
async fn write_ad_group(
    on: &Connection,
    ids: &dyn IdGenerator,
    group: &ChannelAdGroup,
    now: Timestamp,
) -> Result<(), AdsError> {
    for listing in &group.listings {
        on.execute(
            "INSERT INTO marketing_ad_groups
                 (id, channel_ad_group_id, channel_item_id, kind, title, channel_campaign_id,
                  status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
             ON CONFLICT (sales_channel, channel_ad_group_id, channel_item_id)
                 WHERE deleted_at IS NULL
             DO UPDATE SET kind = excluded.kind, title = excluded.title,
                 channel_campaign_id = excluded.channel_campaign_id,
                 status = excluded.status, updated_at = excluded.updated_at",
            vec![
                Value::Text(ids.next_id().to_string()),
                Value::Text(group.id.clone()),
                Value::Text(listing.clone()),
                Value::Text(group.kind.code().into()),
                group.title.clone().map_or(Value::Null, Value::Text),
                group.campaign.clone().map_or(Value::Null, Value::Text),
                Value::Text(group.status.code().into()),
                Value::Text(stored(now)),
            ],
        )
        .await?;
    }
    on.execute(
        "UPDATE marketing_ad_groups SET deleted_at = ?1, updated_at = ?1
         WHERE channel_ad_group_id = ?2 AND deleted_at IS NULL AND updated_at < ?1",
        params![stored(now), group.id.clone()],
    )
    .await?;
    Ok(())
}

/// The live Ad Groups, each with its Listings in the order they were kept.
async fn read_ad_groups(on: &Connection) -> Result<Vec<ChannelAdGroup>, AdsError> {
    let mut rows = on
        .query(
            "SELECT channel_ad_group_id, channel_item_id, kind, title, channel_campaign_id, status
             FROM marketing_ad_groups WHERE deleted_at IS NULL
             ORDER BY channel_ad_group_id, channel_item_id",
            (),
        )
        .await?;
    let mut groups: Vec<ChannelAdGroup> = Vec::new();
    while let Some(row) = rows.next().await? {
        let id: String = row.get(0)?;
        let listing: String = row.get(1)?;
        match groups.last_mut() {
            Some(group) if group.id == id => group.listings.push(listing),
            _ => groups.push(ChannelAdGroup {
                id,
                kind: AdGroupKind::from_code(&row.get::<String>(2)?),
                title: row.get(3)?,
                listings: vec![listing],
                campaign: row.get(4)?,
                status: AdStatus::from_code(&row.get::<String>(5)?),
            }),
        }
    }
    Ok(groups)
}
