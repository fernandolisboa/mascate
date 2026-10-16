//! The owner's Shopee listings, for commerce's Sales Channel port: read
//! them, the fee of Shopee's table, a new price once the owner approves it,
//! the stock that follows the app's, and unlisting and relisting. An item
//! with variations is one Listing per model (ADR 0014); an item without
//! them is model 0 when Shopee is told its price or stock.

use mascate_commerce::{
    ChannelCategory, ChannelListing, ChannelStock, FeeBasis, ListingStatus, OnSalesChannel,
    SaleFee, SalesChannel, Variation,
};
use mascate_kernel::{Currency, ListingType, Money, Percentage, PlatformError, SalesChannelKind};
use rust_decimal::Decimal;

use super::answers::{
    Categories, Item, ItemBaseInfo, ItemList, Model, ModelList, PriceInfo, Update,
};
use super::{ShopeeSeller, is_numeric_id};

/// Items per page of the shop's items; the most Shopee allows.
const PAGE_SIZE: &str = "100";

/// Item ids per `get_item_base_info`; the most Shopee allows.
const BASE_INFO_LIMIT: usize = 50;

/// The item id Shopee's updates take for an item without variations.
pub(super) const NO_MODEL: &str = "0";

/// Shopee's commission for a CNPJ seller, by the item's price, in force
/// since 2026-03-01 ("Comissão para vendedores CNPJ e CPF em 2026",
/// seller.shopee.com.br/edu/article/26839, read on 2026-10-16): the fee
/// where Shopee gives no estimate. Each band is its lowest price, the
/// percentage and the fixed part per unit; below R$ 8,00 the fixed part is
/// half the price. A CPF seller past 450 orders in 90 days pays R$ 3,00
/// more, which the Order's real Fee corrects.
const TABLE: [(Decimal, Decimal, Decimal); 4] = [
    (
        Decimal::from_parts(200, 0, 0, false, 0),
        Decimal::from_parts(14, 0, 0, false, 0),
        Decimal::from_parts(26, 0, 0, false, 0),
    ),
    (
        Decimal::from_parts(100, 0, 0, false, 0),
        Decimal::from_parts(14, 0, 0, false, 0),
        Decimal::from_parts(20, 0, 0, false, 0),
    ),
    (
        Decimal::from_parts(80, 0, 0, false, 0),
        Decimal::from_parts(14, 0, 0, false, 0),
        Decimal::from_parts(16, 0, 0, false, 0),
    ),
    (
        Decimal::ZERO,
        Decimal::from_parts(20, 0, 0, false, 0),
        Decimal::from_parts(4, 0, 0, false, 0),
    ),
];
const HALF_PRICE_BELOW: Decimal = Decimal::from_parts(8, 0, 0, false, 0);

impl OnSalesChannel for ShopeeSeller {
    fn sales_channel(&self) -> SalesChannelKind {
        SalesChannelKind::Shopee
    }
}

impl SalesChannel for ShopeeSeller {
    /// Pages through the shop's items on sale and unlisted.
    fn listing_ids(&self) -> Result<Vec<String>, PlatformError> {
        let mut ids: Vec<String> = Vec::new();
        let mut offset = 0;
        loop {
            let at = offset.to_string();
            let page: ItemList = self.get(
                "/api/v2/product/get_item_list",
                &[
                    ("offset", &at),
                    ("page_size", PAGE_SIZE),
                    ("item_status", "NORMAL"),
                    ("item_status", "UNLIST"),
                ],
            )?;
            for item in page.item {
                if !ids.contains(&item.item_id) {
                    ids.push(item.item_id);
                }
            }
            // An offset that does not move would read the same page forever.
            if !page.has_next_page || page.next_offset <= offset {
                return Ok(ids);
            }
            offset = page.next_offset;
        }
    }

    /// Read 50 items at a time, and the models of each item that has them.
    /// An id Shopee no longer has is not in the answer.
    fn listings(&self, ids: &[String]) -> Result<Vec<ChannelListing>, PlatformError> {
        let shop = self.shop_id()?;
        let ids: Vec<&str> = ids
            .iter()
            .map(String::as_str)
            .filter(|id| is_numeric_id(id))
            .collect();
        let mut listings = Vec::new();
        for chunk in ids.chunks(BASE_INFO_LIMIT) {
            let found: ItemBaseInfo = self.get(
                "/api/v2/product/get_item_base_info",
                &[("item_id_list", &chunk.join(","))],
            )?;
            for item in found.item_list {
                if item.has_model {
                    let models = self.models(&item.item_id)?;
                    for model in &models.model {
                        listings.push(model_listing(&item, model, &models, &shop)?);
                    }
                } else {
                    listings.push(item_listing(&item, &shop)?);
                }
            }
        }
        Ok(listings)
    }

