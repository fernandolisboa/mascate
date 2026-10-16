//! The owner's Amazon listings, for commerce's Sales Channel port: read
//! them, the fee Amazon estimates for each, a new price once the owner
//! approves it, and the stock that follows the app's. Amazon has no
//! variation listings of its own: each child SKU is an offer with its own
//! price and stock, so each is a Listing, and the parent, which sells
//! nothing, is left out (ADR 0014).

use mascate_commerce::{
    ChannelCategory, ChannelListing, ChannelStock, FeeBasis, ListingStatus, OnSalesChannel,
    SaleFee, SalesChannel,
};
use mascate_kernel::{Currency, ListingType, Money, Percentage, PlatformError, SalesChannelKind};
use rust_decimal::Decimal;

use super::answers::{FeesAnswer, Item, ItemSearch, ProductTypeDefinition, Submission, Summary};
use super::{Amazon, MARKETPLACE, path_segment};

/// The most listings a search page or a search by SKU takes.
const PAGE_SIZE: usize = 20;

/// The listings a search pages through at most; past them, the next
/// search starts where the last one stopped.
const SEARCH_CAP: u32 = 1000;

/// The data each listing comes with.
const INCLUDED: &str = "summaries,offers,fulfillmentAvailability,relationships";

/// The seller's own fulfillment; anything else is Amazon's, left out of
/// this epic (ADR 0033).
const OWN_FULFILLMENT: &str = "DEFAULT";

/// Amazon's referral fee for "Demais categorias", the highest of its table,
/// 15% of the sale and at least R$ 2,00 a unit (venda.amazon.com.br/precos,
/// read on 2026-10-16): the fee where Amazon gives no estimate, so a
/// margin is never shown higher than it is. The Professional plan has no
/// fee per unit.
const TABLE_RATE: Decimal = Decimal::from_parts(15, 0, 0, false, 0);
const TABLE_MINIMUM: Decimal = Decimal::from_parts(2, 0, 0, false, 0);

impl OnSalesChannel for Amazon {
    fn sales_channel(&self) -> SalesChannelKind {
        SalesChannelKind::Amazon
    }
}

impl SalesChannel for Amazon {
    /// Pages through the seller's listings, oldest first; a search pages
    /// through at most 1,000, so the next one starts at the last created.
    fn listing_ids(&self) -> Result<Vec<String>, PlatformError> {
        let path = self.items_path()?;
        let page_size = PAGE_SIZE.to_string();
        let mut ids: Vec<String> = Vec::new();
        let mut created_after: Option<String> = None;
        loop {
            let mut read = 0;
            let mut total = 0;
            let mut last_created: Option<String> = None;
            let mut page_token: Option<String> = None;
            loop {
                let mut query = vec![
                    ("marketplaceIds", MARKETPLACE),
                    ("includedData", "summaries"),
                    ("sortBy", "createdDate"),
                    ("sortOrder", "ASC"),
                    ("pageSize", page_size.as_str()),
                ];
                if let Some(after) = &created_after {
                    query.push(("createdAfter", after));
                }
                if let Some(token) = &page_token {
                    query.push(("pageToken", token));
                }
                let page: ItemSearch = self.get(&path, &query)?;
                total = total.max(page.number_of_results);
                read += page.items.len();
                for item in page.items {
                    last_created = summary(&item)
                        .and_then(|summary| summary.created_date.clone())
                        .or(last_created);
                    if !ids.contains(&item.sku) {
                        ids.push(item.sku);
                    }
                }
                match page.pagination.and_then(|pagination| pagination.next_token) {
                    // A token that repeats would read the same page forever.
                    Some(next) if page_token.as_ref() != Some(&next) => page_token = Some(next),
                    _ => break,
                }
            }
            let capped = read as u32 >= SEARCH_CAP && (read as u32) < total;
            match last_created {
                Some(last) if capped && created_after.as_ref() != Some(&last) => {
                    created_after = Some(last);
                }
                _ => return Ok(ids),
            }
        }
    }

    /// Searched by SKU, 20 at a time; a SKU Amazon no longer has is not in
    /// the answer.
    fn listings(&self, ids: &[String]) -> Result<Vec<ChannelListing>, PlatformError> {
        let path = self.items_path()?;
        let page_size = PAGE_SIZE.to_string();
        let mut listings = Vec::new();
        for chunk in ids.chunks(PAGE_SIZE) {
            let identifiers = chunk.join(",");
            let page: ItemSearch = self.get(
                &path,
                &[
                    ("marketplaceIds", MARKETPLACE),
                    ("identifiers", &identifiers),
                    ("identifiersType", "SKU"),
                    ("includedData", INCLUDED),
                    ("pageSize", &page_size),
                ],
            )?;
            listings.extend(page.items.iter().filter_map(channel_listing));
        }
        Ok(listings)
    }

