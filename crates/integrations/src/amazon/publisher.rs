//! Drafts offered on Amazon, for commerce's ListingPublisher port (#128): a
//! draft is an offer on a product Amazon's catalog already has, its ASIN
//! found by the GTIN (Catalog Items 2022-04-01), sent with `putListingsItem`
//! and the `LISTING_OFFER_ONLY` requirements. The product's page, pictures
//! and description are Amazon's; creating a new ASIN is out of epic #32
//! (ADR 0033). Amazon keys a listing by its SKU: sending the same SKU again
//! replaces the offer instead of adding one, so a publish never touches a
//! listing the seller already has with it.

use mascate_commerce::{
    AttributeValue, CatalogItem, CategoryAttribute, CategoryPrediction, ChannelCategory,
    ChannelIssue, Condition, Gtin, ListingPublisher, ListingStatus, ListingToPublish,
    PublishedListing, Requirement,
};
use mascate_kernel::{Currency, PlatformError};
use serde_json::{Map, Value, json};

use super::answers::{CatalogAnswer, CatalogSearch, Item, Submission};
use super::sales_channel::{OWN_FULFILLMENT, offer_value, summary};
use super::{Amazon, MARKETPLACE};
use crate::escaped::escaped;

/// The attribute that names the ASIN an offer joins.
const ASIN: &str = "merchant_suggested_asin";

/// The products a search by one GTIN lists at most.
const CATALOG_PAGE: &str = "20";

impl ListingPublisher for Amazon {
    fn currency(&self) -> Currency {
        Currency::Brl
    }

    /// An offer joins a product of the catalog, found by its GTIN: there
    /// is no category to predict.
    fn predict_categories(&self, _title: &str) -> Result<Vec<CategoryPrediction>, PlatformError> {
        Ok(Vec::new())
    }

    /// An offer on an existing product carries the ASIN; condition, price
    /// and stock come from the draft's own fields.
    fn category_attributes(
        &self,
        product_type: &str,
    ) -> Result<Vec<CategoryAttribute>, PlatformError> {
        product_type_of(product_type)?;
        Ok(vec![CategoryAttribute {
            id: ASIN.into(),
            name: "ASIN".into(),
            requirement: Requirement::Required,
        }])
    }

    fn upload_picture(&self, _file_name: &str, _bytes: &[u8]) -> Result<String, PlatformError> {
        Err(PlatformError::Refused(
            "an offer on an existing ASIN takes the pictures Amazon already has".into(),
        ))
    }

    /// Amazon's preview of the offer: its `ERROR` issues block, its
    /// warnings do not. Nothing is created.
    fn validate(&self, listing: &ListingToPublish) -> Result<Vec<ChannelIssue>, PlatformError> {
        let submission = self.put_offer(listing, true)?;
        Ok(issues(submission))
    }

    /// Sends the offer once Amazon has no listing with its SKU, which the
    /// same SKU would replace. Amazon applies it within minutes, so it
    /// starts under review until a Sync reads it.
    fn publish(&self, listing: &ListingToPublish) -> Result<PublishedListing, PlatformError> {
        if !self.find_by_seller_sku(&listing.seller_sku)?.is_empty() {
            return Err(PlatformError::Refused(format!(
                "Amazon already has a listing with the SKU {}",
                listing.seller_sku
            )));
        }
        let submission = self.put_offer(listing, false)?;
        if submission.status != "ACCEPTED" {
            let messages: Vec<String> = issues(submission)
                .into_iter()
                .filter(|issue| issue.blocks)
                .map(|issue| issue.message)
                .collect();
            return Err(PlatformError::Refused(if messages.is_empty() {
                "Amazon found the offer invalid".into()
            } else {
                messages.join("; ")
            }));
        }
        Ok(PublishedListing {
            id: listing.seller_sku.clone(),
            status: ListingStatus::UnderReview,
            link: listing
                .attributes
                .iter()
                .find(|attribute| attribute.id == ASIN)
                .and_then(|asin| product_link(&asin.value)),
        })
    }

    /// An offer has no description of its own: the product's page has it.
    fn describe(&self, _id: &str, _description: &str) -> Result<(), PlatformError> {
        Ok(())
    }

