//! The seller's Reputation and the listings' Reviews for marketing's port
//! (ADR 0023): `seller_reputation` of `/users/me` and the pages of
//! `/reviews/item/{id}`. Read only: nothing goes back to the channel.

use mascate_kernel::{Percentage, PlatformError};
use mascate_marketing::{
    ChannelReputation, ChannelReview, LOW_RATING, ListingReviews, MetricReading, ReputationColor,
    ReputationSource,
};
use rust_decimal::Decimal;

use super::MercadoLivre;
use super::answers::{
    RateMetricAnswer, ReviewAnswer, ReviewsAnswer, UserReputationAnswer, decimal, time, whole_id,
};
use super::sales_channel::path_id;

/// Reviews per page of `/reviews/item/{id}`.
const REVIEWS_PAGE: u32 = 50;

impl ReputationSource for MercadoLivre {
    /// While Mercado Livre protects a new or recovering seller, the rates
    /// it counts are zero and the real ones come in `excluded`: those are
    /// the ones kept, so the owner sees where the metrics really stand.
    fn reputation(&self) -> Result<ChannelReputation, PlatformError> {
        let user: UserReputationAnswer = self.get("/users/me", &[])?;
        let Some(answer) = user.seller_reputation else {
            return Ok(no_reputation());
        };
        let metrics = answer.metrics.as_ref();
        let sales = metrics.and_then(|metrics| metrics.sales.as_ref());
        Ok(ChannelReputation {
            color: answer.level_id.as_deref().and_then(color),
            real_color: answer.real_level.as_deref().and_then(color),
            protected_until: answer.protection_end_date.as_deref().and_then(time),
            period_days: sales
                .and_then(|sales| sales.period.as_deref())
                .and_then(days),
            sales: sales.and_then(|sales| sales.completed).unwrap_or(0),
            transactions: answer
                .transactions
                .and_then(|transactions| transactions.completed)
                .unwrap_or(0),
            claims: reading(metrics.and_then(|metrics| metrics.claims.as_ref())),
            cancellations: reading(metrics.and_then(|metrics| metrics.cancellations.as_ref())),
            delayed_handling: reading(
                metrics.and_then(|metrics| metrics.delayed_handling_time.as_ref()),
            ),
        })
    }

    /// Reads pages only while low Reviews the first page counts are still
    /// missing; a listing nobody reviewed may answer 404.
    fn reviews(&self, listing: &str) -> Result<ListingReviews, PlatformError> {
        let path = format!("/reviews/item/{}", path_id(listing)?);
        let mut reviews = ListingReviews::default();
        let mut counted = false;
        let mut expected_low = None;
        let mut offset: u32 = 0;
        loop {
            let page: ReviewsAnswer = match self.get(
                &path,
                &[
                    ("limit", &REVIEWS_PAGE.to_string()),
                    ("offset", &offset.to_string()),
                ],
            ) {
                Err(PlatformError::NotFound) => return Ok(ListingReviews::default()),
                other => other?,
            };
            if let Some(levels) = page.rating_levels.as_ref().filter(|_| !counted) {
                reviews.stars = [
                    levels.one_star,
                    levels.two_star,
                    levels.three_star,
                    levels.four_star,
                    levels.five_star,
                ];
                expected_low = Some(reviews.stars[..usize::from(LOW_RATING)].iter().sum::<u32>());
                counted = true;
            }
            if expected_low == Some(0) || page.reviews.is_empty() {
                break;
            }
            offset += u32::try_from(page.reviews.len()).unwrap_or(REVIEWS_PAGE);
            for answer in page.reviews {
                let Some(review) = channel_review(answer) else {
                    continue;
                };
                if expected_low.is_none() {
                    reviews.stars[usize::from(review.rating - 1)] += 1;
                }
                if review.rating <= LOW_RATING {
                    reviews.low.push(review);
                }
            }
            let total = page.paging.and_then(|paging| paging.total).unwrap_or(0);
            let all_low = expected_low.is_some_and(|expected| {
                reviews.low.len() >= usize::try_from(expected).unwrap_or(0)
            });
            if all_low || offset >= total {
                break;
            }
        }
        Ok(reviews)
    }
}

fn no_reputation() -> ChannelReputation {
    ChannelReputation {
        color: None,
        real_color: None,
        protected_until: None,
        period_days: None,
        sales: 0,
        transactions: 0,
        claims: MetricReading::NONE,
        cancellations: MetricReading::NONE,
        delayed_handling: MetricReading::NONE,
    }
}

/// `5_green`, `4_light_green` or plain `green` alike.
fn color(level: &str) -> Option<ReputationColor> {
    let name = level.trim_start_matches(|c: char| c.is_ascii_digit() || c == '_');
    match name {
        "red" => Some(ReputationColor::Red),
        "orange" => Some(ReputationColor::Orange),
        "yellow" => Some(ReputationColor::Yellow),
        "light_green" => Some(ReputationColor::LightGreen),
        "green" => Some(ReputationColor::Green),
        _ => None,
    }
}

/// `60 days` as 60; `historic` counts every sale.
fn days(period: &str) -> Option<u32> {
    period.strip_suffix(" days")?.trim().parse().ok()
}

/// A metric's rate, a fraction, in percent, and its count; the real ones
/// when Mercado Livre keeps them apart.
fn reading(metric: Option<&RateMetricAnswer>) -> MetricReading {
    let Some(metric) = metric else {
        return MetricReading::NONE;
    };
    let real = metric.excluded.as_ref();
    let rate = real
        .and_then(|real| decimal(&real.real_rate))
        .or_else(|| decimal(&metric.rate))
        .unwrap_or_default();
    let count = real
        .and_then(|real| real.real_value)
        .or(metric.value)
        .unwrap_or(0);
    MetricReading {
        rate: Percentage::new(
            (rate * Decimal::ONE_HUNDRED).clamp(Decimal::ZERO, Decimal::ONE_HUNDRED),
        )
        .unwrap_or(Percentage::ZERO),
        count,
    }
}

/// A Review as marketing keeps it; `None` without a rating from 1 to 5 or
/// a date.
fn channel_review(answer: ReviewAnswer) -> Option<ChannelReview> {
    let rating = answer.rate.filter(|rate| (1..=5).contains(rate))?;
    Some(ChannelReview {
        id: whole_id(&answer.id),
        rating,
        title: answer.title.unwrap_or_default(),
        text: answer.content.unwrap_or_default(),
        at: time(&answer.date_created)?,
    })
}
