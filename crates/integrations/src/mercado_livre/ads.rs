//! The owner's Product Ads, for marketing's ports (ADR 0025, ADR 0036): the
//! account from `/advertising/advertisers?product_id=PADS`, its campaigns,
//! its Ad Groups and each listing's metrics of a day through the Ad Groups
//! search, version 2 of the advertising API; and the campaigns and Ad
//! Groups the owner creates, changes, moves, pauses and takes out on their
//! confirmed click.

use chrono::NaiveDate;
use mascate_kernel::{Currency, Money, PlatformError, Roas};
use mascate_marketing::{
    AdGroupKind, AdMetrics, AdState, AdStatus, CampaignPlan, CampaignStatus, CampaignStrategy,
    ChannelAd, ChannelAdCampaigns, ChannelAdGroup, ChannelAds, ChannelCampaign,
};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use serde::de::DeserializeOwned;
use serde_json::Number;

use super::answers::{
    AdAnswer, AdCampaignAnswer, AdGroupAnswer, AdMetricsAnswer, AdsSearchAnswer, AdvertisersAnswer,
    decimal, whole_id,
};
use super::sales_channel::path_id;
use super::{MercadoLivre, SITE, failed, number, read_json, text};

/// Product Ads, among the advertising products (Brand Ads, Display...).
const PRODUCT_ADS: &str = "PADS";

/// The advertisers list answers in version 1, the rest of Product Ads in
/// version 2.
const ADVERTISERS_VERSION: (&str, &str) = ("Api-Version", "1");
const PRODUCT_ADS_VERSION: (&str, &str) = ("Api-Version", "2");

/// Results per page of a search.
const SEARCH_PAGE: u32 = 50;

/// The metrics each Ad Group and ad comes with.
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
        Ok(found.into_iter().map(campaign).collect())
    }

    /// Mercado Livre removed the per-ad metrics search on 2026-05-30: the
    /// day is read by Ad Group, and only a family or catalog group that
    /// spent or sold costs a call to split it into its listings.
    fn ads_on(&self, account: &str, day: NaiveDate) -> Result<Vec<ChannelAd>, PlatformError> {
        let path = groups_path(account)?;
        let day = day.to_string();
        let query = [
            ("date_from", day.as_str()),
            ("date_to", &day),
            ("metrics", METRICS),
        ];
        let groups: Vec<AdGroupAnswer> = self.search(&path, &query)?;
        let mut ads = Vec::new();
        for group in groups {
            let measured = metrics(group.metrics.as_ref());
            let campaign = group.campaign_id.as_ref().map(whole_id);
            match group.ad_group_external_id {
                Some(listing) if is_listing(group.ad_group_type.as_deref()) => {
                    ads.push(ChannelAd {
                        listing,
                        campaign,
                        metrics: measured,
                    });
                }
                _ if !spent_or_sold(&measured) => {}
                _ => {
                    let path = format!(
                        "/advertising/{SITE}/product_ads/ad_groups/{}/ads",
                        path_id(&whole_id(&group.id))?
                    );
                    let found: Vec<AdAnswer> = self.search(&path, &query)?;
                    ads.extend(found.into_iter().map(|ad| ChannelAd {
                        metrics: metrics(ad.metrics.as_ref()),
                        listing: ad.item_id,
                        campaign: ad.campaign_id.as_ref().map(whole_id).or(campaign.clone()),
                    }));
                }
            }
        }
        Ok(ads)
    }

    /// Every Ad Group of the account; a family or catalog product costs a
    /// call for its listings.
    fn ad_groups(&self, account: &str) -> Result<Vec<ChannelAdGroup>, PlatformError> {
        let found: Vec<AdGroupAnswer> = self.search(&groups_path(account)?, &[])?;
        found
            .into_iter()
            .map(|answer| self.ad_group(answer))
            .collect()
    }
}