    /// Shopee's published table at `price`: its API has no estimate.
    fn sale_fee(&self, _basis: &FeeBasis, price: Money) -> Result<SaleFee, PlatformError> {
        table_fee(price)
    }

    /// Every model of the item takes the price, read from Shopee right
    /// before, never from the last Sync.
    fn set_price(&self, id: &str, price: Money) -> Result<(), PlatformError> {
        let item = item_id(id)?;
        let price = price.rounded().amount();
        let models = self.models(item)?;
        let prices: Vec<String> = if models.model.is_empty() {
            vec![format!(
                r#"{{"model_id":{NO_MODEL},"original_price":{price}}}"#
            )]
        } else {
            models
                .model
                .iter()
                .map(|model| {
                    format!(
                        r#"{{"model_id":{},"original_price":{price}}}"#,
                        model.model_id
                    )
                })
                .collect()
        };
        let body = format!(
            r#"{{"item_id":{item},"price_list":[{}]}}"#,
            prices.join(",")
        );
        self.update("/api/v2/product/update_price", &body)
    }

    /// The stock of the item or of each model named in `stock`; the other
    /// models keep theirs. Zero units leave the item listed and sold out.
    fn set_stock(&self, id: &str, stock: &[ChannelStock]) -> Result<(), PlatformError> {
        let item = item_id(id)?;
        if stock.is_empty() {
            return Err(PlatformError::NotFound);
        }
        let mut entries = Vec::new();
        for wanted in stock {
            let model = match wanted.variation.as_deref() {
                None => NO_MODEL,
                Some(model) if is_numeric_id(model) => model,
                Some(_) => return Err(PlatformError::NotFound),
            };
            entries.push(format!(
                r#"{{"model_id":{model},"seller_stock":[{{"stock":{}}}]}}"#,
                wanted.available_quantity
            ));
        }
        let body = format!(
            r#"{{"item_id":{item},"stock_list":[{}]}}"#,
            entries.join(",")
        );
        self.update("/api/v2/product/update_stock", &body)
    }

    /// Unlists the item, every model with it.
    fn pause(&self, id: &str) -> Result<(), PlatformError> {
        self.unlist(id, true)
    }

    fn activate(&self, id: &str) -> Result<(), PlatformError> {
        self.unlist(id, false)
    }

    /// The category's name as Shopee shows it in Portuguese.
    fn category(&self, id: &str) -> Result<ChannelCategory, PlatformError> {
        if !is_numeric_id(id) {
            return Err(PlatformError::NotFound);
        }
        let found: Categories =
            self.get("/api/v2/product/get_category", &[("language", "pt-br")])?;
        let category = found
            .category_list
            .into_iter()
            .find(|category| category.category_id.to_string() == id)
            .ok_or(PlatformError::NotFound)?;
        Ok(ChannelCategory {
            id: id.to_owned(),
            name: category
                .display_category_name
                .or(category.original_category_name)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| id.to_owned()),
        })
    }
}

impl ShopeeSeller {
    fn models(&self, item: &str) -> Result<ModelList, PlatformError> {
        self.get("/api/v2/product/get_model_list", &[("item_id", item)])
    }

