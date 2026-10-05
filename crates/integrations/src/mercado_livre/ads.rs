//! The owner's Product Ads, for marketing's port (ADR 0025): the account
//! from `/advertising/advertisers?product_id=PADS`, its campaigns and each
//! ad's metrics of a day from the Product Ads search, version 2 of the
//! advertising API. Read only: campaigns are made and changed in Mercado
//! Livre's own panel.

use chrono::NaiveDate;
use mascate_kernel::{Currency, Money, PlatformError};
use mascate_marketing::{AdMetrics, CampaignStatus, ChannelAd, ChannelAds, ChannelCampaign};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::de::DeserializeOwned;
use serde_json::Number;

use super::answers::{
    AdAnswer, AdCampaignAnswer, AdMetricsAnswer, AdsSearchAnswer, AdvertisersAnswer, decimal,
    whole_id,
};
use super::sales_channel::path_id;
use super::{MercadoLivre, SITE};

/// Product Ads, among the advertising products (Brand Ads, Display...).
const PRODUCT_ADS: &str = "PADS";

/// The advertisers list answers in version 1, the Product Ads searches in
/// version 2.
const ADVERTISERS_VERSION: (&str, &str) = ("Api-Version", "1");
const SEARCH_VERSION: (&str, &str) = ("Api-Version", "2");

/// Results per page of a search.
const SEARCH_PAGE: u32 = 50;

/// The metrics each ad comes with.
const METRICS: &str =
    "clicks,prints,cost,direct_amount,indirect_amount,total_amount,units_quantity";

/// Mercado Livre Brasil spends and sells in reais; the advertising API
/// sends no currency.
const CURRENCY: Currency = Currency::Brl;

impl ChannelAds for MercadoLivre {
    /// The owner's advertiser in Mercado Livre Brasil; a user Product Ads
    /// was never unlocked for has none, and may get 404.
    fn account(&self) -> Result<Option<String>, PlatformError> {
        let found: AdvertisersAnswer = match self.get_with(
            "/advertising/advertisers",
            &[("product_id", PRODUCT_ADS)],
            &[ADVERTISERS_VERSION],
        ) {
            Err(PlatformError::NotFound) => return Ok(None),
            other => other?,
        };
        Ok(found
            .advertisers
            .into_iter()
            .find(|advertiser| {
                advertiser
                    .site_id
                    .as_deref()
                    .is_none_or(|site| site == SITE)
            })
            .map(|advertiser| whole_id(&advertiser.advertiser_id)))
    }

    fn campaigns(&self, account: &str) -> Result<Vec<ChannelCampaign>, PlatformError> {
        let path = format!(
            "/advertising/{SITE}/advertisers/{}/product_ads/campaigns/search",
            path_id(account)?
        );
        let found: Vec<AdCampaignAnswer> = self.search(&path, &[])?;
        Ok(found
            .into_iter()
            .map(|campaign| {
                let id = whole_id(&campaign.id);
                ChannelCampaign {
                    name: campaign.name.unwrap_or_else(|| format!("Campanha {id}")),
                    status: status(campaign.status.as_deref()),
                    id,
                }
            })
            .collect())
    }

    fn ads_on(&self, account: &str, day: NaiveDate) -> Result<Vec<ChannelAd>, PlatformError> {
        let path = format!(
            "/advertising/{SITE}/advertisers/{}/product_ads/ads/search",
            path_id(account)?
        );
        let day = day.to_string();
        let found: Vec<AdAnswer> = self.search(
            &path,
            &[("date_from", &day), ("date_to", &day), ("metrics", METRICS)],
        )?;
        Ok(found
            .into_iter()
            .map(|ad| ChannelAd {
                metrics: ad
                    .metrics
                    .as_ref()
                    .map_or(AdMetrics::zero(CURRENCY), metrics),
                listing: ad.item_id,
                campaign: ad.campaign_id.as_ref().map(whole_id),
            })
            .collect())
    }
}

impl MercadoLivre {
    /// Every result of a Product Ads search, page by page.
    fn search<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<Vec<T>, PlatformError> {
        let mut results = Vec::new();
        let limit = SEARCH_PAGE.to_string();
        let mut offset: u32 = 0;
        loop {
            let offset_text = offset.to_string();
            let mut asked: Vec<(&str, &str)> = vec![("limit", &limit), ("offset", &offset_text)];
            asked.extend_from_slice(query);
            let page: AdsSearchAnswer<T> = self.get_with(path, &asked, &[SEARCH_VERSION])?;
            let read = u32::try_from(page.results.len()).unwrap_or(SEARCH_PAGE);
            results.extend(page.results);
            offset += read;
            let total = page.paging.map_or(0, |paging| paging.total);
            if read == 0 || offset >= total {
                return Ok(results);
            }
        }
    }
}

/// `active` and `paused`; anything else, as a campaign being set up, is
/// another state.
fn status(status: Option<&str>) -> CampaignStatus {
    match status {
        Some("active") => CampaignStatus::Active,
        Some("paused") => CampaignStatus::Paused,
        _ => CampaignStatus::Other,
    }
}

/// An ad's metrics; the attributed sales are `total_amount`, or the direct
/// and indirect ones together without it.
fn metrics(answer: &AdMetricsAnswer) -> AdMetrics {
    let amount = |number: &Option<Number>| decimal(number).unwrap_or_default();
    let attributed = match decimal(&answer.total_amount) {
        Some(total) => total,
        None => amount(&answer.direct_amount) + amount(&answer.indirect_amount),
    };
    AdMetrics {
        cost: Money::new(amount(&answer.cost).max(Decimal::ZERO), CURRENCY),
        attributed: Money::new(attributed.max(Decimal::ZERO), CURRENCY),
        clicks: count(&answer.clicks),
        prints: count(&answer.prints),
        units: count(&answer.units_quantity),
    }
}

fn count(number: &Option<Number>) -> u64 {
    decimal(number)
        .and_then(|count| count.trunc().to_u64())
        .unwrap_or(0)
}
