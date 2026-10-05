//! The owner's Promotions, for commerce's port (ADR 0024): the seller's
//! campaigns and coupons from `/seller-promotions/users/{id}`, each
//! coupon's terms, each listing's Promotions from
//! `/seller-promotions/items/{id}`, and creating, changing and ending them
//! on the owner's confirmed click. Every call names `app_version=v2`.

use chrono::{NaiveDate, NaiveDateTime};
use mascate_commerce::{
    ChannelOffer, ChannelPromotion, ChannelPromotions, CouponDiscount, CouponTerms, OfferToJoin,
    PromotionKind, PromotionPlan, PromotionStatus,
};
use mascate_kernel::{Currency, Money, Percentage, PlatformError, channel_day};
use serde_json::Value;

use super::MercadoLivre;
use super::answers::{
    ItemPromotionAnswer, SellerPromotionAnswer, SellerPromotionsAnswer, UserAnswer, decimal, time,
};
use super::sales_channel::path_id;

const APP_VERSION: (&str, &str) = ("app_version", "v2");

/// Promotions per page of the seller's list, when it has more than one.
const PROMOTIONS_PAGE: u32 = 50;

/// Mercado Livre Brasil prices in reais; the promotions API sends no
/// currency.
const CURRENCY: Currency = Currency::Brl;

/// Mercado Livre's name for a kind of Promotion.
fn type_id(kind: PromotionKind) -> Option<&'static str> {
    match kind {
        PromotionKind::PriceDiscount => Some("PRICE_DISCOUNT"),
        PromotionKind::SellerCampaign => Some("SELLER_CAMPAIGN"),
        PromotionKind::Coupon => Some("SELLER_COUPON_CAMPAIGN"),
        PromotionKind::ChannelCampaign => None,
    }
}

/// A kind the app knows; any other type (`DEAL`, `LIGHTNING`,
/// `MARKETPLACE_CAMPAIGN`...) is one of the channel's own.
fn kind_of(type_id: &str) -> PromotionKind {
    match type_id {
        "PRICE_DISCOUNT" => PromotionKind::PriceDiscount,
        "SELLER_CAMPAIGN" => PromotionKind::SellerCampaign,
        "SELLER_COUPON_CAMPAIGN" => PromotionKind::Coupon,
        _ => PromotionKind::ChannelCampaign,
    }
}

/// `pending` and `started`; a `candidate` is only invited, and a `finished`
/// one is over.
fn status(status: Option<&str>) -> Option<PromotionStatus> {
    match status? {
        "pending" => Some(PromotionStatus::Pending),
        "started" => Some(PromotionStatus::Started),
        _ => None,
    }
}

/// The channel's day of a date it sends: a time with its offset, such as
/// `2023-04-20T03:00:00Z`, or a local one, such as `2023-07-19T23:59:59`.
fn day(text: &str) -> Option<NaiveDate> {
    time(text)
        .map(channel_day)
        .or_else(|| {
            NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .map(|at| at.date())
        })
        .or_else(|| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok())
}

/// A day as the channel takes it: the first day starts at midnight and the
/// last one ends at 23:59:59 on its own.
fn day_text(day: NaiveDate) -> String {
    day.format("%Y-%m-%dT00:00:00").to_string()
}

/// A Promotion's id, such as `C-MLB360923`, checked before it goes into a
/// path or query: letters, digits and dashes only.
fn promotion_id(id: &str) -> Result<&str, PlatformError> {
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        Ok(id)
    } else {
        Err(PlatformError::NotFound)
    }
}

/// An amount in reais as JSON, rounded to cents as the channel takes it.
fn number(money: Money) -> String {
    money.rounded().amount().to_string()
}

fn text(value: &str) -> String {
    Value::String(value.to_owned()).to_string()
}

fn money(number: &Option<serde_json::Number>) -> Option<Money> {
    decimal(number)
        .filter(|amount| !amount.is_zero())
        .map(|amount| Money::new(amount, CURRENCY))
}

/// A coupon's terms; `None` when the answer leaves out what the app needs.
fn coupon_terms(answer: &SellerPromotionAnswer) -> Option<CouponTerms> {
    let discount = match answer.sub_type.as_deref()? {
        "FIXED_AMOUNT" => CouponDiscount::Amount(money(&answer.fixed_amount)?),
        "FIXED_PERCENTAGE" => {
            CouponDiscount::Percent(Percentage::new(decimal(&answer.fixed_percentage)?).ok()?)
        }
        _ => return None,
    };
    Some(CouponTerms {
        discount,
        minimum_purchase: money(&answer.min_purchase_amount)?,
        maximum_discount: money(&answer.max_purchase_amount),
        budget: money(&answer.budget)?,
    })
}

