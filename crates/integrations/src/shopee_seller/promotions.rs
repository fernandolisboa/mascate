//! The owner's Shopee discounts, for commerce's port (ADR 0024, ADR 0033):
//! a discount is a campaign of the owner's whose listings each have a lower
//! price, every model of an item at the same one. `get_item_promotion`
//! tells which Promotions a listing is in, Shopee's own among them. Shopee
//! dates by the second and the app by Brasília's day; it has no coupon or
//! single-listing discount by this API.

use std::collections::BTreeMap;

use chrono::{DateTime, NaiveDate, TimeDelta};
use mascate_commerce::{
    ChannelOffer, ChannelPromotion, ChannelPromotions, OfferToJoin, PromotionKind, PromotionPlan,
    PromotionStatus,
};
use mascate_kernel::{Currency, Money, PlatformError, channel_day, channel_day_start};

use super::answers::{AddedDiscount, Discount, DiscountItems, DiscountList, ItemPromotions};
use super::sales_channel::item_id;
use super::{ShopeeSeller, is_numeric_id};
use crate::answers::decimal;

const DISCOUNT_LIST: &str = "/api/v2/discount/get_discount_list";
const DISCOUNT: &str = "/api/v2/discount/get_discount";
const ADD_DISCOUNT: &str = "/api/v2/discount/add_discount";
const UPDATE_DISCOUNT: &str = "/api/v2/discount/update_discount";
const DELETE_DISCOUNT: &str = "/api/v2/discount/delete_discount";
const END_DISCOUNT: &str = "/api/v2/discount/end_discount";
const ADD_DISCOUNT_ITEM: &str = "/api/v2/discount/add_discount_item";
const DELETE_DISCOUNT_ITEM: &str = "/api/v2/discount/delete_discount_item";
const ITEM_PROMOTION: &str = "/api/v2/product/get_item_promotion";

/// The most Shopee sends a page.
const PAGE_SIZE: &str = "100";

/// Pages read at most: past them the answer is not trusted to end.
const MAX_PAGES: usize = 100;

/// Shopee Brasil prices in reais; its promotion prices carry no currency.
const CURRENCY: Currency = Currency::Brl;

/// A discount starts at least an hour after it is created; the extra
/// minutes cover the trip there.
const START_AHEAD: TimeDelta = TimeDelta::minutes(65);

/// What Shopee answers for an item that is not in the discount.
const NOT_IN_DISCOUNT: &str = "discount.discount_item_not_exist";

impl ChannelPromotions for ShopeeSeller {
    /// The discounts about to start and running.
    fn promotions(&self) -> Result<Vec<ChannelPromotion>, PlatformError> {
        Ok(self.discounts()?.iter().filter_map(promotion).collect())
    }

    /// One offer per Promotion the item is in, at the lowest price among
    /// its models: the owner's discounts by their id, any other Shopee's.
    fn offers(&self, listing: &str) -> Result<Vec<ChannelOffer>, PlatformError> {
        let item = item_id(listing)?;
        let answer: ItemPromotions = match self.get(ITEM_PROMOTION, &[("item_id_list", item)]) {
            Err(PlatformError::NotFound) => return Ok(Vec::new()),
            other => other?,
        };
        let found: Vec<_> = answer
            .success_list
            .into_iter()
            .filter(|found| found.item_id == item)
            .flat_map(|found| found.promotion)
            .collect();
        if found.is_empty() {
            return Ok(Vec::new());
        }
        let discounts: BTreeMap<String, Discount> = self
            .discounts()?
            .into_iter()
            .map(|discount| (discount.discount_id.clone(), discount))
            .collect();
        let mut offers: Vec<ChannelOffer> = Vec::new();
        for promotion in found {
            let Some(status) = status(promotion.promotion_staging.as_deref()) else {
                continue;
            };
            let price = promotion
                .promotion_price_info
                .iter()
                .filter_map(|info| decimal(info.promotion_price.as_ref()?))
                .min()
                .map(|amount| Money::new(amount, CURRENCY));
            let id = Some(promotion.promotion_id.clone());
            if let Some(kept) = offers.iter_mut().find(|offer| offer.promotion == id) {
                kept.price = match (kept.price, price) {
                    (Some(kept), Some(price)) if price.amount() < kept.amount() => Some(price),
                    (kept, price) => kept.or(price),
                };
                continue;
            }
            let discount = discounts.get(&promotion.promotion_id);
            offers.push(ChannelOffer {
                listing: item.to_owned(),
                kind: if discount.is_some() {
                    PromotionKind::SellerCampaign
                } else {
                    PromotionKind::ChannelCampaign
                },
                promotion: id,
                name: discount
                    .and_then(|discount| discount.discount_name.clone())
                    .or(promotion.promotion_type)
                    .unwrap_or_default(),
                status,
                price,
                starts: promotion.start_time.and_then(first_day),
                ends: promotion.end_time.and_then(last_day),
            });
        }
        Ok(offers)
    }

