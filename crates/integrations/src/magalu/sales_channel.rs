//! The owner's Magalu SKUs, for commerce's Sales Channel port: read them
//! with their price and stock in the store's channel, a new price once the
//! owner approves it, and the stock that follows the app's. Each SKU sells
//! on its own, variations included (they only share a group), so each is a
//! Listing (ADR 0014).

use mascate_commerce::{
    ChannelCategory, ChannelListing, ChannelStock, FeeBasis, ListingStatus, OnSalesChannel,
    SaleFee, SalesChannel,
};
use mascate_kernel::{Currency, ListingType, Money, Percentage, PlatformError, SalesChannelKind};
use rust_decimal::Decimal;

use super::answers::{ChannelRef, Prices, Sku, SkuPage, Stocks};
use super::{MAX_OFFSET, Magalu, in_units, sku_segment};

/// The most SKUs a page takes.
const PAGE_SIZE: usize = 100;

/// The channel name of the Magazine Luiza site in a SKU's addresses.
const MAGAZINE_LUIZA: &str = "magazineluiza";

/// Magalu publishes no consolidated table: its commission runs from 12% to
/// 20% by category, plus R$ 5,00 a unit on items above R$ 10,00, for a
/// company store (Parceiro Magalu terms as compiled by TOTVS and
/// ruacep.com.br, read on 2026-10-16). The fee where Magalu gives no
/// estimate is the highest, so a margin is never shown higher than it is;
/// the Order's real commission corrects it.
const TABLE_RATE: Decimal = Decimal::from_parts(20, 0, 0, false, 0);
const TABLE_FIXED: Decimal = Decimal::from_parts(5, 0, 0, false, 0);
const FIXED_ABOVE: Decimal = Decimal::from_parts(10, 0, 0, false, 0);
/// The SKU answer names no category and the fee above is the same for
/// every one, so each listing's fee basis names that table.
const TABLE_CATEGORY: &str = "fee-table";

impl OnSalesChannel for Magalu {
    fn sales_channel(&self) -> SalesChannelKind {
        SalesChannelKind::Magalu
    }
}

impl SalesChannel for Magalu {
    /// Pages through the portfolio, oldest first, leaving out the SKUs
    /// being deleted and the ones Magalu stocks itself.
    fn listing_ids(&self) -> Result<Vec<String>, PlatformError> {
        let limit = PAGE_SIZE.to_string();
        let mut ids: Vec<String> = Vec::new();
        let mut offset = 0;
        loop {
            let page: SkuPage = self.get(
                "/seller/v1/portfolios/skus",
                &[
                    ("_offset", &offset.to_string()),
                    ("_limit", &limit),
                    ("_sort", "created_at:asc"),
                ],
            )?;
            let read = page.results.len();
            for sku in page.results {
                if !sku.fulfillment && !is_gone(&sku) && !ids.contains(&sku.sku) {
                    ids.push(sku.sku);
                }
            }
            offset += read;
            if read < PAGE_SIZE || offset >= MAX_OFFSET {
                return Ok(ids);
            }
        }
    }

    /// Each SKU read on its own, with its price and stock in the store's
    /// channel; a SKU Magalu no longer has, or one without a price there, is
    /// left out.
    fn listings(&self, ids: &[String]) -> Result<Vec<ChannelListing>, PlatformError> {
        let channel = self.channel()?;
        let mut listings = Vec::new();
        for id in ids {
            let Ok(segment) = sku_segment(id) else {
                continue;
            };
            let sku: Sku = match self.get(&format!("/seller/v1/portfolios/skus/{segment}"), &[]) {
                Ok(sku) => sku,
                Err(PlatformError::NotFound) => continue,
                Err(error) => return Err(error),
            };
            if sku.fulfillment {
                continue;
            }
            let prices: Prices =
                match self.get(&format!("/seller/v1/portfolios/prices/{segment}"), &[]) {
                    Ok(prices) => prices,
                    Err(PlatformError::NotFound) => continue,
                    Err(error) => return Err(error),
                };
            let Some(price) = price_in(&prices, &channel) else {
                continue;
            };
            let stocks: Stocks =
                match self.get(&format!("/seller/v1/portfolios/stocks/{segment}"), &[]) {
                    Ok(stocks) => stocks,
                    Err(PlatformError::NotFound) => Stocks {
                        results: Vec::new(),
                    },
                    Err(error) => return Err(error),
                };
            listings.push(channel_listing(sku, price, available_in(&stocks, &channel)));
        }
        Ok(listings)
    }

    /// Magalu has no fee estimate: its published table.
    fn sale_fee(&self, _basis: &FeeBasis, price: Money) -> Result<SaleFee, PlatformError> {
        table_fee(price)
    }