    /// Amazon's estimate for the listing at `price`, shipped by the seller;
    /// for a draft, or where Amazon gives no estimate, its published table.
    fn sale_fee(&self, basis: &FeeBasis, price: Money) -> Result<SaleFee, PlatformError> {
        let Some(sku) = &basis.listing else {
            return table_fee(price);
        };
        let amount = price.rounded().amount();
        let currency = price.currency().code();
        // The amount goes as its decimal text: a JSON number, never a float.
        let body = format!(
            r#"{{"FeesEstimateRequest":{{"MarketplaceId":"{MARKETPLACE}","IsAmazonFulfilled":false,"PriceToEstimateFees":{{"ListingPrice":{{"CurrencyCode":"{currency}","Amount":{amount}}}}},"Identifier":{}}}}}"#,
            serde_json::Value::from(sku.as_str())
        );
        let answer: FeesAnswer = self.send(
            "POST",
            &format!(
                "/products/fees/v0/listings/{}/feesEstimate",
                path_segment(sku)
            ),
            &[],
            &body,
        )?;
        let result = answer
            .payload
            .and_then(|payload| payload.fees_estimate_result)
            .ok_or_else(unexpected_fee)?;
        match result.status.as_str() {
            "Success" => {}
            // Amazon estimates no fee for this offer, as for one it is
            // still reviewing.
            "ClientError" => return table_fee(price),
            status => {
                return Err(PlatformError::Failed(format!(
                    "Amazon could not estimate the fee: {status}"
                )));
            }
        }
        let estimate = result.fees_estimate.ok_or_else(unexpected_fee)?;
        let in_brl = |money: &super::answers::FeesMoney| {
            (money.currency_code == price.currency().code())
                .then(|| money.amount.decimal())
                .flatten()
        };
        let total = estimate
            .total_fees_estimate
            .as_ref()
            .and_then(in_brl)
            .ok_or_else(unexpected_fee)?;
        let referral = estimate
            .fee_detail_list
            .iter()
            .find(|detail| detail.fee_type == "ReferralFee")
            .and_then(|detail| detail.final_fee.as_ref())
            .and_then(in_brl)
            .unwrap_or(total);
        if price.amount() <= Decimal::ZERO {
            return Err(unexpected_fee());
        }
        Ok(SaleFee {
            rate: Percentage::new(referral * Decimal::ONE_HUNDRED / price.amount())
                .map_err(|_| unexpected_fee())?,
            fixed: Money::new(total - referral, price.currency()),
        })
    }

    fn set_price(&self, id: &str, price: Money) -> Result<(), PlatformError> {
        let value = format!(
            r#"[{{"marketplace_id":"{MARKETPLACE}","currency":"{}","audience":"ALL","our_price":[{{"schedule":[{{"value_with_tax":{}}}]}}]}}]"#,
            price.currency().code(),
            price.rounded().amount()
        );
        self.patch(id, "/attributes/purchasable_offer", &value)
    }

    /// An Amazon listing has no variations: `stock` names only it.
    fn set_stock(&self, id: &str, stock: &[ChannelStock]) -> Result<(), PlatformError> {
        let [
            ChannelStock {
                variation: None,
                available_quantity,
            },
        ] = stock
        else {
            return Err(PlatformError::NotFound);
        };
        self.send_quantity(id, *available_quantity)
    }

    /// Amazon has no pause: zero units take the listing off sale.
    fn pause(&self, id: &str) -> Result<(), PlatformError> {
        self.send_quantity(id, 0)
    }

    /// Nothing to tell Amazon: the stock send that follows puts the units
    /// back.
    fn activate(&self, _id: &str) -> Result<(), PlatformError> {
        Ok(())
    }

    fn pauses_with_zero_stock(&self) -> bool {
        true
    }

    /// A listing's category is its product type, named in Portuguese.
    fn category(&self, id: &str) -> Result<ChannelCategory, PlatformError> {
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(PlatformError::NotFound);
        }
        let found: ProductTypeDefinition = self.get(
            &format!("/definitions/2020-09-01/productTypes/{id}"),
            &[
                ("marketplaceIds", MARKETPLACE),
                ("requirements", "LISTING_OFFER_ONLY"),
                ("locale", "pt_BR"),
            ],
        )?;
        Ok(ChannelCategory {
            id: found.product_type,
            name: found.display_name,
        })
    }
}

