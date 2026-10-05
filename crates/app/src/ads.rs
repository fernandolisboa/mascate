//! Product Ads (#27) as the screens share them: read once a day after an
//! Order Sync while the Reputation unlocks them, their cost handed to
//! Commerce for each sale's Realized Margin, and each Product's ROAS
//! against its break-even, worked out from Commerce's prices and summed up
//! by Marketing. Nothing goes to Mercado Livre: campaigns stay in its own
//! panel (ADR 0025).

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use gpui_kit::{App, Global, Task};
use mascate_catalog::Catalog;
use mascate_commerce::{AdCost, Listings, Pricing};
use mascate_finance::Taxes;
use mascate_integrations::{Connection, ConnectionState, MercadoLivre};
use mascate_kernel::{RecordId, Timestamp};
use mascate_marketing::{
    AdsError, AdsReport, AdsSync, AdvertisedListing, ProductAds, Reputation, SellerTool,
};

use crate::connections::AppConnections;
use crate::listings;
use crate::mercado_livre;
use crate::pricing;
use crate::reputation::{self, reputation};

/// The app's Product Ads; absent when the database did not open.
pub struct AppProductAds(pub Arc<ProductAds>);

impl Global for AppProductAds {}

pub(crate) fn product_ads(cx: &App) -> Option<Arc<ProductAds>> {
    cx.try_global::<AppProductAds>().map(|app| app.0.clone())
}

/// The Syncs of Product Ads since the app opened; the screens that show
/// them read them again after each.
#[derive(Default)]
pub struct AdsSyncs {
    pub finished: u64,
}

impl Global for AdsSyncs {}

pub(crate) fn failure(error: &AdsError) -> String {
    match error {
        AdsError::Platform(error) => mercado_livre::failure(error),
        AdsError::Currencies(_) => "Os valores do Product Ads estão em moedas diferentes.".into(),
        error => format!("Não consegui ler ou gravar o Product Ads no banco: {error}"),
    }
}

/// Whether the Reputation, as last read, unlocks Product Ads; `None`
/// before it was ever read.
pub(crate) async fn unlocked(reputation: &Reputation) -> Result<Option<bool>, String> {
    Ok(reputation
        .standing()
        .await
        .map_err(|e| reputation::failure(&e))?
        .map(|standing| {
            standing
                .tools
                .iter()
                .any(|(tool, unlocked)| *tool == SellerTool::ProductAds && *unlocked)
        }))
}

/// Runs a Sync of Product Ads off the UI thread, once a day or when
/// `forced`, and only while the Reputation unlocks them. `Ok(None)` when
/// the Connection is not up, Product Ads are locked or nothing was due.
pub fn sync_now(cx: &mut App, forced: bool) -> Option<Task<Result<Option<AdsSync>, String>>> {
    let (Some(ads), Some(channel), Some(reputation)) =
        (product_ads(cx), mercado_livre::adapter(cx), reputation(cx))
    else {
        return None;
    };
    let connections = cx.global::<AppConnections>().0.clone();
    Some(cx.background_executor().spawn(async move {
        if connections.state(Connection::MercadoLivre) != ConnectionState::Connected
            || unlocked(&reputation).await? != Some(true)
        {
            return Ok(None);
        }
        if !forced && !ads.is_due().await.map_err(|e| failure(&e))? {
            return Ok(None);
        }
        ads.sync(channel.as_ref())
            .await
            .map(Some)
            .map_err(|e| failure(&e))
    }))
}

/// Tells the screens a Sync ran.
pub fn finished(outcome: &Result<Option<AdsSync>, String>, cx: &mut App) {
    if let Err(error) = outcome {
        eprintln!("could not sync the Product Ads: {error}");
    }
    cx.default_global::<AdsSyncs>().finished += 1;
}

/// What Product Ads cost, by listing and day, as Commerce takes it into
/// the Realized Margin; nothing without the app's Product Ads.
pub(crate) async fn costs(ads: Option<&ProductAds>) -> Result<Vec<AdCost>, String> {
    let Some(ads) = ads else {
        return Ok(Vec::new());
    };
    Ok(ads
        .costs()
        .await
        .map_err(|e| failure(&e))?
        .into_iter()
        .map(|day| AdCost {
            listing: day.listing,
            day: day.day,
            cost: day.cost,
        })
        .collect())
}

/// What the report needs from the rest of the app.
pub(crate) struct ReportSources {
    pub ads: Arc<ProductAds>,
    pub listings: Arc<Listings>,
    pub catalog: Arc<Catalog>,
    pub pricing: Arc<Pricing>,
    pub taxes: Arc<Taxes>,
    /// `None` while the Connection is not up: the break-even needs the
    /// channel's sale fee.
    pub channel: Option<Arc<MercadoLivre>>,
}

/// The ads of the days that start `during`, by campaign and by Product,
/// each advertised listing with its Product and the break-even at today's
/// price. A break-even that cannot be worked out (no Average Cost, no
/// Connection) is left unknown.
pub(crate) async fn report(
    sources: &ReportSources,
    during: Range<Timestamp>,
) -> Result<AdsReport, String> {
    let ads = &sources.ads;
    let advertised = ads
        .report(during.clone(), &[])
        .await
        .map_err(|e| failure(&e))?;
    if advertised.total.is_none() {
        return Ok(advertised);
    }
    let sold = listings::listing_products(&sources.listings, &sources.catalog).await?;
    let mut records: BTreeMap<String, RecordId> = BTreeMap::new();
    for listing in sources
        .listings
        .listings()
        .await
        .map_err(|e| listings::failure(&e))?
    {
        records.entry(listing.listed.id).or_insert(listing.id);
    }
    let assumptions = pricing::assumptions(&sources.catalog, &sources.taxes).await?;
    let mut known = Vec::new();
    for item in advertised.products.iter().flat_map(|row| &row.listings) {
        let Some(product) = sold.get(item) else {
            continue;
        };
        let break_even = match (&sources.channel, records.get(item)) {
            (Some(channel), Some(&record)) => sources
                .pricing
                .break_even(record, channel.as_ref(), assumptions)
                .await
                .ok(),
            _ => None,
        };
        known.push(AdvertisedListing {
            listing: item.clone(),
            product: product
                .product
                .map_or_else(|| item.clone(), |product| product.to_string()),
            name: product.name.clone(),
            break_even,
        });
    }
    ads.report(during, &known).await.map_err(|e| failure(&e))
}
