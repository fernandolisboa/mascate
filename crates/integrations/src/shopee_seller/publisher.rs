//! Drafts published to Shopee (#132), for commerce's ListingPublisher port:
//! the categories Shopee recommends for a title, a category's attributes
//! with the brand, weight and GTIN the draft fills, pictures to Shopee's
//! media space, a check before anything goes out, the new item, its
//! variations as models and its description. Shopee has no validator, so
//! the check is the adapter's own: the brand in Shopee's list, values a
//! drop-down takes, the weight, and a shipping channel that carries it.

use std::time::Duration;

use mascate_commerce::{
    CategoryAttribute, CategoryPrediction, ChannelIssue, Condition, Gtin, ListingPublisher,
    ListingToPublish, PublishedListing, PublishedVariation, Requirement, VariationsToPublish,
};
use mascate_kernel::{Currency, PlatformError, parse_amount};
use rust_decimal::Decimal;
use serde_json::Value;

use super::answers::{
    AddedItem, AttributeNode, AttributeTrees, BrandList, CategoryRecommendation, FoundItems,
    ItemBaseInfo, LogisticsChannels, ModelList, Translation, UploadedImage,
};
use super::sales_channel::{item_id, link, status, variation_name};
use super::{ShopeeSeller, attempt, is_numeric_id};
use crate::pictures::PictureForm;

/// The draft's fields for what Shopee asks of every item, beside the
/// category's attributes, whose ids are numbers.
const BRAND: &str = "brand";
const WEIGHT: &str = "weight";
const GTIN: &str = "gtin_code";

/// How Shopee names an item without a brand.
const NO_BRAND: &str = "NoBrand";

/// The most pictures an item takes.
const MAX_PICTURES: usize = 9;

/// Shopee asks for at least 5 seconds between creating an item and giving
/// it variations, while it processes the item.
const VARIATION_WAIT: Duration = Duration::from_secs(5);

/// Brands per page, the most Shopee allows, and pages read before giving
/// up on finding one.
const BRAND_PAGE: &str = "100";
const BRAND_PAGES: usize = 30;

/// Item ids per page of a search by SKU.
const SEARCH_PAGE: &str = "100";
const SEARCH_PAGES: usize = 10;

/// Drop-downs: the value must be one of Shopee's.
const SINGLE_DROP_DOWN: u8 = 1;
const MULTIPLE_DROP_DOWN: u8 = 4;
const MULTIPLE_COMBO_BOX: u8 = 5;
const QUANTITY_WITH_UNIT: u8 = 2;

/// What the draft comes to as Shopee takes it, and what holds it back.
struct Prepared {
    issues: Vec<ChannelIssue>,
    /// `attribute_list`, as JSON.
    attributes: Vec<Value>,
    brand: Value,
    /// Kilograms.
    weight: Option<Decimal>,
    logistics: Vec<u64>,
    gtin: Option<String>,
}

impl ListingPublisher for ShopeeSeller {
    fn currency(&self) -> Currency {
        Currency::Brl
    }

    /// Shopee's recommendation, named from its category tree.
    fn predict_categories(&self, title: &str) -> Result<Vec<CategoryPrediction>, PlatformError> {
        let found: CategoryRecommendation = self.get(
            "/api/v2/product/category_recommend",
            &[("item_name", title)],
        )?;
        if found.category_id.is_empty() {
            return Ok(Vec::new());
        }
        let tree = self.categories()?;
        Ok(found
            .category_id
            .into_iter()
            .map(|id| {
                let id = id.to_string();
                let category = tree
                    .iter()
                    .find(|category| category.id == id)
                    .cloned()
                    .unwrap_or_else(|| mascate_commerce::ChannelCategory {
                        name: id.clone(),
                        id,
                    });
                CategoryPrediction {
                    category,
                    attributes: Vec::new(),
                }
            })
            .collect())
    }