/// Mercado Livre documents these writes on its Global Selling page, with
/// the `/marketplace` prefix; the app sends them with the prefix of the
/// Brazilian reads, to be confirmed with the real account (ADR 0036).
impl ChannelAdCampaigns for MercadoLivre {
    fn create_campaign(
        &self,
        account: &str,
        plan: &CampaignPlan,
    ) -> Result<ChannelCampaign, PlatformError> {
        let mut fields = plan_fields(plan)?;
        fields.push(r#""channel":"marketplace""#.into());
        self.manage(
            Write::Post,
            &format!(
                "/advertising/{SITE}/advertisers/{}/product_ads/campaigns",
                path_id(account)?
            ),
            &fields,
        )
    }

    fn change_campaign(
        &self,
        campaign: &str,
        plan: &CampaignPlan,
    ) -> Result<ChannelCampaign, PlatformError> {
        self.manage(Write::Put, &campaign_path(campaign)?, &plan_fields(plan)?)
    }

    fn set_campaign_status(
        &self,
        campaign: &str,
        status: CampaignStatus,
    ) -> Result<ChannelCampaign, PlatformError> {
        self.manage(
            Write::Put,
            &campaign_path(campaign)?,
            &[format!(r#""status":{}"#, text(status_code(status)?))],
        )
    }

    /// Found by the listing's id: the search by item answers each Ad Group
    /// with the listing as its external id.
    fn ad_group_of(
        &self,
        account: &str,
        listing: &str,
    ) -> Result<Option<ChannelAdGroup>, PlatformError> {
        let found: Vec<AdGroupAnswer> =
            self.search(&groups_path(account)?, &[("filters[item_ids]", listing)])?;
        found
            .into_iter()
            .next()
            .map(|answer| self.ad_group(answer))
            .transpose()
    }

    fn place_ad_group(
        &self,
        group: &str,
        campaign: &str,
        status: AdStatus,
    ) -> Result<AdState, PlatformError> {
        let path = format!(
            "/advertising/{SITE}/product_ads/ad_groups/{}",
            path_id(group)?
        );
        let campaign_id: u64 = campaign
            .parse()
            .map_err(|_| PlatformError::Refused("a campaign id is a number".into()))?;
        let status_code = match status {
            AdStatus::Active => "active",
            AdStatus::Paused => "paused",
            AdStatus::Other => {
                return Err(PlatformError::Refused(
                    "an ad is set to active or paused".into(),
                ));
            }
        };
        let body = format!(
            r#"{{"status":{},"campaign_id":{campaign_id}}}"#,
            text(status_code)
        );
        let response = self.write_ads(Write::Put, &path, &body)?;
        match response.status().as_u16() {
            200 | 201 => read_json::<AdGroupAnswer>(response).map(|answer| AdState {
                campaign: campaign_of(answer.campaign_id.as_ref()),
                status: answer
                    .status
                    .as_deref()
                    .map_or(AdStatus::Other, AdStatus::from_code),
            }),
            403 | 404 => Err(PlatformError::NotFound),
            _ => Err(failed(response, &path)),
        }
    }

    /// An Ad Group the campaign no longer has answers 404: it is out
    /// already.
    fn remove_ad_group(&self, campaign: &str, group: &str) -> Result<(), PlatformError> {
        let path = format!("{}/ad_groups/{}", campaign_path(campaign)?, path_id(group)?);
        let response = self.write_ads(Write::Delete, &path, "")?;
        match response.status().as_u16() {
            200 | 204 | 404 => Ok(()),
            403 => Err(PlatformError::NotFound),
            _ => Err(failed(response, &path)),
        }
    }
}

#[derive(Clone, Copy)]
enum Write {
    Post,
    Put,
    Delete,
}

impl MercadoLivre {
    /// The campaign Mercado Livre answers to writing `fields` to `path`. An
    /// account it does not let manage campaigns by API answers 403 or 404,
    /// both [`PlatformError::NotFound`] for marketing.
    fn manage(
        &self,
        write: Write,
        path: &str,
        fields: &[String],
    ) -> Result<ChannelCampaign, PlatformError> {
        let body = format!("{{{}}}", fields.join(","));
        let response = self.write_ads(write, path, &body)?;
        match response.status().as_u16() {
            200 | 201 => read_json::<AdCampaignAnswer>(response).map(campaign),
            403 | 404 => Err(PlatformError::NotFound),
            _ => Err(failed(response, path)),
        }
    }

    /// Mercado Livre's answer to writing `body` to `path` in version 2,
    /// whatever its status; a delete sends no body.
    fn write_ads(
        &self,
        write: Write,
        path: &str,
        body: &str,
    ) -> Result<ureq::http::Response<ureq::Body>, PlatformError> {
        let url = format!("{}{path}", self.api);
        self.answered(|bearer| {
            let request = match write {
                Write::Post => self.agent.post(&url),
                Write::Put => self.agent.put(&url),
                Write::Delete => {
                    return self
                        .agent
                        .delete(&url)
                        .header("Accept", "application/json")
                        .header("Authorization", bearer)
                        .header(PRODUCT_ADS_VERSION.0, PRODUCT_ADS_VERSION.1)
                        .call();
                }
            };
            request
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .header("Authorization", bearer)
                .header(PRODUCT_ADS_VERSION.0, PRODUCT_ADS_VERSION.1)
                .send(body)
        })
    }

    /// An Ad Group with its listings: an `ITEM` group is its own, a family
    /// or catalog product lists them through its ads.
    fn ad_group(&self, answer: AdGroupAnswer) -> Result<ChannelAdGroup, PlatformError> {
        let id = whole_id(&answer.id);
        let kind = answer
            .ad_group_type
            .as_deref()
            .map_or(AdGroupKind::Other, AdGroupKind::from_code);
        let listings = match answer.ad_group_external_id {
            Some(listing) if kind == AdGroupKind::Item => vec![listing],
            _ => {
                let path = format!(
                    "/advertising/{SITE}/product_ads/ad_groups/{}/ads",
                    path_id(&id)?
                );
                let mut listings: Vec<String> = self
                    .search::<AdAnswer>(&path, &[])?
                    .into_iter()
                    .map(|ad| ad.item_id)
                    .collect();
                listings.sort();
                listings.dedup();
                listings
            }
        };
        Ok(ChannelAdGroup {
            kind,
            title: answer.title,
            listings,
            campaign: campaign_of(answer.campaign_id.as_ref()),
            status: answer
                .status
                .as_deref()
                .map_or(AdStatus::Other, AdStatus::from_code),
            id,
        })
    }

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
            let page: AdsSearchAnswer<T> = self.get_with(path, &asked, &[PRODUCT_ADS_VERSION])?;
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

/// An `ITEM` group is a single listing, its id the group's external id.
fn is_listing(group_type: Option<&str>) -> bool {
    group_type.is_some_and(|kind| AdGroupKind::from_code(kind) == AdGroupKind::Item)
}

/// The campaign an Ad Group is in; none without one, and `0` read as none
/// too, in case Mercado Livre answers that way for a group never placed.
fn campaign_of(id: Option<&Number>) -> Option<String> {
    id.map(whole_id).filter(|id| id != "0")
}

fn groups_path(account: &str) -> Result<String, PlatformError> {
    Ok(format!(
        "/advertising/{SITE}/advertisers/{}/product_ads/ad_groups/search",
        path_id(account)?
    ))
}

fn spent_or_sold(metrics: &AdMetrics) -> bool {
    !metrics.cost.amount().is_zero() || !metrics.attributed.amount().is_zero()
}

fn campaign_path(campaign: &str) -> Result<String, PlatformError> {
    Ok(format!(
        "/advertising/{SITE}/product_ads/campaigns/{}",
        path_id(campaign)?
    ))
}

/// The whole plan as Mercado Livre takes it: the budget in reais rounded
/// to cents, and the Target ROAS to the hundredth its answers carry.
fn plan_fields(plan: &CampaignPlan) -> Result<Vec<String>, PlatformError> {
    if plan.budget.currency() != CURRENCY {
        return Err(PlatformError::Refused(
            "Mercado Livre Brasil takes a campaign's budget in reais".into(),
        ));
    }
    let target = plan
        .target
        .ratio()
        .round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
        .normalize();
    Ok(vec![
        format!(r#""name":{}"#, text(&plan.name)),
        format!(r#""status":{}"#, text(status_code(plan.status)?)),
        format!(r#""budget":{}"#, number(plan.budget)),
        format!(r#""strategy":{}"#, text(plan.strategy.code())),
        format!(r#""roas_target":{target}"#),
    ])
}

/// A campaign is set to run or to be paused, nothing else.
fn status_code(status: CampaignStatus) -> Result<&'static str, PlatformError> {
    match status {
        CampaignStatus::Active => Ok("active"),
        CampaignStatus::Paused => Ok("paused"),
        CampaignStatus::Other => Err(PlatformError::Refused(
            "a campaign is set to active or paused".into(),
        )),
    }
}

/// A campaign as Mercado Livre reports it: one without a name gets one
/// from its id; a budget, strategy or Target ROAS it leaves out or the app
/// does not know is left unknown.
fn campaign(answer: AdCampaignAnswer) -> ChannelCampaign {
    let id = whole_id(&answer.id);
    let currency = match answer.currency_id.as_deref() {
        None => Some(CURRENCY),
        Some(code) => Currency::from_code(code),
    };
    ChannelCampaign {
        name: answer.name.unwrap_or_else(|| format!("Campanha {id}")),
        status: status(answer.status.as_deref()),
        budget: decimal(&answer.budget)
            .zip(currency)
            .map(|(amount, currency)| Money::new(amount, currency)),
        strategy: answer
            .strategy
            .as_deref()
            .and_then(CampaignStrategy::from_code),
        target: decimal(&answer.roas_target)
            .filter(|ratio| !ratio.is_zero())
            .and_then(Roas::new),
        id,
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

/// An Ad Group's or ad's metrics, none when Mercado Livre sent none; the
/// attributed sales are `total_amount`, or the direct and indirect ones
/// together without it.
fn metrics(answer: Option<&AdMetricsAnswer>) -> AdMetrics {
    let Some(answer) = answer else {
        return AdMetrics::zero(CURRENCY);
    };
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
