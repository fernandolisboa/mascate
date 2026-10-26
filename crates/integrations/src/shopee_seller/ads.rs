//! Shopee Ads for marketing's port (#183, ADR 0036 decision 9): the shop's
//! product-level campaigns from `v2.ads`, and each campaign's day of
//! performance placed on its products. Read only: Shopee lets only the
//! sellers on its list create or change ads by API. The Ads API needs a
//! permission Shopee support grants to the shop; without it the shop has
//! no account here.

use chrono::{NaiveDate, TimeDelta};
use mascate_commerce::split_by_value;
use mascate_kernel::{Currency, Money, PlatformError, Roas, SalesChannelKind};
use mascate_marketing::{
    AdMetrics, CampaignStatus, ChannelAd, ChannelAdGroup, ChannelAds, ChannelCampaign,
};
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;

use super::answers::{
    AdCampaignIds, AdCampaignSetting, AdCampaignSettings, AdDayMetrics, AdShopPerformance, Envelope,
};
use super::{ShopeeSeller, again_or_done, answered, read_json};
use crate::answers::decimal;
use crate::retry::Attempt;

const CAMPAIGN_IDS: &str = "/api/v2/ads/get_product_level_campaign_id_list";
const CAMPAIGN_SETTINGS: &str = "/api/v2/ads/get_product_level_campaign_setting_info";
const DAILY_PERFORMANCE: &str = "/api/v2/ads/get_product_campaign_daily_performance";

/// Campaigns per page of the list, and per call of their settings and
/// performance: the most Shopee takes.
const BATCH: usize = 100;

/// Pages read at most, 10,000 campaigns: past them the answer is not
/// trusted to end.
const MAX_PAGES: usize = 100;

/// Common info, auto bidding info and the products of an automatic
/// selection.
const SETTINGS_INFO: &str = "1,3,4";

/// The day format of the performance calls.
const DAY_FORMAT: &str = "%d-%m-%Y";

impl ChannelAds for ShopeeSeller {
    fn advertised_channel(&self) -> SalesChannelKind {
        SalesChannelKind::Shopee
    }

    /// The shop itself, when Shopee lets the app read its ads.
    fn account(&self) -> Result<Option<String>, PlatformError> {
        let shop = self.shop_id()?;
        let probe: Option<AdCampaignIds> = self.get_reading(
            CAMPAIGN_IDS,
            &[("ad_type", "all"), ("offset", "0"), ("limit", "1")],
            ads_answer,
        )?;
        Ok(probe.map(|_| shop))
    }

    fn campaigns(&self, _: &str) -> Result<Vec<ChannelCampaign>, PlatformError> {
        Ok(self
            .settings()?
            .into_iter()
            .map(|setting| campaign(&setting))
            .collect())
    }

    /// Shopee tells a campaign's day, not each product's: a campaign with
    /// one product is that Listing's, and one with several, as an automatic
    /// selection, is split evenly across them, in cents that add up.
    fn ads_on(&self, _: &str, day: NaiveDate) -> Result<Vec<ChannelAd>, PlatformError> {
        let settings = self.settings()?;
        let mut ads = Vec::new();
        for batch in settings.chunks(BATCH) {
            for shop in self.performance(batch, day)? {
                let currency = currency(shop.region.as_deref())?;
                for found in shop.campaign_list {
                    let Some(setting) = batch
                        .iter()
                        .find(|setting| setting.campaign_id == found.campaign_id)
                    else {
                        continue;
                    };
                    let Some(measured) = found.metrics_list.iter().find(|metrics| {
                        NaiveDate::parse_from_str(&metrics.date, DAY_FORMAT) == Ok(day)
                    }) else {
                        continue;
                    };
                    let listings = listings(setting);
                    let shares = split(&metrics(measured, currency), listings.len());
                    ads.extend(listings.into_iter().zip(shares).map(|(listing, metrics)| {
                        ChannelAd {
                            listing,
                            campaign: Some(found.campaign_id.clone()),
                            metrics,
                        }
                    }));
                }
            }
        }
        Ok(ads)
    }

    /// Shopee advertises each product on its own: no Ad Groups.
    fn ad_groups(&self, _: &str) -> Result<Vec<ChannelAdGroup>, PlatformError> {
        Ok(Vec::new())
    }
}

impl ShopeeSeller {
    /// Every product-level campaign of the shop with its settings.
    fn settings(&self) -> Result<Vec<AdCampaignSetting>, PlatformError> {
        let mut ids = Vec::new();
        let limit = BATCH.to_string();
        for page in 0..MAX_PAGES {
            let offset = (page * BATCH).to_string();
            let found: AdCampaignIds = self.read_ads(
                CAMPAIGN_IDS,
                &[("ad_type", "all"), ("offset", &offset), ("limit", &limit)],
            )?;
            let last = !found.has_next_page || found.campaign_list.is_empty();
            ids.extend(found.campaign_list.into_iter().map(|id| id.campaign_id));
            if last {
                break;
            }
        }
        let mut settings = Vec::new();
        for batch in ids.chunks(BATCH) {
            let found: AdCampaignSettings = self.read_ads(
                CAMPAIGN_SETTINGS,
                &[
                    ("info_type_list", SETTINGS_INFO),
                    ("campaign_id_list", &batch.join(",")),
                ],
            )?;
            settings.extend(found.campaign_list);
        }
        Ok(settings)
    }