impl MercadoLivre {
    /// The campaign or coupon in `answer`; `None` for another type, one
    /// finished or only invited, or one without its days.
    fn seller_promotion(
        &self,
        answer: SellerPromotionAnswer,
    ) -> Result<Option<ChannelPromotion>, PlatformError> {
        let kind = kind_of(&answer.kind);
        if !matches!(kind, PromotionKind::SellerCampaign | PromotionKind::Coupon) {
            return Ok(None);
        }
        let (Some(status), Some(starts), Some(ends)) = (
            status(answer.status.as_deref()),
            answer.start_date.as_deref().and_then(day),
            answer.finish_date.as_deref().and_then(day),
        ) else {
            return Ok(None);
        };
        let coupon = match kind {
            PromotionKind::Coupon => match coupon_terms(&answer) {
                Some(terms) => Some(terms),
                None => self.coupon(&answer.id)?,
            },
            _ => None,
        };
        Ok(Some(ChannelPromotion {
            id: answer.id,
            kind,
            name: answer.name.unwrap_or_default(),
            starts,
            ends,
            status,
            coupon,
        }))
    }

    /// The terms of the coupon `id`, read alone.
    fn coupon(&self, id: &str) -> Result<Option<CouponTerms>, PlatformError> {
        let answer: SellerPromotionAnswer = self.get(
            &format!("/seller-promotions/promotions/{}", promotion_id(id)?),
            &[("promotion_type", "SELLER_COUPON_CAMPAIGN"), APP_VERSION],
        )?;
        Ok(coupon_terms(&answer))
    }
}

impl ChannelPromotions for MercadoLivre {
    fn promotions(&self) -> Result<Vec<ChannelPromotion>, PlatformError> {
        let user: UserAnswer = self.get("/users/me", &[])?;
        let path = format!("/seller-promotions/users/{}", user.id);
        let mut promotions = Vec::new();
        let mut offset: u32 = 0;
        loop {
            let page: SellerPromotionsAnswer = if offset == 0 {
                self.get(&path, &[APP_VERSION])?
            } else {
                self.get(
                    &path,
                    &[
                        APP_VERSION,
                        ("offset", &offset.to_string()),
                        ("limit", &PROMOTIONS_PAGE.to_string()),
                    ],
                )?
            };
            let read = u32::try_from(page.results.len()).unwrap_or(u32::MAX);
            for answer in page.results {
                promotions.extend(self.seller_promotion(answer)?);
            }
            offset = offset.saturating_add(read);
            let total = page.paging.map_or(0, |paging| paging.total);
            if read == 0 || offset >= total {
                break;
            }
        }
        Ok(promotions)
    }

    /// A listing in no Promotion may answer 404.
    fn offers(&self, listing: &str) -> Result<Vec<ChannelOffer>, PlatformError> {
        let answers: Vec<ItemPromotionAnswer> = match self.get(
            &format!("/seller-promotions/items/{}", path_id(listing)?),
            &[APP_VERSION],
        ) {
            Err(PlatformError::NotFound) => return Ok(Vec::new()),
            other => other?,
        };
        Ok(answers
            .into_iter()
            .filter_map(|answer| {
                let kind = kind_of(&answer.kind);
                Some(ChannelOffer {
                    listing: listing.to_owned(),
                    kind,
                    promotion: answer
                        .id
                        .filter(|_| kind != PromotionKind::PriceDiscount)
                        .filter(|id| !id.is_empty()),
                    name: answer.name.unwrap_or_default(),
                    status: status(answer.status.as_deref())?,
                    price: money(&answer.price).filter(|_| kind != PromotionKind::Coupon),
                    starts: answer.start_date.as_deref().and_then(day),
                    ends: answer.finish_date.as_deref().and_then(day),
                })
            })
            .collect())
    }