    /// Sent once: Shopee would make a second discount of a repeated call.
    fn create(&self, plan: &PromotionPlan) -> Result<ChannelPromotion, PlatformError> {
        if plan.kind != PromotionKind::SellerCampaign {
            return Err(PlatformError::NotFound);
        }
        let (start, end) = self.period(plan.starts, plan.ends);
        let created: AddedDiscount = self.post_once_for(
            ADD_DISCOUNT,
            &format!(
                r#"{{"discount_name":{},"start_time":{start},"end_time":{end}}}"#,
                text(&plan.name)
            ),
        )?;
        Ok(ChannelPromotion {
            id: created.discount_id,
            kind: plan.kind,
            name: plan.name.clone(),
            starts: plan.starts,
            ends: plan.ends,
            status: PromotionStatus::Pending,
            coupon: None,
        })
    }

    /// The name, and each day only when it moved: Shopee refuses a running
    /// discount's start, even unchanged.
    fn change(&self, id: &str, plan: &PromotionPlan) -> Result<(), PlatformError> {
        let kept = self.discount(id, "1")?;
        let (start, end) = self.period(plan.starts, plan.ends);
        let mut fields = vec![
            format!(r#""discount_id":{}"#, kept.discount_id),
            format!(r#""discount_name":{}"#, text(&plan.name)),
        ];
        if kept.status.as_deref() == Some("upcoming")
            && first_day(kept.start_time) != Some(plan.starts)
        {
            fields.push(format!(r#""start_time":{start}"#));
        }
        if last_day(kept.end_time) != Some(plan.ends) {
            fields.push(format!(r#""end_time":{end}"#));
        }
        self.post::<serde_json::Value>(UPDATE_DISCOUNT, &format!("{{{}}}", fields.join(",")))?;
        Ok(())
    }

    /// Deletes a discount that has not started and ends a running one; one
    /// already over is left as it is.
    fn end(&self, id: &str, _kind: PromotionKind) -> Result<(), PlatformError> {
        let kept = self.discount(id, "1")?;
        let path = match kept.status.as_deref() {
            Some("upcoming") => DELETE_DISCOUNT,
            Some("ongoing") => END_DISCOUNT,
            _ => return Ok(()),
        };
        self.post::<serde_json::Value>(
            path,
            &format!(r#"{{"discount_id":{}}}"#, kept.discount_id),
        )?;
        Ok(())
    }

    /// Every model of the item that is on sale at the offer's price, with
    /// no limit per buyer. Sent once; commerce reads the item's Promotions
    /// before asking again.
    fn join(&self, listing: &str, offer: &OfferToJoin) -> Result<(), PlatformError> {
        let item = item_id(listing)?;
        let discount = discount_id(offer.promotion.as_deref())?;
        let price = offer
            .price
            .ok_or(PlatformError::NotFound)?
            .rounded()
            .amount();
        let models = self.models(item)?;
        let mut on_sale: Vec<&str> = models
            .model
            .iter()
            .filter(|model| model.model_status.as_deref() != Some("MODEL_UNAVAILABLE"))
            .map(|model| model.model_id.as_str())
            .collect();
        if on_sale.is_empty() {
            on_sale = models
                .model
                .iter()
                .map(|model| model.model_id.as_str())
                .collect();
        }
        let entry = if on_sale.is_empty() {
            format!(r#"{{"item_id":{item},"item_promotion_price":{price},"purchase_limit":0}}"#)
        } else {
            let prices: Vec<String> = on_sale
                .iter()
                .map(|model| format!(r#"{{"model_id":{model},"model_promotion_price":{price}}}"#))
                .collect();
            format!(
                r#"{{"item_id":{item},"model_list":[{}],"purchase_limit":0}}"#,
                prices.join(",")
            )
        };
        let answer: DiscountItems = self.post_once_for(
            ADD_DISCOUNT_ITEM,
            &format!(r#"{{"discount_id":{discount},"item_list":[{entry}]}}"#),
        )?;
        refused(answer)
    }

    /// Each of the item's models in the discount, as the discount lists
    /// them.
    fn leave(
        &self,
        listing: &str,
        kind: PromotionKind,
        promotion: Option<&str>,
    ) -> Result<(), PlatformError> {
        if kind != PromotionKind::SellerCampaign {
            return Err(PlatformError::NotFound);
        }
        let item = item_id(listing)?;
        let discount = discount_id(promotion)?;
        let models = self
            .discount_models(discount, item)?
            .ok_or(PlatformError::NotFound)?;
        let bodies: Vec<String> = if models.is_empty() {
            vec![format!(r#"{{"discount_id":{discount},"item_id":{item}}}"#)]
        } else {
            models
                .iter()
                .map(|model| {
                    format!(r#"{{"discount_id":{discount},"item_id":{item},"model_id":{model}}}"#)
                })
                .collect()
        };
        for body in bodies {
            let answer: DiscountItems = self.post(DELETE_DISCOUNT_ITEM, &body)?;
            refused(answer)?;
        }
        Ok(())
    }
}

impl ShopeeSeller {
    /// The shop's discounts about to start and running, page by page.
    fn discounts(&self) -> Result<Vec<Discount>, PlatformError> {
        let mut discounts = Vec::new();
        for status in ["upcoming", "ongoing"] {
            for page in 1..=MAX_PAGES {
                let answer: DiscountList = self.get(
                    DISCOUNT_LIST,
                    &[
                        ("discount_status", status),
                        ("page_no", &page.to_string()),
                        ("page_size", PAGE_SIZE),
                    ],
                )?;
                let last = !answer.more || answer.discount_list.is_empty();
                discounts.extend(answer.discount_list);
                if last {
                    break;
                }
            }
        }
        Ok(discounts)
    }

    /// The discount `id` with page `page` of its items.
    fn discount(&self, id: &str, page: &str) -> Result<Discount, PlatformError> {
        let id = discount_id(Some(id))?;
        self.get(
            DISCOUNT,
            &[
                ("discount_id", id),
                ("page_no", page),
                ("page_size", PAGE_SIZE),
            ],
        )
    }

    /// The models of `item` in the discount, none for an item without
    /// them; `None` when the item is not in it.
    fn discount_models(
        &self,
        discount: &str,
        item: &str,
    ) -> Result<Option<Vec<String>>, PlatformError> {
        for page in 1..=MAX_PAGES {
            let answer = self.discount(discount, &page.to_string())?;
            if let Some(found) = answer
                .item_list
                .into_iter()
                .find(|found| found.item_id == item)
            {
                return Ok(Some(
                    found
                        .model_list
                        .into_iter()
                        .map(|model| model.model_id)
                        .collect(),
                ));
            }
            if !answer.more {
                break;
            }
        }
        Ok(None)
    }

    /// The seconds a discount from `starts` to `ends` runs between: from
    /// the start of the first day, or a little over an hour from now when
    /// that is sooner, to the last second of the last day.
    fn period(&self, starts: NaiveDate, ends: NaiveDate) -> (i64, i64) {
        let soonest = self.timestamp() + START_AHEAD.num_seconds();
        let start = channel_day_start(starts).timestamp().max(soonest);
        let end = ends
            .succ_opt()
            .map_or(i64::MAX, |next| channel_day_start(next).timestamp() - 1);
        (start, end)
    }
}

/// A discount Shopee reports, as commerce keeps it; one already over is
/// not.
fn promotion(discount: &Discount) -> Option<ChannelPromotion> {
    Some(ChannelPromotion {
        id: discount.discount_id.clone(),
        kind: PromotionKind::SellerCampaign,
        name: discount.discount_name.clone().unwrap_or_default(),
        starts: first_day(discount.start_time)?,
        ends: last_day(discount.end_time)?,
        status: status(discount.status.as_deref())?,
        coupon: None,
    })
}

/// `upcoming` and `ongoing`, in a discount's status or a listing's
/// Promotion's staging.
fn status(status: Option<&str>) -> Option<PromotionStatus> {
    match status? {
        "upcoming" => Some(PromotionStatus::Pending),
        "ongoing" => Some(PromotionStatus::Started),
        _ => None,
    }
}

fn first_day(at: i64) -> Option<NaiveDate> {
    DateTime::from_timestamp(at, 0).map(channel_day)
}

/// The day of a Promotion's end: an end at midnight closes the day before.
fn last_day(at: i64) -> Option<NaiveDate> {
    first_day(at - 1)
}

/// A discount's id, digits only, so no other body is ever reached.
fn discount_id(id: Option<&str>) -> Result<&str, PlatformError> {
    id.filter(|id| is_numeric_id(id))
        .ok_or(PlatformError::NotFound)
}

/// The refusals Shopee names in a success answer, one item already out of
/// the discount aside.
fn refused(answer: DiscountItems) -> Result<(), PlatformError> {
    let reasons: Vec<String> = answer
        .error_list
        .into_iter()
        .filter(|error| error.fail_error.as_deref() != Some(NOT_IN_DISCOUNT))
        .map(|error| {
            error
                .fail_message
                .filter(|message| !message.trim().is_empty())
                .or(error.fail_error)
                .unwrap_or_else(|| "Shopee did not take the item".into())
        })
        .collect();
    if reasons.is_empty() {
        Ok(())
    } else {
        Err(PlatformError::Refused(reasons.join("; ")))
    }
}

/// `value` as a JSON string.
fn text(value: &str) -> String {
    serde_json::Value::from(value).to_string()
}