    /// The campaigns' performance of `day`. Shopee refuses a range that
    /// starts and ends on the same day, so it reads the day after too,
    /// which is at most today.
    fn performance(
        &self,
        campaigns: &[AdCampaignSetting],
        day: NaiveDate,
    ) -> Result<Vec<AdShopPerformance>, PlatformError> {
        let ids: Vec<&str> = campaigns
            .iter()
            .map(|setting| setting.campaign_id.as_str())
            .collect();
        let start = day.format(DAY_FORMAT).to_string();
        let end = (day + TimeDelta::days(1)).format(DAY_FORMAT).to_string();
        self.read_ads(
            DAILY_PERFORMANCE,
            &[
                ("start_date", &start),
                ("end_date", &end),
                ("campaign_id_list", &ids.join(",")),
            ],
        )
    }

    /// An Ads call made after the shop's permission was found: losing it
    /// in between is an error.
    fn read_ads<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, PlatformError> {
        self.get_reading(path, query, ads_answer)?.ok_or_else(|| {
            PlatformError::Refused("Shopee no longer lets the app read the shop's ads".into())
        })
    }
}

/// Shopee's answer to an Ads call: `None` when the shop has not been
/// granted the Ads API, and its own rate limits retried as the rest.
fn ads_answer<T: DeserializeOwned>(
    response: ureq::http::Response<ureq::Body>,
    path: &str,
) -> Attempt<Option<T>> {
    let status = response.status().as_u16();
    let Ok(envelope) = read_json::<Envelope<T>>(response) else {
        return again_or_done(status, path);
    };
    match envelope.error.as_deref() {
        Some("error_permission_denied" | "error_api_permission") => Attempt::Done(Ok(None)),
        Some(error) if error.starts_with("ads.rate_limit.") => {
            Attempt::Again(PlatformError::RateLimited)
        }
        _ => match answered(status, envelope, path) {
            Attempt::Done(answer) => Attempt::Done(answer.map(Some)),
            Attempt::Again(error) => Attempt::Again(error),
        },
    }
}

/// Shopee's Ads send no currency: a Brazilian shop spends and sells in
/// reais.
fn currency(region: Option<&str>) -> Result<Currency, PlatformError> {
    match region {
        Some("BR") | None => Ok(Currency::Brl),
        Some(other) => Err(PlatformError::Refused(format!(
            "Shopee Ads of a shop in {other}, not in Brazil"
        ))),
    }
}

fn campaign(setting: &AdCampaignSetting) -> ChannelCampaign {
    let common = setting.common_info.as_ref();
    ChannelCampaign {
        id: setting.campaign_id.clone(),
        name: common
            .and_then(|common| common.ad_name.clone())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| format!("Campanha {}", setting.campaign_id)),
        status: match common.and_then(|common| common.campaign_status.as_deref()) {
            Some("ongoing") => CampaignStatus::Active,
            Some("paused") => CampaignStatus::Paused,
            _ => CampaignStatus::Other,
        },
        // Shopee does not say whether the budget is the day's or the
        // campaign's whole, and 0 means no limit.
        budget: None,
        strategy: None,
        target: setting
            .auto_bidding_info
            .as_ref()
            .and_then(|bidding| bidding.roas_target.as_ref())
            .and_then(decimal)
            .filter(|target| *target > Decimal::ZERO)
            .and_then(Roas::new),
    }
}

/// The Listings a campaign advertises: those it lists, or those Shopee
/// picked for an automatic selection.
fn listings(setting: &AdCampaignSetting) -> Vec<String> {
    let listed = setting
        .common_info
        .as_ref()
        .map(|common| common.item_id_list.clone())
        .unwrap_or_default();
    let picked = setting
        .auto_product_ads_info
        .iter()
        .map(|product| product.item_id);
    let mut items: Vec<u64> = if listed.is_empty() {
        picked.collect()
    } else {
        listed
    };
    items.sort_unstable();
    items.dedup();
    items.into_iter().map(|item| item.to_string()).collect()
}

fn metrics(day: &AdDayMetrics, currency: Currency) -> AdMetrics {
    let money = |number: Option<&serde_json::Number>| {
        Money::new(number.and_then(decimal).unwrap_or_default(), currency)
    };
    AdMetrics {
        cost: money(day.expense.as_ref()),
        attributed: money(day.broad_gmv.as_ref()),
        clicks: day.clicks,
        prints: day.impression,
        units: day.broad_order_amount,
    }
}

/// `metrics` in `parts` even shares, the first ones taking what does not
/// split: every cent and count lands once.
fn split(metrics: &AdMetrics, parts: usize) -> Vec<AdMetrics> {
    let even = vec![Decimal::ONE; parts];
    let costs = split_by_value(metrics.cost, &even);
    let attributed = split_by_value(metrics.attributed, &even);
    let count = |total: u64, at: usize| {
        let parts = parts as u64;
        total / parts + u64::from((at as u64) < total % parts)
    };
    costs
        .into_iter()
        .zip(attributed)
        .enumerate()
        .map(|(at, (cost, attributed))| AdMetrics {
            cost,
            attributed,
            clicks: count(metrics.clicks, at),
            prints: count(metrics.prints, at),
            units: count(metrics.units, at),
        })
        .collect()
}