    fn unlist(&self, id: &str, unlist: bool) -> Result<(), PlatformError> {
        let item = item_id(id)?;
        self.update(
            "/api/v2/product/unlist_item",
            &format!(r#"{{"item_list":[{{"item_id":{item},"unlist":{unlist}}}]}}"#),
        )
    }

    /// Sends an update; Shopee answers success even when it refused some of
    /// it, naming each refusal in `failure_list`.
    fn update(&self, path: &str, body: &str) -> Result<(), PlatformError> {
        let answer: Update = self.post(path, body)?;
        if answer.failure_list.is_empty() {
            return Ok(());
        }
        let reasons: Vec<String> = answer
            .failure_list
            .into_iter()
            .filter_map(|failure| failure.failed_reason)
            .filter(|reason| !reason.trim().is_empty())
            .collect();
        Err(PlatformError::Refused(if reasons.is_empty() {
            "Shopee did not take the change".into()
        } else {
            reasons.join("; ")
        }))
    }
}

fn item_id(id: &str) -> Result<&str, PlatformError> {
    if is_numeric_id(id) {
        Ok(id)
    } else {
        Err(PlatformError::NotFound)
    }
}

/// An item without variations as one Listing.
fn item_listing(item: &Item, shop: &str) -> Result<ChannelListing, PlatformError> {
    let available = item
        .stock_info_v2
        .as_ref()
        .map(|stock| stock.available())
        .unwrap_or_default();
    Ok(ChannelListing {
        id: item.item_id.clone(),
        variation: None,
        title: title(item),
        price: price(&item.price_info, &item.item_id)?,
        available_quantity: available,
        status: status(&item.item_status, None, available),
        link: Some(link(shop, &item.item_id)),
        listing_type: Some(ListingType::default()),
        category: item.category_id.map(|id| id.to_string()),
        seller_sku: sku(&item.item_sku),
    })
}

/// One model of an item as its own Listing, named by its tier options.
fn model_listing(
    item: &Item,
    model: &Model,
    models: &ModelList,
    shop: &str,
) -> Result<ChannelListing, PlatformError> {
    let available = model
        .stock_info_v2
        .as_ref()
        .map(|stock| stock.available())
        .unwrap_or_default();
    Ok(ChannelListing {
        id: item.item_id.clone(),
        variation: Some(Variation {
            id: model.model_id.clone(),
            name: variation_name(model, models),
        }),
        title: title(item),
        price: price(&model.price_info, &item.item_id)?,
        available_quantity: available,
        status: status(&item.item_status, model.model_status.as_deref(), available),
        link: Some(link(shop, &item.item_id)),
        listing_type: Some(ListingType::default()),
        category: item.category_id.map(|id| id.to_string()),
        seller_sku: sku(&model.model_sku).or_else(|| sku(&item.item_sku)),
    })
}

fn title(item: &Item) -> String {
    item.item_name
        .clone()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| item.item_id.clone())
}

fn price(prices: &[PriceInfo], item: &str) -> Result<Money, PlatformError> {
    prices
        .first()
        .and_then(|price| {
            Some(Money::new(
                crate::answers::decimal(&price.original_price)?,
                Currency::from_code(&price.currency)?,
            ))
        })
        .ok_or_else(|| {
            PlatformError::Failed(format!("Shopee sent the item {item} without a price"))
        })
}

fn sku(sku: &Option<String>) -> Option<String> {
    sku.as_deref()
        .map(str::trim)
        .filter(|sku| !sku.is_empty())
        .map(str::to_owned)
}

/// Where buyers see the item.
fn link(shop: &str, item: &str) -> String {
    format!("https://shopee.com.br/product/{shop}/{item}")
}

/// "Cor: Preto · Tamanho: M", from the model's option in each tier.
fn variation_name(model: &Model, models: &ModelList) -> String {
    let name = model
        .tier_index
        .iter()
        .zip(&models.tier_variation)
        .filter_map(|(&option, tier)| {
            let option = tier.option_list.get(option)?.option.trim();
            Some(if tier.name.trim().is_empty() {
                option.to_owned()
            } else {
                format!("{}: {option}", tier.name.trim())
            })
        })
        .collect::<Vec<_>>()
        .join(" · ");
    if name.is_empty() {
        model.model_id.clone()
    } else {
        name
    }
}

/// A listing's status. Shopee keeps an item with no units listed and sold
/// out, and sells it again when units arrive: the share's out of stock.
fn status(item: &str, model: Option<&str>, available: u32) -> ListingStatus {
    match item {
        "UNLIST" => ListingStatus::Paused,
        "NORMAL" if model == Some("MODEL_UNAVAILABLE") => ListingStatus::Inactive,
        "NORMAL" if available == 0 => ListingStatus::OutOfStock,
        "NORMAL" => ListingStatus::Active,
        "REVIEWING" => ListingStatus::UnderReview,
        "SELLER_DELETE" | "SHOPEE_DELETE" => ListingStatus::Closed,
        // `BANNED` and any new one.
        _ => ListingStatus::Inactive,
    }
}

/// The published table's fee at `price`.
pub(super) fn table_fee(price: Money) -> Result<SaleFee, PlatformError> {
    let amount = price.amount();
    let (_, rate, fixed) = TABLE
        .into_iter()
        .find(|(from, _, _)| amount >= *from)
        .unwrap_or(TABLE[TABLE.len() - 1]);
    let fixed = if amount < HALF_PRICE_BELOW {
        (amount / Decimal::TWO).max(Decimal::ZERO)
    } else {
        fixed
    };
    Ok(SaleFee {
        rate: Percentage::new(rate).map_err(|_| {
            PlatformError::Failed("Shopee's fee table has a rate out of range".into())
        })?,
        fixed: Money::new(fixed, price.currency()),
    })
}