    /// The category's attributes, the mandatory ones required, after the
    /// brand, which Shopee requires in some categories, and before the
    /// weight with packaging, which it always asks, and the GTIN.
    fn category_attributes(&self, category: &str) -> Result<Vec<CategoryAttribute>, PlatformError> {
        let tree = self.attribute_tree(category)?;
        let brands = self.brands(category, 0, "1")?;
        let mut attributes = vec![CategoryAttribute {
            id: BRAND.into(),
            name: "Marca".into(),
            requirement: if brands.is_mandatory {
                Requirement::Required
            } else {
                Requirement::Recommended
            },
        }];
        attributes.extend(tree.iter().map(|attribute| CategoryAttribute {
            id: attribute.attribute_id.to_string(),
            name: in_portuguese(&attribute.name, &attribute.multi_lang),
            requirement: if attribute.mandatory {
                Requirement::Required
            } else {
                Requirement::Optional
            },
        }));
        attributes.push(CategoryAttribute {
            id: WEIGHT.into(),
            name: "Peso com embalagem (kg)".into(),
            requirement: Requirement::Required,
        });
        attributes.push(CategoryAttribute {
            id: GTIN.into(),
            name: "GTIN (código de barras)".into(),
            requirement: Requirement::Recommended,
        });
        Ok(attributes)
    }

    fn upload_picture(&self, file_name: &str, bytes: &[u8]) -> Result<String, PlatformError> {
        let form = PictureForm::new("image", file_name, bytes);
        let path = "/api/v2/media_space/upload_image";
        let uploaded: UploadedImage = self.signed(path, super::BACKOFF, attempt, |common| {
            let mut request = self
                .agent
                .post(format!("{}{path}", self.api))
                .header("Accept", "application/json")
                .header("Content-Type", &form.content_type);
            for (key, value) in common {
                request = request.query(*key, *value);
            }
            request.send(&form.body[..])
        })?;
        uploaded
            .image_info
            .map(|image| image.image_id)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| PlatformError::Failed("Shopee kept the picture without its id".into()))
    }

    fn validate(&self, listing: &ListingToPublish) -> Result<Vec<ChannelIssue>, PlatformError> {
        Ok(self.prepare(listing)?.issues)
    }

    /// Creates the item once: a lost answer is looked for by its SKU before
    /// another try. An item with variations is created unlisted and goes on
    /// sale once it has them.
    fn publish(&self, listing: &ListingToPublish) -> Result<PublishedListing, PlatformError> {
        let prepared = self.prepare(listing)?;
        let blocking: Vec<&str> = prepared
            .issues
            .iter()
            .filter(|issue| issue.blocks)
            .map(|issue| issue.message.as_str())
            .collect();
        if !blocking.is_empty() {
            return Err(PlatformError::Refused(blocking.join(" ")));
        }
        let body = item_body(listing, &prepared)?;
        let added: AddedItem = self.post_once_for("/api/v2/product/add_item", &body)?;
        let shop = self.shop_id()?;
        Ok(PublishedListing {
            status: status(
                added.item_status.as_deref().unwrap_or("NORMAL"),
                None,
                listing.available_quantity,
            ),
            link: Some(link(&shop, &added.item_id)),
            id: added.item_id,
        })
    }

    fn describe(&self, id: &str, description: &str) -> Result<(), PlatformError> {
        let item = item_id(id)?;
        let body = format!(
            r#"{{"item_id":{item},"description":{}}}"#,
            Value::from(description)
        );
        self.post::<Value>("/api/v2/product/update_item", &body)
            .map(|_| ())
    }

    /// Shopee's search matches the SKU loosely, so each item found is read
    /// to keep only the ones with exactly `seller_sku`.
    fn find_by_seller_sku(&self, seller_sku: &str) -> Result<Vec<PublishedListing>, PlatformError> {
        let mut ids: Vec<String> = Vec::new();
        let mut offset = String::new();
        for _ in 0..SEARCH_PAGES {
            let mut query = vec![("page_size", SEARCH_PAGE), ("item_sku", seller_sku)];
            if !offset.is_empty() {
                query.push(("offset", offset.as_str()));
            }
            let page: FoundItems = self.get("/api/v2/product/search_item", &query)?;
            ids.extend(page.item_id_list.iter().map(u64::to_string));
            if page.next_offset.is_empty() || page.next_offset == offset {
                break;
            }
            offset = page.next_offset;
        }
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let shop = self.shop_id()?;
        let mut found = Vec::new();
        for chunk in ids.chunks(50) {
            let items: ItemBaseInfo = self.get(
                "/api/v2/product/get_item_base_info",
                &[("item_id_list", &chunk.join(","))],
            )?;
            found.extend(
                items
                    .item_list
                    .into_iter()
                    .filter(|item| item.item_sku.as_deref().map(str::trim) == Some(seller_sku))
                    .map(|item| PublishedListing {
                        // An item with models has no stock of its own: its
                        // models' comes with the next Sync.
                        status: status(
                            &item.item_status,
                            None,
                            item.stock_info_v2
                                .as_ref()
                                .map_or(u32::from(item.has_model), |stock| stock.available()),
                        ),
                        link: Some(link(&shop, &item.item_id)),
                        id: item.item_id,
                    }),
            );
        }
        Ok(found)
    }

    /// The models go once, after the wait Shopee asks for, and the item
    /// goes on sale; an item that has models already keeps them, and only
    /// goes on sale.
    fn add_variations(
        &self,
        id: &str,
        listing: &ListingToPublish,
    ) -> Result<Vec<PublishedVariation>, PlatformError> {
        let item = item_id(id)?;
        let wanted = listing.variations.as_ref().ok_or_else(|| {
            PlatformError::Refused("the listing has no variations to send".into())
        })?;
        let mut models = self.models(item)?;
        if models.model.is_empty() {
            self.pause.pause(VARIATION_WAIT);
            self.post_once_for::<Value>(
                "/api/v2/product/init_tier_variation",
                &variations_body(item, listing, wanted),
            )?;
            models = self.models(item)?;
        }
        self.unlist(item, false)?;
        Ok(published_variations(&models))
    }
}

