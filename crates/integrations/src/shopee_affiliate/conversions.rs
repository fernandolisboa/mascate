//! Shopee Afiliados as a `CommissionSource` (#80, ADR 0030): the owner's
//! Conversions from `conversionReport`, one per item sold, and the Payout a
//! `validationId` from Shopee's billing page stands for, from
//! `validatedReport`. Both read through the Connection, signature and retry
//! of the offer search (ADR 0027).
//!
//! Both reports page with a `scrollId` that works once and only for 30
//! seconds, so pages follow each other without pauses; a query without one
//! waits 30 seconds after the last, as Shopee asks.

use std::ops::Range;
use std::sync::PoisonError;

use chrono::{DateTime, TimeDelta};
use mascate_affiliate::{
    AffiliatePlatform, CommissionSource, CommissionStatus, ReportedConversion, ReportedPayout,
    ReportedProgram,
};
use mascate_kernel::{Currency, Money, PlatformError, Timestamp};
use rust_decimal::Decimal;
use serde::Deserialize;
use serde_json::{Map, Value};

use super::{ShopeeAffiliates, brl, number, string};

/// Shopee lets the app read three months back; the shortest three months
/// in a row (February to April) have 89 days.
const HISTORY_DAYS: i64 = 89;

/// Delivery, the buyer's confirmation and the return period, then the
/// monthly validation where fraud is ruled out: past it, an order's status
/// no longer changes.
const SETTLE_DAYS: i64 = 60;

/// The most a report page brings.
const REPORT_PAGE_SIZE: u32 = 500;

/// How long a `scrollId` lasts, and the least time between two queries
/// without one.
pub(super) const SCROLL_LIFETIME: TimeDelta = TimeDelta::seconds(30);

/// An expired `scrollId` restarts the reading from the first page this many
/// times before the Sync gives up.
const RESTARTS: u32 = 2;

/// What a Conversion item reads: the item, its order and the conversion
/// above them.
const CONVERSION_FIELDS: &str = "purchaseTime clickTime conversionId totalCommission \
     netCommission utmContent device buyerType orders{orderId orderStatus shopType \
     items{itemId modelId itemName shopId shopName qty itemPrice actualAmount completeTime \
     itemTotalCommission refundAmount fraudStatus displayItemStatus itemNotes}}";

/// What a validated conversion reads: only what makes up the Payout, so a
/// field Shopee left out of this report never fails it.
const VALIDATED_FIELDS: &str =
    "conversionId totalCommission netCommission orders{orderId items{itemId modelId}}";

/// Shopee Afiliados is one Program for every Conversion and link.
pub(super) fn program() -> ReportedProgram {
    ReportedProgram {
        id: "shopee-affiliates".into(),
        name: "Shopee Afiliados".into(),
    }
}

impl CommissionSource for ShopeeAffiliates {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Shopee
    }

    fn history(&self) -> TimeDelta {
        TimeDelta::days(HISTORY_DAYS)
    }

    fn settle_time(&self) -> TimeDelta {
        TimeDelta::days(SETTLE_DAYS)
    }

    /// Shopee filters by the time of purchase. A window longer than it lets
    /// the app read is refused before asking.
    fn conversions(
        &self,
        during: Range<Timestamp>,
    ) -> Result<Vec<ReportedConversion>, PlatformError> {
        if during.end - during.start > self.history() {
            return Err(PlatformError::Refused(
                "a Shopee só deixa ler as conversões de 3 meses para trás".into(),
            ));
        }
        let arguments = format!(
            "purchaseTimeStart: {}, purchaseTimeEnd: {}",
            during.start.timestamp(),
            during.end.timestamp()
        );
        Ok(self
            .report("conversionReport", &arguments, CONVERSION_FIELDS)?
            .into_iter()
            .flat_map(conversions)
            .collect())
    }
}

