//! Listing Quality for marketing's port (ADR 0021, ADR 0033 decision 8):
//! the shop's listings with issues that may cost penalty points, read once
//! for the whole shop, and Shopee's rating of the shop with each metric
//! against its target (`account_health`). Shopee gives no score of a
//! listing and no visits by this API.

use std::collections::BTreeMap;

use mascate_kernel::{PlatformError, SalesChannelKind};
use mascate_marketing::{
    ActionKind, ChannelPerformance, ChannelQuality, Comparator, MetricGroup, MetricTarget,
    MetricUnit, PerformanceMetric, PerformanceRating, QualityAction, QualitySource,
};

use super::ShopeeSeller;
use super::answers::{ListingsWithIssues, Metric, ShopPerformance};
use crate::answers::decimal;

const LISTINGS_WITH_ISSUES: &str = "/api/v2/account_health/get_listings_with_issues";
const SHOP_PERFORMANCE: &str = "/api/v2/account_health/get_shop_performance";

/// The most Shopee sends a page.
const PAGE_SIZE: usize = 100;

/// Pages read at most, 10,000 listings: past them the answer is not
/// trusted to end.
const MAX_PAGES: usize = 100;

const FIX_LABEL: &str = "Corrigir na Central do Vendedor";

impl QualitySource for ShopeeSeller {
    fn rated_channel(&self) -> SalesChannelKind {
        SalesChannelKind::Shopee
    }

    fn quality(&self, listing: &str) -> Result<Option<ChannelQuality>, PlatformError> {
        Ok(self
            .qualities(&[listing.to_owned()])?
            .into_iter()
            .next()
            .flatten())
    }

    /// Every listing Shopee reports with issues, read once; the others have
    /// nothing pending.
    fn qualities(&self, listings: &[String]) -> Result<Vec<Option<ChannelQuality>>, PlatformError> {
        if listings.is_empty() {
            return Ok(Vec::new());
        }
        let mut issues: BTreeMap<String, Vec<QualityAction>> = BTreeMap::new();
        let page_size = PAGE_SIZE.to_string();
        let mut read = 0;
        for page in 1..=MAX_PAGES {
            let answer: ListingsWithIssues = self.get(
                LISTINGS_WITH_ISSUES,
                &[("page_no", &page.to_string()), ("page_size", &page_size)],
            )?;
            read += answer.listing_list.len();
            let last = answer.listing_list.is_empty() || read >= answer.total_count;
            for listing in answer.listing_list {
                let found = issues.entry(listing.item_id.clone()).or_default();
                let action = action(listing.reason, &listing.item_id);
                if !found.iter().any(|kept| kept.key == action.key) {
                    found.push(action);
                }
            }
            if last {
                break;
            }
        }
        Ok(listings
            .iter()
            .map(|listing| {
                Some(ChannelQuality {
                    score: None,
                    pending: issues.get(listing).cloned().unwrap_or_default(),
                })
            })
            .collect())
    }

    fn visits(&self, _: &str, _: u32) -> Result<Option<u32>, PlatformError> {
        Ok(None)
    }

    fn performance(&self) -> Result<Option<ChannelPerformance>, PlatformError> {
        let answer: ShopPerformance = self.get(SHOP_PERFORMANCE, &[])?;
        Ok(Some(ChannelPerformance {
            rating: answer
                .overall_performance
                .and_then(|overall| overall.rating)
                .and_then(rating),
            metrics: answer.metric_list.iter().filter_map(metric).collect(),
        }))
    }
}

/// An issue as marketing keeps it, by Shopee's reason: what puts the
/// listing at risk of penalty points is a problem, an improvement is a
/// suggestion.
fn action(reason: i64, item: &str) -> QualityAction {
    let (kind, text) = match reason {
        1 => (
            ActionKind::Problem,
            "A Shopee considera o produto proibido. Revise o anúncio ou encerre-o para evitar \
             pontos de penalidade."
                .to_owned(),
        ),
        2 => (
            ActionKind::Problem,
            "A Shopee suspeita de produto falsificado. Revise o anúncio e a marca informada."
                .to_owned(),
        ),
        3 => (
            ActionKind::Problem,
            "A Shopee considera o anúncio spam: repetido, com palavras-chave fora do produto ou \
             informação enganosa."
                .to_owned(),
        ),
        4 => (
            ActionKind::Problem,
            "Uma imagem do anúncio é inadequada. Troque-a por uma que siga as regras da Shopee."
                .to_owned(),
        ),
        5 => (
            ActionKind::Problem,
            "Faltam informações no anúncio. Complete os dados que a Shopee pede.".to_owned(),
        ),
        6 => (
            ActionKind::Opportunity,
            "O anúncio precisa das melhorias que a Shopee Mall pede.".to_owned(),
        ),
        7 => (
            ActionKind::Opportunity,
            "A Shopee sugere melhorias neste anúncio.".to_owned(),
        ),
        other => (
            ActionKind::Problem,
            format!("A Shopee apontou um problema neste anúncio (motivo {other})."),
        ),
    };
    QualityAction {
        key: reason.to_string(),
        kind,
        text,
        label: FIX_LABEL.into(),
        link: Some(format!(
            "https://seller.shopee.com.br/portal/product/{item}"
        )),
    }
}

fn rating(code: i64) -> Option<PerformanceRating> {
    match code {
        1 => Some(PerformanceRating::Poor),
        2 => Some(PerformanceRating::ImprovementNeeded),
        3 => Some(PerformanceRating::Good),
        4 => Some(PerformanceRating::Excellent),
        _ => None,
    }
}

/// One metric as marketing keeps it; a group of metrics, or one of a kind
/// this app does not know, is left out.
fn metric(metric: &Metric) -> Option<PerformanceMetric> {
    if metric.metric_id < 0 {
        return None;
    }
    let group = match metric.metric_type {
        1 => MetricGroup::Fulfillment,
        2 => MetricGroup::Listings,
        3 => MetricGroup::CustomerService,
        _ => return None,
    };
    let unit = match metric.unit {
        Some(2) => MetricUnit::Percent,
        Some(3) => MetricUnit::Seconds,
        Some(4) => MetricUnit::Days,
        Some(5) => MetricUnit::Hours,
        _ => MetricUnit::Number,
    };
    let target = metric.target.as_ref().and_then(|target| {
        Some(MetricTarget {
            value: decimal(target.value.as_ref()?)?,
            comparator: Comparator::from_sign(target.comparator.as_deref()?)?,
        })
    });
    Some(PerformanceMetric {
        key: metric.metric_id.to_string(),
        group,
        name: metric
            .metric_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| format!("Métrica {}", metric.metric_id)),
        current: metric.current_period.as_ref().and_then(decimal),
        previous: metric.last_period.as_ref().and_then(decimal),
        unit,
        target,
    })
}