impl ShopeeSeller {
    fn attribute_tree(&self, category: &str) -> Result<Vec<AttributeNode>, PlatformError> {
        if !is_numeric_id(category) {
            return Err(PlatformError::NotFound);
        }
        let trees: AttributeTrees = self.get(
            "/api/v2/product/get_attribute_tree",
            &[("category_id_list", category), ("language", "pt-BR")],
        )?;
        Ok(trees
            .list
            .into_iter()
            .next()
            .map(|tree| tree.attribute_tree)
            .unwrap_or_default())
    }

    fn brands(
        &self,
        category: &str,
        offset: u64,
        page_size: &str,
    ) -> Result<BrandList, PlatformError> {
        if !is_numeric_id(category) {
            return Err(PlatformError::NotFound);
        }
        self.get(
            "/api/v2/product/get_brand_list",
            &[
                ("offset", &offset.to_string()),
                ("page_size", page_size),
                ("category_id", category),
                ("status", "1"),
                ("language", "pt-br"),
            ],
        )
    }

    /// The brand as Shopee takes it: one of its list for the category, or
    /// none where the category lets an item go without; `Err` with why
    /// not.
    fn brand(&self, category: &str, typed: &str) -> Result<Result<Value, String>, PlatformError> {
        let first = self.brands(category, 0, BRAND_PAGE)?;
        let mandatory = first.is_mandatory;
        let typed = typed.trim();
        if typed.is_empty() || is_no_brand(typed) {
            return Ok(if mandatory {
                Err(
                    "A Shopee pede a marca nesta categoria; escreva como ela aparece na lista \
                     de marcas da Shopee."
                        .into(),
                )
            } else {
                Ok(brand_value(0, NO_BRAND))
            });
        }
        let mut page = first;
        for _ in 0..BRAND_PAGES {
            if let Some(brand) = page.brand_list.iter().find(|brand| {
                same_text(&brand.original_brand_name, typed)
                    || brand
                        .display_brand_name
                        .as_deref()
                        .is_some_and(|name| same_text(name, typed))
            }) {
                return Ok(Ok(brand_value(brand.brand_id, &brand.original_brand_name)));
            }
            if !page.has_next_page {
                break;
            }
            page = self.brands(category, page.next_offset, BRAND_PAGE)?;
        }
        Ok(Err(format!(
            "A marca {typed} não está na lista da Shopee para esta categoria. Escreva como a \
             Shopee lista{}.",
            if mandatory {
                ""
            } else {
                ", ou deixe a marca vazia para anunciar sem marca"
            }
        )))
    }