impl ShopeeAffiliates {
    /// The Payout `validation_id` stands for, paid at `paid_at`: the net
    /// Commission of the conversions Shopee validated under it, covering
    /// each of their items. `NotFound` when Shopee knows none.
    pub fn validated_payout(
        &self,
        validation_id: &str,
        paid_at: Timestamp,
    ) -> Result<ReportedPayout, PlatformError> {
        let validation_id = validation_id.trim();
        if !number(validation_id) {
            return Err(PlatformError::Refused(
                "o validationId é o número que a Shopee mostra no faturamento".into(),
            ));
        }
        let validated = self.report(
            "validatedReport",
            &format!("validationId: {validation_id}"),
            VALIDATED_FIELDS,
        )?;
        if validated.is_empty() {
            return Err(PlatformError::NotFound);
        }
        let mut amount = Decimal::ZERO;
        let mut covers = Vec::new();
        for conversion in &validated {
            // What Shopee passes on is the net, after a linked MCN's fee.
            let net = ["netCommission", "totalCommission"]
                .into_iter()
                .find_map(|field| conversion.get(field).and_then(Value::as_str).and_then(brl))
                .ok_or_else(|| {
                    PlatformError::Failed(
                        "a Shopee mandou uma conversão validada sem comissão".into(),
                    )
                })?;
            amount += net.amount();
            covers.extend(items(conversion).map(|item| item.key));
        }
        Ok(ReportedPayout {
            key: validation_id.to_owned(),
            paid_at,
            amount: Money::new(amount, Currency::Brl),
            covers,
            raw: Value::Array(validated).to_string(),
        })
    }

    /// Every node `operation` reports for `arguments`, page after page. An
    /// expired `scrollId` starts over from the first page.
    fn report(
        &self,
        operation: &str,
        arguments: &str,
        fields: &str,
    ) -> Result<Vec<Value>, PlatformError> {
        let mut restarts = 0;
        'reading: loop {
            let mut nodes = Vec::new();
            let mut scroll: Option<String> = None;
            loop {
                let mut query = format!("{{{operation}({arguments}, limit: {REPORT_PAGE_SIZE}");
                if let Some(scroll) = &scroll {
                    query.push_str(&format!(", scrollId: {}", string(scroll)));
                }
                query.push_str(&format!(
                    "){{nodes{{{fields}}}pageInfo{{hasNextPage scrollId}}}}}}"
                ));
                let page: ReportPage = match self.report_page(operation, &query, scroll.is_none()) {
                    Ok(page) => page,
                    // Shopee answers a `scrollId` past its 30 seconds, or used
                    // already, as a wrong parameter.
                    Err(PlatformError::Refused(_)) if scroll.is_some() && restarts < RESTARTS => {
                        restarts += 1;
                        continue 'reading;
                    }
                    Err(error) => return Err(error),
                };
                let more = page.page_info.has_next_page && !page.nodes.is_empty();
                nodes.extend(page.nodes);
                match page.page_info.scroll_id.filter(|next| !next.is_empty()) {
                    Some(next) if more => scroll = Some(next),
                    _ => return Ok(nodes),
                }
            }
        }
    }

    fn report_page(
        &self,
        operation: &str,
        query: &str,
        first: bool,
    ) -> Result<ReportPage, PlatformError> {
        if first {
            self.query_after(operation, query, || self.wait_since_last_first_page())
        } else {
            self.query(operation, query)
        }
    }

    /// Waits out what is left of the 30 seconds Shopee asks between two
    /// queries without a `scrollId`. Holding the lock keeps a second reader
    /// in line behind the first.
    fn wait_since_last_first_page(&self) {
        let mut last = self
            .last_first_page
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(at) = *last {
            let since = self.clock.now() - at;
            if let Ok(left) = (SCROLL_LIFETIME - since).to_std()
                && !left.is_zero()
            {
                self.pause.pause(left);
            }
        }
        *last = Some(self.clock.now());
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportPage {
    #[serde(default)]
    nodes: Vec<Value>,
    page_info: ReportPageInfo,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReportPageInfo {
    #[serde(default)]
    has_next_page: bool,
    scroll_id: Option<String>,
}

/// One item of a conversion, with the order it is in.
struct Item<'a> {
    key: String,
    order: &'a Map<String, Value>,
    item: &'a Map<String, Value>,
}

/// Each item of `conversion` with its natural key: conversion, order, item
/// and variation. An item without its ids is left out.
fn items(conversion: &Value) -> impl Iterator<Item = Item<'_>> {
    let conversion_id = id(conversion.get("conversionId"));
    objects(conversion.get("orders")).flat_map(move |order| {
        let order_id = id(order.get("orderId"));
        let conversion_id = conversion_id.clone();
        objects(order.get("items")).filter_map(move |item| {
            let item_id = id(item.get("itemId"))?;
            // A product without variations has none, or 0.
            let model_id = id(item.get("modelId")).unwrap_or_else(|| "0".into());
            Some(Item {
                key: format!(
                    "{}:{}:{item_id}:{model_id}",
                    conversion_id.as_deref()?,
                    order_id.as_deref()?
                ),
                order,
                item,
            })
        })
    })
}

