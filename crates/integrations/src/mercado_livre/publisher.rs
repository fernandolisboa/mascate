//! Drafts published to Mercado Livre, for commerce's ListingPublisher port:
//! the category predictor, a category's attributes, picture upload, the
//! listing validator, the new listing and its description.

use mascate_commerce::{
    AttributeValue, CategoryAttribute, CategoryPrediction, ChannelCategory, ChannelIssue,
    ListingPublisher, ListingToPublish, PublishedListing, Requirement,
};
use mascate_kernel::{Currency, PlatformError};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use super::answers::{
    AttributeDefinition, DomainPrediction, ErrorAnswer, ItemState, MultigetEntry, PictureAnswer,
    SellerItems, UserAnswer,
};
use super::sales_channel::{is_web_link, listing_type_id, path_id, status};
use super::{MercadoLivre, SITE, failed, read_json};

/// Categories asked of the predictor.
const PREDICTIONS: &str = "3";

/// Attributes the draft fills from its own fields, never by hand.
const SET_BY_THE_DRAFT: [&str; 2] = ["SELLER_SKU", "ITEM_CONDITION"];

/// Mercado Livre's barcode attribute: without it a listing loses exposure.
const GTIN: &str = "GTIN";

/// A seller Mercado Livre moved to user products, whose listings take a
/// `family_name` and get a title Mercado Livre writes.
const USER_PRODUCT_SELLER: &str = "user_product_seller";

impl ListingPublisher for MercadoLivre {
    fn currency(&self) -> Currency {
        Currency::Brl
    }

    fn predict_categories(&self, title: &str) -> Result<Vec<CategoryPrediction>, PlatformError> {
        let found: Vec<DomainPrediction> = self.get(
            &format!("/sites/{SITE}/domain_discovery/search"),
            &[("limit", PREDICTIONS), ("q", title)],
        )?;
        Ok(found
            .into_iter()
            .map(|prediction| CategoryPrediction {
                category: ChannelCategory {
                    id: prediction.category_id,
                    name: prediction.category_name,
                },
                attributes: prediction
                    .attributes
                    .into_iter()
                    .filter_map(|attribute| {
                        Some(AttributeValue {
                            id: attribute.id?,
                            value: attribute.value_name?,
                        })
                    })
                    .collect(),
            })
            .collect())
    }

    /// Read-only attributes, and the ones the draft sets itself, are left
    /// out. `required` (and `catalog_required`) make one required; one
    /// required only in some cases, or for new items, and the GTIN are
    /// recommended, and the validator has the last word.
    fn category_attributes(&self, category: &str) -> Result<Vec<CategoryAttribute>, PlatformError> {
        let category = path_id(category)?;
        let found: Vec<AttributeDefinition> =
            self.get(&format!("/categories/{category}/attributes"), &[])?;
        Ok(found
            .into_iter()
            .filter(|attribute| {
                !attribute.tags.read_only && !SET_BY_THE_DRAFT.contains(&attribute.id.as_str())
            })
            .map(|attribute| {
                let tags = &attribute.tags;
                let requirement = if tags.required || tags.catalog_required {
                    Requirement::Required
                } else if tags.conditional_required || tags.new_required || attribute.id == GTIN {
                    Requirement::Recommended
                } else {
                    Requirement::Optional
                };
                CategoryAttribute {
                    id: attribute.id,
                    name: attribute.name,
                    requirement,
                }
            })
            .collect())
    }

    fn upload_picture(&self, file_name: &str, bytes: &[u8]) -> Result<String, PlatformError> {
        let boundary = boundary_for(bytes);
        let body = multipart(&boundary, file_name, bytes);
        let path = "/pictures/items/upload";
        let response = self.signed(path, |bearer| {
            self.agent
                .post(format!("{}{path}", self.api))
                .header("Accept", "application/json")
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .header("Authorization", bearer)
                .send(&body[..])
        })?;
        Ok(read_json::<PictureAnswer>(response)?.id)
    }

    /// Mercado Livre answers 204 when the listing would go out, and 400
    /// with its causes when not; a cause of type `warning` does not block.
    fn validate(&self, listing: &ListingToPublish) -> Result<Vec<ChannelIssue>, PlatformError> {
        let body = self.item_body(listing)?;
        let path = "/items/validate";
        let response = self.answered(|bearer| self.posting(path, bearer, &body))?;
        match response.status().as_u16() {
            200 | 201 | 204 => Ok(Vec::new()),
            400 => {
                let answer: ErrorAnswer = read_json(response)?;
                let issues: Vec<ChannelIssue> = answer
                    .cause
                    .into_iter()
                    .filter_map(|cause| {
                        Some(ChannelIssue {
                            blocks: cause.kind.as_deref() != Some("warning"),
                            message: cause.message?,
                        })
                    })
                    .collect();
                if issues.is_empty() {
                    let message = answer
                        .message
                        .or(answer.error)
                        .unwrap_or_else(|| "validation_error".into());
                    return Ok(vec![ChannelIssue {
                        message,
                        blocks: true,
                    }]);
                }
                Ok(issues)
            }
            _ => Err(failed(response, path)),
        }
    }