    /// The shop's shipping channels that can carry an item of `weight` kg:
    /// on in the shop, picked by buyers at checkout, with a fee Shopee sets.
    fn logistics(&self, weight: Option<Decimal>) -> Result<Vec<u64>, PlatformError> {
        let channels: LogisticsChannels = self.get("/api/v2/logistics/get_channel_list", &[])?;
        Ok(channels
            .logistics_channel_list
            .into_iter()
            .filter(|channel| {
                channel.enabled
                    && channel.mask_channel_id == 0
                    && !matches!(
                        channel.fee_type.as_deref(),
                        Some("SIZE_SELECTION" | "CUSTOM_PRICE")
                    )
            })
            .filter(|channel| {
                let (Some(weight), Some(limit)) = (weight, &channel.weight_limit) else {
                    return true;
                };
                let bound = |number: &Option<serde_json::Number>| {
                    number
                        .as_ref()
                        .and_then(crate::answers::decimal)
                        .filter(|bound| !bound.is_zero())
                };
                bound(&limit.item_max_weight).is_none_or(|max| weight <= max)
                    && bound(&limit.item_min_weight).is_none_or(|min| weight >= min)
            })
            .map(|channel| channel.logistics_channel_id)
            .collect())
    }

    /// What Shopee would make of `listing`, with what holds it back.
    fn prepare(&self, listing: &ListingToPublish) -> Result<Prepared, PlatformError> {
        let mut issues = Vec::new();
        let mut block = |message: String| {
            issues.push(ChannelIssue {
                message,
                blocks: true,
            })
        };
        let value = |id: &str| {
            listing
                .attributes
                .iter()
                .find(|attribute| attribute.id == id)
                .map(|attribute| attribute.value.trim())
                .filter(|value| !value.is_empty())
        };
        if listing.description.trim().is_empty() {
            block("A Shopee pede uma descrição.".into());
        }
        if listing.pictures.len() > MAX_PICTURES {
            block(format!(
                "A Shopee aceita até {MAX_PICTURES} fotos; marque no máximo {MAX_PICTURES}."
            ));
        }
        let weight = match value(WEIGHT) {
            None => {
                block("Digite o peso com embalagem, em kg.".into());
                None
            }
            Some(text) => match parse_amount(text).filter(|kg| *kg > Decimal::ZERO) {
                Some(kg) => Some(kg),
                None => {
                    block(format!(
                        "O peso {text} não é um número de kg; digite como 0,35."
                    ));
                    None
                }
            },
        };
        let gtin = match value(GTIN) {
            None => None,
            Some(text) => match Gtin::parse(text) {
                Some(gtin) => Some(gtin.digits().to_owned()),
                None => {
                    block(format!(
                        "O GTIN {text} não confere: são 8, 12, 13 ou 14 números do código de \
                         barras."
                    ));
                    None
                }
            },
        };
        let tree = self.attribute_tree(&listing.category)?;
        let mut attributes = Vec::new();
        for node in &tree {
            let Some(text) = value(&node.attribute_id.to_string()) else {
                continue;
            };
            match attribute_value(node, text) {
                Ok(values) => attributes.push(serde_json::json!({
                    "attribute_id": node.attribute_id,
                    "attribute_value_list": values,
                })),
                Err(message) => block(message),
            }
        }
        let brand = match self.brand(&listing.category, value(BRAND).unwrap_or_default())? {
            Ok(brand) => brand,
            Err(message) => {
                block(message);
                Value::Null
            }
        };
        let logistics = self.logistics(weight)?;
        if logistics.is_empty() {
            block(
                "Nenhum canal de envio ligado na loja da Shopee leva este anúncio. Ligue um em \
                 Minha loja › Configurações de envio, no Seller Center, e valide de novo."
                    .into(),
            );
        }
        Ok(Prepared {
            issues,
            attributes,
            brand,
            weight,
            logistics,
            gtin,
        })
    }
}