/// The marking of the link's Promotion Channel in a report's `utmContent`:
/// the first subId (ADR 0031, decision 4). Shopee joins the five subIds
/// with dashes, so a marking for Shopee has none.
fn channel_tag(utm_content: &str) -> Option<String> {
    let first = utm_content.trim().split('-').next()?.trim();
    (!first.is_empty()).then(|| first.to_owned())
}

/// The Conversions in one node of the report, one per item. An item whose
/// Commission or purchase time cannot be read is left out.
fn conversions(conversion: Value) -> Vec<ReportedConversion> {
    let Some(sold_at) = time(conversion.get("purchaseTime")) else {
        return Vec::new();
    };
    let tracking = conversion
        .get("utmContent")
        .and_then(Value::as_str)
        .and_then(channel_tag);
    let mut above = conversion.as_object().cloned().unwrap_or_default();
    above.remove("orders");
    items(&conversion)
        .filter_map(|Item { key, order, item }| {
            let text = |field: &str| item.get(field).and_then(Value::as_str);
            let commission = brl(text("itemTotalCommission")?)?;
            let order_status = order
                .get("orderStatus")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_owned();
            let fraud_status = text("fraudStatus").map(str::trim).unwrap_or_default();
            let refunded = text("refundAmount").is_some_and(|refund| {
                brl(refund).is_some_and(|refund| refund.amount() > Decimal::ZERO)
            });
            let status = status(&order_status, fraud_status, refunded);
            let product = text("itemName")
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("item {}", id(item.get("itemId")).unwrap_or_default()));
            let mut order = order.clone();
            order.remove("items");
            Some(ReportedConversion {
                key,
                program: program(),
                product,
                sold_at,
                commission,
                status,
                platform_status: if fraud_status.is_empty() {
                    order_status.clone()
                } else {
                    format!("{order_status}/{fraud_status}")
                },
                // Completion is when it was approved; Shopee dates no
                // reversal.
                status_changed_at: (status == CommissionStatus::Approved)
                    .then(|| time(item.get("completeTime")))
                    .flatten(),
                tracking: tracking.clone(),
                raw: serde_json::json!({
                    "conversion": above,
                    "order": order,
                    "item": item,
                })
                .to_string(),
            })
        })
        .collect()
}

/// The Commission's status for an item in an order with `order_status`.
/// Fraud reverses it whatever the order says, and so does a refund once
/// the order completed. A status Shopee adds later stays pending until the
/// app learns it; the raw status is kept either way.
fn status(order_status: &str, fraud_status: &str, refunded: bool) -> CommissionStatus {
    if fraud_status == "FRAUD" {
        return CommissionStatus::Reversed;
    }
    match order_status {
        "COMPLETED" if refunded => CommissionStatus::Reversed,
        "COMPLETED" => CommissionStatus::Approved,
        "CANCELLED" => CommissionStatus::Cancelled,
        _ => CommissionStatus::Pending,
    }
}

/// The objects in a list field, such as a conversion's `orders`.
fn objects(list: Option<&Value>) -> impl Iterator<Item = &Map<String, Value>> {
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
}

/// Shopee's Int64 ids, as numbers or as text.
fn id(value: Option<&Value>) -> Option<String> {
    let id = match value? {
        Value::Number(number) => number.as_u64()?.to_string(),
        Value::String(text) => text.trim().to_owned(),
        _ => return None,
    };
    (!id.is_empty()).then_some(id)
}

/// Shopee's times, in seconds since 1970 UTC; 0 means none yet.
fn time(value: Option<&Value>) -> Option<Timestamp> {
    let seconds = value?.as_i64().filter(|seconds| *seconds > 0)?;
    DateTime::from_timestamp(seconds, 0)
}
