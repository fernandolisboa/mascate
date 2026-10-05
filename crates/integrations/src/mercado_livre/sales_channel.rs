//! The owner's listings, for commerce's Sales Channel port: read them, the
//! sale fee at a price, a new price once the owner approves it, the stock
//! that follows the app's, and pausing and reactivating.

use mascate_commerce::{
    ChannelCategory, ChannelListing, ChannelStock, ListingStatus, SaleFee, SalesChannel, Variation,
};
use mascate_kernel::{ListingType, Money, Percentage, PlatformError};
use rust_decimal::Decimal;

use super::MercadoLivre;
use super::answers::{
    Attribute, CategoryAnswer, ItemVariationIds, MultigetEntry, SellerItem, SellerItems,
    UserAnswer, decimal, money,
};

/// Listings per page of the seller's listings; the most Mercado Livre allows.
const SEARCH_PAGE: &str = "100";

/// Ids per `/items` multiget; the most Mercado Livre allows.
const MULTIGET_LIMIT: usize = 20;

/// Mercado Livre's id for a listing type.
pub(super) fn listing_type_id(listing: ListingType) -> &'static str {
    match listing {
        ListingType::Classic => "gold_special",
        ListingType::Premium => "gold_pro",
    }
}

impl SalesChannel for MercadoLivre {
    /// Pages through the seller's listings with `search_type=scan`, which
    /// has no cap on how many it returns, one status at a time.
    fn listing_ids(&self) -> Result<Vec<String>, PlatformError> {
        let user: UserAnswer = self.get("/users/me", &[])?;
        let path = format!("/users/{}/items/search", user.id);
        let mut ids: Vec<String> = Vec::new();
        for status in ["active", "paused"] {
            let mut scroll: Option<String> = None;
            loop {
                let mut query = vec![
                    ("status", status),
                    ("search_type", "scan"),
                    ("limit", SEARCH_PAGE),
                ];
                if let Some(scroll) = &scroll {
                    query.push(("scroll_id", scroll));
                }
                let page: SellerItems = self.get(&path, &query)?;
                if page.results.is_empty() {
                    break;
                }
                for id in page.results {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                match page.scroll_id {
                    Some(next) => scroll = Some(next),
                    None => break,
                }
            }
        }
        Ok(ids)
    }

    fn listings(&self, ids: &[String]) -> Result<Vec<ChannelListing>, PlatformError> {
        let mut listings = Vec::new();
        for chunk in ids.chunks(MULTIGET_LIMIT) {
            // `include_attributes=all` brings each variation's own attributes,
            // where its SELLER_SKU is.
            let answers: Vec<MultigetEntry> = self.get(
                "/items",
                &[("ids", &chunk.join(",")), ("include_attributes", "all")],
            )?;
            for answer in answers {
                match answer.code {
                    200 => {
                        let item: SellerItem = answer.body().map_err(|error| {
                            PlatformError::Failed(format!(
                                "Mercado Livre answered a listing unexpectedly: {error}"
                            ))
                        })?;
                        listings.extend(channel_listings(item)?);
                    }
                    // Deleted: the Sync closes it.
                    404 => {}
                    code => {
                        return Err(PlatformError::Failed(format!(
                            "Mercado Livre answered {code} for one of the listings"
                        )));
                    }
                }
            }
        }
        Ok(listings)
    }

    /// The percentage and fixed parts of the fee from `sale_fee_details`;
    /// without them, the whole fee as a share of the price.
    fn sale_fee(
        &self,
        category: &str,
        price: Money,
        listing_type: ListingType,
    ) -> Result<SaleFee, PlatformError> {
        let found = self.listing_price(category, price, listing_type)?;
        let unexpected =
            || PlatformError::Failed("Mercado Livre answered a sale fee unexpectedly".into());
        let parts = found.sale_fee_details.as_ref().and_then(|details| {
            Some((
                decimal(&details.percentage_fee)?,
                decimal(&details.fixed_fee).unwrap_or_default(),
            ))
        });
        let (percent, fixed) = match parts {
            Some(parts) => parts,
            None => {
                let whole = money(&found.sale_fee_amount, &found.currency_id)
                    .ok_or(PlatformError::NotFound)?;
                if price.amount() <= Decimal::ZERO {
                    return Err(unexpected());
                }
                (
                    whole.amount() * Decimal::ONE_HUNDRED / price.amount(),
                    Decimal::ZERO,
                )
            }
        };
        Ok(SaleFee {
            rate: Percentage::new(percent).map_err(|_| unexpected())?,
            fixed: Money::new(fixed, price.currency()),
        })
    }

    /// A listing with variations takes the price on each of them, all named
    /// in one request: Mercado Livre deletes a variation left out. So the
    /// variations are read from Mercado Livre right before, never from the
    /// last Sync.
    fn set_price(&self, id: &str, price: Money) -> Result<(), PlatformError> {
        let id = path_id(id)?;
        let price = price.rounded().amount();
        let item: ItemVariationIds = self.get(&format!("/items/{id}"), &[])?;
        let body = if item.variations.is_empty() {
            format!(r#"{{"price":{price}}}"#)
        } else {
            let variations: Vec<String> = item
                .variations
                .iter()
                .map(|variation| format!(r#"{{"id":{},"price":{price}}}"#, variation.id))
                .collect();
            format!(r#"{{"variations":[{}]}}"#, variations.join(","))
        };
        self.put(&format!("/items/{id}"), &body)
    }

    /// Like the price, a listing with variations names every one of them,
    /// read right before, and a stock only on the ones in `stock`. Zero
    /// units make Mercado Livre pause the listing until some return.
    fn set_stock(&self, id: &str, stock: &[ChannelStock]) -> Result<(), PlatformError> {
        let id = path_id(id)?;
        let item: ItemVariationIds = self.get(&format!("/items/{id}"), &[])?;
        let body = if item.variations.is_empty() {
            match stock {
                [
                    ChannelStock {
                        variation: None,
                        available_quantity,
                    },
                ] => format!(r#"{{"available_quantity":{available_quantity}}}"#),
                _ => return Err(PlatformError::NotFound),
            }
        } else {
            let known = |variation: &ChannelStock| {
                item.variations.iter().any(|known| {
                    variation.variation.as_deref() == Some(known.id.to_string().as_str())
                })
            };
            if !stock.iter().all(known) {
                return Err(PlatformError::NotFound);
            }
            let variations: Vec<String> = item
                .variations
                .iter()
                .map(|variation| {
                    let wanted = stock.iter().find(|wanted| {
                        wanted.variation.as_deref() == Some(variation.id.to_string().as_str())
                    });
                    match wanted {
                        Some(wanted) => format!(
                            r#"{{"id":{},"available_quantity":{}}}"#,
                            variation.id, wanted.available_quantity
                        ),
                        None => format!(r#"{{"id":{}}}"#, variation.id),
                    }
                })
                .collect();
            format!(r#"{{"variations":[{}]}}"#, variations.join(","))
        };
        self.put(&format!("/items/{id}"), &body)
    }

    fn pause(&self, id: &str) -> Result<(), PlatformError> {
        self.put(
            &format!("/items/{}", path_id(id)?),
            r#"{"status":"paused"}"#,
        )
    }

    fn activate(&self, id: &str) -> Result<(), PlatformError> {
        self.put(
            &format!("/items/{}", path_id(id)?),
            r#"{"status":"active"}"#,
        )
    }

    fn category(&self, id: &str) -> Result<ChannelCategory, PlatformError> {
        let found: CategoryAnswer = self.get(&format!("/categories/{}", path_id(id)?), &[])?;
        Ok(ChannelCategory {
            id: found.id,
            name: found.name,
        })
    }
}

/// `id` as it goes into a path: only Mercado Livre's own letters and
/// digits, so no other path is ever reached.
pub(super) fn path_id(id: &str) -> Result<&str, PlatformError> {
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()) {
        Ok(id)
    } else {
        Err(PlatformError::NotFound)
    }
}

/// A listing, or one per variation when it has them. A variation's SKU is,
/// in Mercado Livre's own order: its SELLER_SKU attribute, its
/// `seller_custom_field`, then the listing's.
fn channel_listings(item: SellerItem) -> Result<Vec<ChannelListing>, PlatformError> {
    let price = money(&item.price, &item.currency_id).ok_or_else(|| {
        PlatformError::Failed(format!("Mercado Livre sent {} without a price", item.id))
    })?;
    let item_sku = seller_sku(&item.attributes, &item.seller_custom_field);
    let listing = ChannelListing {
        id: item.id,
        variation: None,
        title: item.title,
        price,
        available_quantity: item.available_quantity,
        status: status(&item.status),
        link: item.permalink.filter(|link| is_web_link(link)),
        listing_type: item.listing_type_id.as_deref().and_then(listing_type),
        category: item.category_id,
        seller_sku: item_sku,
    };
    if item.variations.is_empty() {
        return Ok(vec![listing]);
    }
    Ok(item
        .variations
        .into_iter()
        .map(|variation| ChannelListing {
            variation: Some(Variation {
                id: variation.id.to_string(),
                name: variation_name(&variation.attribute_combinations),
            }),
            price: money(&variation.price, &item.currency_id).unwrap_or(listing.price),
            available_quantity: variation.available_quantity,
            seller_sku: seller_sku(&variation.attributes, &variation.seller_custom_field)
                .or_else(|| listing.seller_sku.clone()),
            ..listing.clone()
        })
        .collect())
}

fn seller_sku(attributes: &[Attribute], custom_field: &Option<String>) -> Option<String> {
    attributes
        .iter()
        .find(|attribute| attribute.id.as_deref() == Some("SELLER_SKU"))
        .and_then(|attribute| attribute.value_name.clone())
        .filter(|sku| !sku.trim().is_empty())
        .or_else(|| custom_field.clone().filter(|sku| !sku.trim().is_empty()))
        .map(|sku| sku.trim().to_owned())
}

/// "Cor: Preto · Tamanho: M".
fn variation_name(combinations: &[Attribute]) -> String {
    combinations
        .iter()
        .filter_map(|attribute| {
            let value = attribute.value_name.as_deref()?;
            Some(match attribute.name.as_deref() {
                Some(name) => format!("{name}: {value}"),
                None => value.to_owned(),
            })
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

pub(super) fn status(code: &str) -> ListingStatus {
    match code {
        "active" => ListingStatus::Active,
        "paused" => ListingStatus::Paused,
        "closed" => ListingStatus::Closed,
        "under_review" => ListingStatus::UnderReview,
        // `inactive`, `payment_required`, `not_yet_active` and any new one.
        _ => ListingStatus::Inactive,
    }
}

pub(super) fn listing_type(id: &str) -> Option<ListingType> {
    ListingType::ALL
        .into_iter()
        .find(|kind| listing_type_id(*kind) == id)
}

/// The app opens a listing's link in the browser, so it takes only a web
/// address.
pub(super) fn is_web_link(link: &str) -> bool {
    link.starts_with("https://") || link.starts_with("http://")
}