    /// Sends the price in cents as both the price and the list price: the
    /// app has no Magalu Promotions (ADR 0033), so no "from" price shows
    /// crossed out. Magalu answers 202 and applies it later.
    fn set_price(&self, id: &str, price: Money) -> Result<(), PlatformError> {
        if price.currency() != Currency::Brl {
            return Err(PlatformError::Refused("Magalu sells in reais only".into()));
        }
        let cents = (price.rounded().amount() * Decimal::ONE_HUNDRED).normalize();
        let body = serde_json::json!({
            "price": cents.mantissa(),
            "list_price": cents.mantissa(),
            "normalizer": 100,
            "currency": "BRL",
            "channel": {"id": self.channel()?},
        });
        self.patch(
            &format!("/seller/v1/portfolios/prices/{}", sku_segment(id)?),
            &body.to_string(),
        )
    }

    /// A Magalu SKU has no variations of its own: `stock` names only it.
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

    /// Magalu has no pause in its API: zero units take the SKU off sale.
    fn pause(&self, id: &str) -> Result<(), PlatformError> {
        self.send_quantity(id, 0)
    }

    /// Nothing to tell Magalu: the stock send that follows puts the units
    /// back.
    fn activate(&self, _id: &str) -> Result<(), PlatformError> {
        Ok(())
    }

    fn pauses_with_zero_stock(&self) -> bool {
        true
    }

    fn applies_price_later(&self) -> bool {
        true
    }

    /// The app reads no category of a Magalu SKU.
    fn category(&self, _id: &str) -> Result<ChannelCategory, PlatformError> {
        Err(PlatformError::NotFound)
    }
}

impl Magalu {
    fn send_quantity(&self, id: &str, quantity: u32) -> Result<(), PlatformError> {
        let body = serde_json::json!({
            "quantity": quantity,
            "type": "AVAILABLE",
            "channel": {"id": self.channel()?},
        });
        self.patch(
            &format!("/seller/v1/portfolios/stocks/{}", sku_segment(id)?),
            &body.to_string(),
        )
    }
}

fn is_gone(sku: &Sku) -> bool {
    matches!(sku.status.as_deref(), Some("DELETED" | "DELETING"))
}

/// Whether an entry kept for `of` is the store's `channel`'s; one without
/// a channel counts for every channel.
fn is_in(of: &Option<ChannelRef>, channel: &str) -> bool {
    of.as_ref().is_none_or(|of| of.id == channel)
}

/// The SKU's price in the store's channel.
fn price_in(prices: &Prices, channel: &str) -> Option<Money> {
    let price = prices
        .results
        .iter()
        .find(|price| is_in(&price.channel, channel))?;
    if price.price <= 0 {
        return None;
    }
    Some(Money::new(
        in_units(price.price, price.normalizer)?,
        Currency::from_code(&price.currency)?,
    ))
}

/// The units available to sell in the store's channel, every warehouse of
/// it together; the reserved ones are already sold.
fn available_in(stocks: &Stocks, channel: &str) -> u32 {
    let units: i64 = stocks
        .results
        .iter()
        .filter(|stock| stock.kind == "AVAILABLE" && is_in(&stock.channel, channel))
        .map(|stock| stock.quantity.max(0))
        .sum();
    u32::try_from(units).unwrap_or(u32::MAX)
}

fn channel_listing(sku: Sku, price: Money, available: u32) -> ChannelListing {
    let status = match sku.status.as_deref() {
        Some("DELETED" | "DELETING") => ListingStatus::Closed,
        Some("UNDER_REVIEW") => ListingStatus::UnderReview,
        Some("BLOCKED") => ListingStatus::Inactive,
        // Turned off in Magalu's panel: the owner's pause.
        _ if !sku.active => ListingStatus::Paused,
        _ if available == 0 => ListingStatus::OutOfStock,
        Some("PUBLISHED") => ListingStatus::Active,
        // Units but not published: Magalu holds it for something to fix.
        _ => ListingStatus::Inactive,
    };
    let link = sku
        .url_marketplace
        .iter()
        .filter(|address| address.url.starts_with("https://"))
        .max_by_key(|address| address.channel == MAGAZINE_LUIZA)
        .map(|address| address.url.clone());
    ChannelListing {
        id: sku.sku.clone(),
        variation: None,
        title: sku
            .title
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| sku.sku.clone()),
        price,
        available_quantity: available,
        status,
        link,
        // Magalu has a single way to list a SKU: the default type.
        listing_type: Some(ListingType::default()),
        category: Some(TABLE_CATEGORY.into()),
        seller_sku: Some(sku.sku),
    }
}

/// The published table's fee at `price`.
pub(super) fn table_fee(price: Money) -> Result<SaleFee, PlatformError> {
    let rate = Percentage::new(TABLE_RATE)
        .map_err(|_| PlatformError::Failed("Magalu's fee table has a rate out of range".into()))?;
    let fixed = if price.amount() > FIXED_ABOVE {
        TABLE_FIXED
    } else {
        Decimal::ZERO
    };
    Ok(SaleFee {
        rate,
        fixed: Money::new(fixed, price.currency()),
    })
}