    fn create(&self, plan: &PromotionPlan) -> Result<ChannelPromotion, PlatformError> {
        let promotion_type = type_id(plan.kind).ok_or(PlatformError::NotFound)?;
        let mut fields = vec![
            format!(r#""promotion_type":{}"#, text(promotion_type)),
            format!(r#""name":{}"#, text(&plan.name)),
            format!(r#""start_date":{}"#, text(&day_text(plan.starts))),
            format!(r#""finish_date":{}"#, text(&day_text(plan.ends))),
        ];
        match (plan.kind, plan.coupon) {
            (PromotionKind::SellerCampaign, _) => {
                fields.push(r#""sub_type":"FLEXIBLE_PERCENTAGE""#.into());
            }
            (PromotionKind::Coupon, Some(terms)) => {
                match terms.discount {
                    CouponDiscount::Amount(amount) => {
                        fields.push(r#""sub_type":"FIXED_AMOUNT""#.into());
                        fields.push(format!(r#""fixed_amount":{}"#, number(amount)));
                    }
                    CouponDiscount::Percent(rate) => {
                        fields.push(r#""sub_type":"FIXED_PERCENTAGE""#.into());
                        fields.push(format!(
                            r#""fixed_percentage":{}"#,
                            rate.percent().normalize()
                        ));
                    }
                }
                fields.push(format!(
                    r#""min_purchase_amount":{}"#,
                    number(terms.minimum_purchase)
                ));
                if let Some(cap) = terms.maximum_discount {
                    fields.push(format!(r#""max_purchase_amount":{}"#, number(cap)));
                }
                fields.push(format!(r#""budget":{}"#, number(terms.budget)));
            }
            _ => return Err(PlatformError::NotFound),
        }
        let created: SellerPromotionAnswer = self.post(
            "/seller-promotions/promotions?app_version=v2",
            &format!("{{{}}}", fields.join(",")),
        )?;
        let id = created.id.clone();
        let status = status(created.status.as_deref()).unwrap_or(PromotionStatus::Pending);
        Ok(ChannelPromotion {
            starts: created
                .start_date
                .as_deref()
                .and_then(day)
                .unwrap_or(plan.starts),
            ends: created
                .finish_date
                .as_deref()
                .and_then(day)
                .unwrap_or(plan.ends),
            coupon: coupon_terms(&created).or(plan.coupon),
            name: created.name.unwrap_or_else(|| plan.name.clone()),
            kind: plan.kind,
            status,
            id,
        })
    }

    fn change(&self, id: &str, plan: &PromotionPlan) -> Result<(), PlatformError> {
        let promotion_type = type_id(plan.kind).ok_or(PlatformError::NotFound)?;
        let body = format!(
            r#"{{"promotion_type":{},"name":{},"start_date":{},"finish_date":{}}}"#,
            text(promotion_type),
            text(&plan.name),
            text(&day_text(plan.starts)),
            text(&day_text(plan.ends)),
        );
        self.put(
            &format!(
                "/seller-promotions/promotions/{}?app_version=v2",
                promotion_id(id)?
            ),
            &body,
        )
    }

    fn end(&self, id: &str, kind: PromotionKind) -> Result<(), PlatformError> {
        let promotion_type = type_id(kind).ok_or(PlatformError::NotFound)?;
        self.delete(
            &format!("/seller-promotions/promotions/{}", promotion_id(id)?),
            &[("promotion_type", promotion_type), APP_VERSION],
        )
    }

    fn join(&self, listing: &str, offer: &OfferToJoin) -> Result<(), PlatformError> {
        let promotion_type = type_id(offer.kind).ok_or(PlatformError::NotFound)?;
        let mut fields = vec![format!(r#""promotion_type":{}"#, text(promotion_type))];
        if let Some(promotion) = &offer.promotion {
            fields.push(format!(
                r#""promotion_id":{}"#,
                text(promotion_id(promotion)?)
            ));
        }
        if let Some(price) = offer.price {
            fields.push(format!(r#""deal_price":{}"#, number(price)));
        }
        if let Some((starts, ends)) = offer.days {
            fields.push(format!(r#""start_date":{}"#, text(&day_text(starts))));
            fields.push(format!(r#""finish_date":{}"#, text(&day_text(ends))));
        }
        self.post::<Value>(
            &format!(
                "/seller-promotions/items/{}?app_version=v2",
                path_id(listing)?
            ),
            &format!("{{{}}}", fields.join(",")),
        )?;
        Ok(())
    }

    fn leave(
        &self,
        listing: &str,
        kind: PromotionKind,
        promotion: Option<&str>,
    ) -> Result<(), PlatformError> {
        let promotion_type = type_id(kind).ok_or(PlatformError::NotFound)?;
        let mut query = vec![("promotion_type", promotion_type)];
        if let Some(promotion) = promotion {
            query.push(("promotion_id", promotion_id(promotion)?));
        }
        query.push(APP_VERSION);
        self.delete(
            &format!("/seller-promotions/items/{}", path_id(listing)?),
            &query,
        )
    }
}