/// `typed` as the values Shopee takes for `node`: one of its options for a
/// drop-down, several separated by commas where it takes several, the
/// number and its unit for a quantity, and the text itself otherwise.
fn attribute_value(node: &AttributeNode, typed: &str) -> Result<Vec<Value>, String> {
    let name = in_portuguese(&node.name, &node.multi_lang);
    let info = node.attribute_info.as_ref();
    let input = info.and_then(|info| info.input_type);
    let several = matches!(input, Some(MULTIPLE_DROP_DOWN | MULTIPLE_COMBO_BOX));
    let parts: Vec<&str> = if several {
        typed
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect()
    } else {
        vec![typed]
    };
    let mut values = Vec::new();
    for part in parts {
        let option = node.attribute_value_list.iter().find(|option| {
            same_text(&option.name, part)
                || option
                    .multi_lang
                    .iter()
                    .any(|translation| same_text(&translation.value, part))
        });
        match option {
            Some(option) => values.push(serde_json::json!({
                "value_id": option.value_id,
                "original_value_name": option.name,
            })),
            None if matches!(input, Some(SINGLE_DROP_DOWN | MULTIPLE_DROP_DOWN)) => {
                let shown: Vec<String> = node
                    .attribute_value_list
                    .iter()
                    .take(8)
                    .map(|option| in_portuguese(&option.name, &option.multi_lang))
                    .collect();
                return Err(format!(
                    "Em {name}, a Shopee aceita só valores da lista dela, como {}.",
                    shown.join(", ")
                ));
            }
            None if info.and_then(|info| info.format_type) == Some(QUANTITY_WITH_UNIT) => {
                let units = &info
                    .expect("a format type comes with the info")
                    .attribute_unit_list;
                let unit = units.iter().find(|unit| {
                    part.to_lowercase()
                        .strip_suffix(&unit.to_lowercase())
                        .is_some_and(|number| parse_amount(number.trim()).is_some())
                });
                match unit {
                    Some(unit) => {
                        let number = part[..part.len() - unit.len()].trim();
                        values.push(serde_json::json!({
                            "value_id": 0,
                            "original_value_name": number,
                            "value_unit": unit,
                        }));
                    }
                    None => {
                        return Err(format!(
                            "Em {name}, digite o número e a unidade, como 500 {}.",
                            units.first().map_or("g", String::as_str)
                        ));
                    }
                }
            }
            None => values.push(serde_json::json!({
                "value_id": 0,
                "original_value_name": part,
            })),
        }
    }
    Ok(values)
}