    fn publish(&self, listing: &ListingToPublish) -> Result<PublishedListing, PlatformError> {
        let created: ItemState = self.post("/items", &self.item_body(listing)?)?;
        Ok(published(created))
    }

    /// Posts the description; one already there, from an attempt whose
    /// answer was lost, is replaced instead.
    fn describe(&self, id: &str, description: &str) -> Result<(), PlatformError> {
        let id = path_id(id)?;
        let path = format!("/items/{id}/description");
        let body = json!({ "plain_text": description }).to_string();
        match self.post::<Value>(&path, &body) {
            Err(PlatformError::Refused(_)) => self.put(&format!("{path}?api_version=2"), &body),
            other => other.map(|_| ()),
        }
    }

    fn find_by_seller_sku(&self, seller_sku: &str) -> Result<Vec<PublishedListing>, PlatformError> {
        let user: UserAnswer = self.get("/users/me", &[])?;
        let found: SellerItems = self.get(
            &format!("/users/{}/items/search", user.id),
            &[("seller_sku", seller_sku)],
        )?;
        if found.results.is_empty() {
            return Ok(Vec::new());
        }
        let answers: Vec<MultigetEntry> = self.get(
            "/items",
            &[
                ("ids", &found.results.join(",")),
                ("attributes", "id,status,permalink"),
            ],
        )?;
        Ok(answers
            .into_iter()
            .filter(|answer| answer.code == 200)
            .filter_map(|answer| answer.body::<ItemState>().ok())
            .map(published)
            .collect())
    }
}

impl MercadoLivre {
    /// The JSON `POST /items` and `/items/validate` take. A seller moved to
    /// user products sends the title as `family_name`, and Mercado Livre
    /// writes the listing's title from it.
    fn item_body(&self, listing: &ListingToPublish) -> Result<String, PlatformError> {
        let user: UserAnswer = self.get("/users/me", &[])?;
        let title_field = if user.tags.iter().any(|tag| tag == USER_PRODUCT_SELLER) {
            "family_name"
        } else {
            "title"
        };
        let mut attributes: Vec<Value> = listing
            .attributes
            .iter()
            .map(|attribute| json!({ "id": attribute.id, "value_name": attribute.value }))
            .collect();
        attributes.push(json!({ "id": "SELLER_SKU", "value_name": listing.seller_sku }));
        let mut body = json!({
            title_field: listing.title,
            "site_id": SITE,
            "category_id": listing.category,
            "currency_id": listing.price.currency().code(),
            "available_quantity": listing.available_quantity,
            "buying_mode": "buy_it_now",
            "condition": listing.condition.code(),
            "listing_type_id": listing_type_id(listing.listing_type),
            "pictures": listing
                .pictures
                .iter()
                .map(|id| json!({ "id": id }))
                .collect::<Vec<_>>(),
            "attributes": attributes,
        });
        if let Some(warranty) = &listing.warranty {
            body["sale_terms"] = json!([
                { "id": "WARRANTY_TYPE", "value_name": "Garantia do vendedor" },
                { "id": "WARRANTY_TIME", "value_name": warranty },
            ]);
        }
        Ok(with_price(&body, listing.price.rounded().amount()))
    }
}

/// `object` with `price` first, written as the decimal number it is: a
/// price never goes through a float.
fn with_price(object: &Value, price: Decimal) -> String {
    let rest = object.to_string();
    match rest.strip_prefix('{') {
        Some("}") | None => format!(r#"{{"price":{price}}}"#),
        Some(fields) => format!(r#"{{"price":{price},{fields}"#),
    }
}

fn published(item: ItemState) -> PublishedListing {
    PublishedListing {
        status: status(item.status.as_deref().unwrap_or_default()),
        link: item.permalink.filter(|link| is_web_link(link)),
        id: item.id,
    }
}

/// A `multipart/form-data` body with the file in the `file` field. The
/// name goes in quotes, so quotes and line breaks are taken out of it.
fn multipart(boundary: &str, file_name: &str, bytes: &[u8]) -> Vec<u8> {
    let name: String = file_name
        .chars()
        .filter(|c| !matches!(c, '"' | '\r' | '\n' | '\\'))
        .collect();
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\n\
         Content-Type: {}\r\n\r\n",
        content_type(file_name)
    )
    .into_bytes();
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

fn content_type(file_name: &str) -> &'static str {
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else {
        "image/jpeg"
    }
}

/// A multipart boundary must not occur in the file: one derived from its
/// bytes, checked against them, does not.
fn boundary_for(bytes: &[u8]) -> String {
    let mut seed = bytes.len() as u64;
    loop {
        // FNV-1a over the bytes and the seed.
        let hash = bytes
            .iter()
            .chain(seed.to_le_bytes().iter())
            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
            });
        let boundary = format!("mascate-{hash:016x}");
        if !contains(bytes, boundary.as_bytes()) {
            return boundary;
        }
        seed = seed.wrapping_add(1);
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