impl Amazon {
    fn items_path(&self) -> Result<String, PlatformError> {
        Ok(format!("/listings/2021-08-01/items/{}", self.seller()?))
    }

    fn send_quantity(&self, id: &str, quantity: u32) -> Result<(), PlatformError> {
        let value = format!(
            r#"[{{"fulfillment_channel_code":"{OWN_FULFILLMENT}","quantity":{quantity}}}]"#
        );
        self.patch(id, "/attributes/fulfillment_availability", &value)
    }

    /// Merges `value`, JSON text, into the listing `sku`'s attribute at
    /// `path`, leaving the rest of the attribute as it is. Amazon takes the
    /// change to apply it within minutes, or names the issues that make it
    /// invalid.
    fn patch(&self, sku: &str, path: &str, value: &str) -> Result<(), PlatformError> {
        if sku.is_empty() {
            return Err(PlatformError::NotFound);
        }
        let body = format!(
            r#"{{"productType":"PRODUCT","patches":[{{"op":"merge","path":"{path}","value":{value}}}]}}"#
        );
        let submission: Submission = self.send(
            "PATCH",
            &format!("{}/{}", self.items_path()?, path_segment(sku)),
            &[("marketplaceIds", MARKETPLACE)],
            &body,
        )?;
        if submission.status == "INVALID" {
            let issues: Vec<String> = submission
                .issues
                .into_iter()
                .filter(|issue| issue.severity.as_deref() == Some("ERROR"))
                .filter_map(|issue| issue.message)
                .collect();
            return Err(PlatformError::Refused(if issues.is_empty() {
                "Amazon found the change invalid".into()
            } else {
                issues.join("; ")
            }));
        }
        Ok(())
    }
}

/// The store's summary of `item`.
fn summary(item: &Item) -> Option<&Summary> {
    item.summaries
        .iter()
        .find(|summary| summary.marketplace_id == MARKETPLACE)
}

/// The Listing `item` is, when it sells in the store: a parent SKU, an
/// offer without a price or one Amazon ships from its own centers is not
/// one.
fn channel_listing(item: &Item) -> Option<ChannelListing> {
    let summary = summary(item)?;
    let is_parent = item
        .relationships
        .iter()
        .flat_map(|of_store| &of_store.relationships)
        .any(|relationship| !relationship.child_skus.is_empty());
    if is_parent {
        return None;
    }
    let quantity = item
        .fulfillment_availability
        .iter()
        .find(|availability| availability.fulfillment_channel_code == OWN_FULFILLMENT)?
        .quantity
        .unwrap_or_default();
    let price = item
        .offers
        .iter()
        .filter(|offer| offer.marketplace_id == MARKETPLACE)
        .find(|offer| offer.offer_type.as_deref().is_none_or(|kind| kind == "B2C"))
        .and_then(|offer| offer.price.as_ref())
        .and_then(|price| {
            Some(Money::new(
                price.amount.decimal()?,
                Currency::from_code(&price.currency_code)?,
            ))
        })?;
    let status = if quantity == 0 {
        ListingStatus::OutOfStock
    } else if summary.status.iter().any(|status| status == "BUYABLE") {
        ListingStatus::Active
    } else {
        // Units but not for sale: Amazon holds it for an issue to fix.
        ListingStatus::Inactive
    };
    Some(ChannelListing {
        id: item.sku.clone(),
        variation: None,
        title: summary
            .item_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| item.sku.clone()),
        price,
        available_quantity: quantity,
        status,
        link: summary
            .asin
            .as_deref()
            .filter(|asin| asin.chars().all(|c| c.is_ascii_alphanumeric()))
            .map(|asin| format!("https://www.amazon.com.br/dp/{asin}")),
        // Amazon has a single way to list an offer: the default type.
        listing_type: Some(ListingType::default()),
        category: summary.product_type.clone(),
        seller_sku: Some(item.sku.clone()),
    })
}

/// The published table's fee at `price`.
fn table_fee(price: Money) -> Result<SaleFee, PlatformError> {
    let rate = Percentage::new(TABLE_RATE).map_err(|_| unexpected_fee())?;
    let minimum = Money::new(TABLE_MINIMUM, price.currency());
    if rate.of(price).amount() < minimum.amount() {
        return Ok(SaleFee {
            rate: Percentage::ZERO,
            fixed: minimum,
        });
    }
    Ok(SaleFee {
        rate,
        fixed: Money::zero(price.currency()),
    })
}

fn unexpected_fee() -> PlatformError {
    PlatformError::Failed("Amazon answered a fee estimate unexpectedly".into())
}