/// `product/add_item`'s body. Price and weight go as their decimal text,
/// JSON numbers, never floats.
fn item_body(listing: &ListingToPublish, prepared: &Prepared) -> Result<String, PlatformError> {
    if !is_numeric_id(&listing.category) {
        return Err(PlatformError::NotFound);
    }
    let weight = prepared
        .weight
        .ok_or_else(|| PlatformError::Refused("no weight".into()))?;
    let logistics: Vec<Value> = prepared
        .logistics
        .iter()
        .map(|id| serde_json::json!({ "logistic_id": id, "enabled": true }))
        .collect();
    let gtin = prepared
        .gtin
        .as_ref()
        .map(|gtin| format!(r#","gtin_code":{}"#, Value::from(gtin.as_str())))
        .unwrap_or_default();
    Ok(format!(
        r#"{{"item_name":{},"description":{},"original_price":{},"weight":{},"category_id":{},"condition":"{}","item_sku":{},"item_status":"{}","image":{{"image_id_list":{}}},"logistic_info":{},"attribute_list":{},"brand":{},"seller_stock":[{{"stock":{}}}]{gtin}}}"#,
        Value::from(listing.title.as_str()),
        Value::from(listing.description.as_str()),
        listing.price.rounded().amount(),
        weight.normalize(),
        listing.category,
        match listing.condition {
            Condition::New => "NEW",
            Condition::Used => "USED",
        },
        Value::from(listing.seller_sku.as_str()),
        // Until it has its variations, an item with them is not on sale.
        if listing.variations.is_some() {
            "UNLIST"
        } else {
            "NORMAL"
        },
        Value::from(listing.pictures.clone()),
        Value::from(logistics),
        Value::from(prepared.attributes.clone()),
        prepared.brand,
        listing.available_quantity,
    ))
}

/// `product/init_tier_variation`'s body: one tier of Shopee's own options,
/// a model per option at the listing's price.
fn variations_body(item: &str, listing: &ListingToPublish, wanted: &VariationsToPublish) -> String {
    let options: Vec<Value> = wanted
        .options
        .iter()
        .map(|option| {
            serde_json::json!({
                "variation_option_id": 0,
                "variation_option_name": option.option,
            })
        })
        .collect();
    let price = listing.price.rounded().amount();
    let models: Vec<String> = wanted
        .options
        .iter()
        .enumerate()
        .map(|(at, option)| {
            format!(
                r#"{{"tier_index":[{at}],"original_price":{price},"model_sku":{},"seller_stock":[{{"stock":{}}}]}}"#,
                Value::from(option.seller_sku.as_str()),
                option.available_quantity
            )
        })
        .collect();
    format!(
        r#"{{"item_id":{item},"standardise_tier_variation":[{{"variation_id":0,"variation_group_id":0,"variation_name":{},"variation_option_list":{}}}],"model":[{}]}}"#,
        Value::from(wanted.tier.as_str()),
        Value::from(options),
        models.join(",")
    )
}

/// Each model as the variation of its SKU.
fn published_variations(models: &ModelList) -> Vec<PublishedVariation> {
    models
        .model
        .iter()
        .filter_map(|model| {
            let available = model
                .stock_info_v2
                .as_ref()
                .map(|stock| stock.available())
                .unwrap_or_default();
            Some(PublishedVariation {
                id: model.model_id.clone(),
                seller_sku: model
                    .model_sku
                    .as_deref()
                    .map(str::trim)
                    .filter(|sku| !sku.is_empty())?
                    .to_owned(),
                name: variation_name(model, models),
                status: status("NORMAL", model.model_status.as_deref(), available),
            })
        })
        .collect()
}

fn brand_value(id: u64, name: &str) -> Value {
    serde_json::json!({ "brand_id": id, "original_brand_name": name })
}

fn is_no_brand(typed: &str) -> bool {
    ["sem marca", "nobrand", "no brand", "genérico", "generico"]
        .iter()
        .any(|none| same_text(none, typed))
}

/// The same words, case and spaces around aside.
fn same_text(left: &str, right: &str) -> bool {
    left.trim().to_lowercase() == right.trim().to_lowercase()
}

/// `name` as Shopee shows it in Portuguese, when it translates it.
fn in_portuguese(name: &str, translations: &[Translation]) -> String {
    translations
        .iter()
        .find(|translation| translation.language.eq_ignore_ascii_case("pt-br"))
        .map(|translation| translation.value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or(name.trim())
        .to_owned()
}