    /// The seller's listing with the SKU, if Amazon has one: a listing's id
    /// is its SKU.
    fn find_by_seller_sku(&self, seller_sku: &str) -> Result<Vec<PublishedListing>, PlatformError> {
        if seller_sku.is_empty() {
            return Ok(Vec::new());
        }
        let found: Result<Item, PlatformError> = self.get(
            &format!("{}/{}", self.items_path()?, escaped(seller_sku)),
            &[
                ("marketplaceIds", MARKETPLACE),
                ("includedData", "summaries,fulfillmentAvailability"),
            ],
        );
        match found {
            Ok(item) => Ok(vec![published(&item)]),
            Err(PlatformError::NotFound) => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    /// The store's products with the GTIN; one without a product type
    /// cannot take an offer and is left out.
    fn catalog_items(&self, gtin: &Gtin) -> Result<Vec<CatalogItem>, PlatformError> {
        let found: CatalogSearch = self.get(
            "/catalog/2022-04-01/items",
            &[
                ("identifiers", gtin.digits()),
                ("identifiersType", identifiers_type(gtin)),
                ("marketplaceIds", MARKETPLACE),
                ("includedData", "summaries,productTypes"),
                ("locale", "pt_BR"),
                ("pageSize", CATALOG_PAGE),
            ],
        )?;
        Ok(found.items.into_iter().filter_map(catalog_item).collect())
    }
}

impl Amazon {
    /// `putListingsItem` with the offer, or its preview, which creates
    /// nothing. Issues come in Portuguese.
    fn put_offer(
        &self,
        listing: &ListingToPublish,
        preview: bool,
    ) -> Result<Submission, PlatformError> {
        if listing.seller_sku.is_empty() {
            return Err(PlatformError::Refused(
                "an Amazon listing needs a SKU".into(),
            ));
        }
        let path = format!("{}/{}", self.items_path()?, escaped(&listing.seller_sku));
        let mut query = vec![("marketplaceIds", MARKETPLACE), ("issueLocale", "pt_BR")];
        if preview {
            query.push(("mode", "VALIDATION_PREVIEW"));
        }
        self.send("PUT", &path, &query, &offer_body(listing)?)
    }
}

/// The offer as `putListingsItem` takes it: the ASIN's product type, the
/// draft's attributes, its condition, price and units, shipped by the
/// seller.
fn offer_body(listing: &ListingToPublish) -> Result<String, PlatformError> {
    let product_type = product_type_of(&listing.category)?;
    let mut attributes = Map::new();
    for AttributeValue { id, value } in &listing.attributes {
        attributes.insert(
            id.clone(),
            json!([{ "value": value, "marketplace_id": MARKETPLACE }]),
        );
    }
    attributes.insert(
        "condition_type".into(),
        json!([{ "value": condition_type(listing.condition), "marketplace_id": MARKETPLACE }]),
    );
    attributes.insert(
        "fulfillment_availability".into(),
        json!([{
            "fulfillment_channel_code": OWN_FULFILLMENT,
            "quantity": listing.available_quantity,
        }]),
    );
    let rest = Value::Object(attributes).to_string();
    let rest = rest
        .strip_prefix('{')
        .and_then(|rest| rest.strip_suffix('}'))
        .unwrap_or_default();
    // The price goes as its decimal text, never a float.
    Ok(format!(
        r#"{{"productType":"{product_type}","requirements":"LISTING_OFFER_ONLY","attributes":{{"purchasable_offer":{},{rest}}}}}"#,
        offer_value(listing.price)
    ))
}

/// A product type as Amazon names them, `HEADPHONES`: anything else is not
/// one.
fn product_type_of(text: &str) -> Result<&str, PlatformError> {
    if !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Ok(text)
    } else {
        Err(PlatformError::NotFound)
    }
}

/// Amazon's condition for the draft's: a used item goes as in good shape.
fn condition_type(condition: Condition) -> &'static str {
    match condition {
        Condition::New => "new_new",
        Condition::Used => "used_good",
    }
}

/// How Amazon names the GTIN's kind: 12 digits are a UPC, 14 a GTIN-14,
/// and 8 or 13 an EAN.
fn identifiers_type(gtin: &Gtin) -> &'static str {
    match gtin.digits().len() {
        12 => "UPC",
        14 => "GTIN",
        _ => "EAN",
    }
}

fn issues(submission: Submission) -> Vec<ChannelIssue> {
    submission
        .issues
        .into_iter()
        .filter_map(|issue| {
            Some(ChannelIssue {
                blocks: issue.severity.as_deref() == Some("ERROR"),
                message: issue.message?,
            })
        })
        .collect()
}

/// The listing as the store shows it: off sale with no units, for sale when
/// buyable, still under review before Amazon summarises it.
fn published(item: &Item) -> PublishedListing {
    let summary = summary(item);
    let quantity = item
        .fulfillment_availability
        .iter()
        .find(|availability| availability.fulfillment_channel_code == OWN_FULFILLMENT)
        .and_then(|availability| availability.quantity);
    let status = match summary {
        None => ListingStatus::UnderReview,
        Some(_) if quantity == Some(0) => ListingStatus::OutOfStock,
        Some(summary) if summary.status.iter().any(|status| status == "BUYABLE") => {
            ListingStatus::Active
        }
        Some(_) => ListingStatus::Inactive,
    };
    PublishedListing {
        id: item.sku.clone(),
        status,
        link: summary
            .and_then(|summary| summary.asin.as_deref())
            .and_then(product_link),
    }
}

/// The product's page in the store, for an ASIN of letters and digits.
fn product_link(asin: &str) -> Option<String> {
    (!asin.is_empty() && asin.chars().all(|c| c.is_ascii_alphanumeric()))
        .then(|| format!("https://www.amazon.com.br/dp/{asin}"))
}

fn catalog_item(answer: CatalogAnswer) -> Option<CatalogItem> {
    let link = product_link(&answer.asin)?;
    let product_type = answer
        .product_types
        .iter()
        .filter(|kind| {
            kind.marketplace_id
                .as_deref()
                .is_none_or(|id| id == MARKETPLACE)
        })
        .find_map(|kind| kind.product_type.clone())
        .filter(|kind| product_type_of(kind).is_ok())?;
    let summary = answer
        .summaries
        .into_iter()
        .find(|summary| summary.marketplace_id == MARKETPLACE);
    let (title, brand, classification) = match summary {
        Some(summary) => (
            summary.item_name,
            summary.brand,
            summary
                .browse_classification
                .and_then(|classification| classification.display_name),
        ),
        None => (None, None, None),
    };
    Some(CatalogItem {
        title: title
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| answer.asin.clone()),
        brand: brand.filter(|brand| !brand.trim().is_empty()),
        category: ChannelCategory {
            name: classification
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| product_type.clone()),
            id: product_type,
        },
        attributes: vec![AttributeValue {
            id: ASIN.into(),
            value: answer.asin.clone(),
        }],
        id: answer.asin,
        link: Some(link),
    })
}
