//! Drafts published to Shopee through the Open Platform v2 seller adapter
//! (#132): categories, attributes, brand and shipping channels, pictures
//! to the media space, the item created once, its variations as models,
//! and commerce's draft end to end on a real temporary database, against
//! the fake server serving the answers in `fixtures/shopee-seller`,
//! written from the documentation (see its README).

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use hmac::{Hmac, Mac};
use mascate_commerce::{
    AttributeValue, CatalogProduct, CategoryAttribute, ChannelCategory, Check, Condition,
    DraftEdit, DraftStart, DraftVariation, ListingDraft, ListingError, ListingPublisher,
    ListingStatus, ListingToPublish, Listings, PublishedListing, PublishedVariation, Requirement,
    VariationToPublish, VariationsToPublish,
};
use mascate_integrations::ShopeeSeller;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, ListingType, Money, Pause, PlatformError, RecordId};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use sha2::Sha256;

const PARTNER: &str = "2001887";
const KEY: &str = "shpk-mascate-partner-key";
const SHOP: &str = "14701711";
/// 2026-10-16 12:00 UTC, the fake clock's time.
const NOW: i64 = 1_792_152_000;
const ACCESS: &str = "59777174636562737266615546704c6d";

const RENEW: &str = "/api/v2/auth/access_token/get";
const RECOMMEND: &str = "/api/v2/product/category_recommend";
const CATEGORIES: &str = "/api/v2/product/get_category";
const ATTRIBUTES: &str = "/api/v2/product/get_attribute_tree";
const BRANDS: &str = "/api/v2/product/get_brand_list";
const CHANNELS: &str = "/api/v2/logistics/get_channel_list";
const UPLOAD: &str = "/api/v2/media_space/upload_image";
const ADD_ITEM: &str = "/api/v2/product/add_item";
const SEARCH: &str = "/api/v2/product/search_item";
const BASE_INFO: &str = "/api/v2/product/get_item_base_info";
const MODELS: &str = "/api/v2/product/get_model_list";
const INIT_TIERS: &str = "/api/v2/product/init_tier_variation";
const UNLIST: &str = "/api/v2/product/unlist_item";
const UPDATE_ITEM: &str = "/api/v2/product/update_item";

const CAMISETAS: &str = "100017";
const ITEM: &str = "3000142341";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/shopee-seller/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

fn value(id: &str, value: &str) -> AttributeValue {
    AttributeValue {
        id: id.into(),
        value: value.into(),
    }
}

/// HMAC-SHA256 of `base` with the partner key, lowercase hex.
fn sign(base: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(KEY.as_bytes()).unwrap();
    mac.update(base.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn public(path: &str) -> String {
    format!(
        "{path}?partner_id={PARTNER}&timestamp={NOW}&sign={}",
        sign(&format!("{PARTNER}{path}{NOW}"))
    )
}

/// A shop API call to `path`, then `query`.
fn shop(path: &str, query: &str) -> String {
    format!(
        "{path}?partner_id={PARTNER}&timestamp={NOW}&access_token={ACCESS}&shop_id={SHOP}&sign={}{query}",
        sign(&format!("{PARTNER}{path}{NOW}{ACCESS}{SHOP}"))
    )
}

fn brands(offset: u64, page_size: u32) -> String {
    shop(
        BRANDS,
        &format!(
            "&offset={offset}&page_size={page_size}&category_id={CAMISETAS}&status=1&language=pt-br"
        ),
    )
}

fn models() -> String {
    shop(MODELS, &format!("&item_id={ITEM}"))
}

/// Remembers each pause instead of sleeping.
#[derive(Default)]
struct RecordedPauses(Mutex<Vec<Duration>>);

impl Pause for RecordedPauses {
    fn pause(&self, duration: Duration) {
        self.0.lock().unwrap().push(duration);
    }
}

struct Fake {
    server: FakeHttpServer,
    pauses: Arc<RecordedPauses>,
    adapter: Arc<ShopeeSeller>,
}

impl Fake {
    /// The shop connected, with the T-shirts' category, two pages of
    /// brands, its shipping channels, and the item Shopee creates.
    fn connected() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        for (name, value) in [
            ("SHOPEE_PARTNER_ID", PARTNER),
            ("SHOPEE_PARTNER_KEY", KEY),
            ("SHOPEE_SHOP_ID", SHOP),
            ("SHOPEE_REFRESH_TOKEN", "3a4b4c4d4e4f50515253545556575859"),
        ] {
            store.write(name, &Secret::new(value)).unwrap();
        }
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
        ));
        let pauses = Arc::new(RecordedPauses::default());
        let adapter = Arc::new(
            ShopeeSeller::new(server.url(), "Mascate/test", store, clock)
                .with_pause(pauses.clone()),
        );
        let fake = Self {
            server,
            pauses,
            adapter,
        };
        fake.serve(&public(RENEW), "access-token-get.json");
        fake.server
            .serve_any_query(RECOMMEND, 200, fixture("category-recommend.json"));
        fake.serve(&shop(CATEGORIES, "&language=pt-br"), "categories.json");
        fake.serve(
            &shop(
                ATTRIBUTES,
                &format!("&category_id_list={CAMISETAS}&language=pt-BR"),
            ),
            "attribute-tree-camisetas.json",
        );
        fake.serve(&brands(0, 1), "brand-list.json");
        fake.serve(&brands(0, 100), "brand-list.json");
        fake.serve(&brands(100, 100), "brand-list-page-2.json");
        fake.serve(&shop(CHANNELS, ""), "channel-list.json");
        fake.serve(&shop(UPLOAD, ""), "upload-image.json");
        fake.serve(&shop(ADD_ITEM, ""), "add-item.json");
        fake.serve(
            &shop(SEARCH, "&page_size=100&item_sku=CAM-PRE-M"),
            "search-item-empty.json",
        );
        fake.serve(&models(), "model-list-empty.json");
        fake.serve(&shop(INIT_TIERS, ""), "init-tier-variation.json");
        fake.serve(&shop(UNLIST, ""), "update-ok.json");
        fake.serve(&shop(UPDATE_ITEM, ""), "update-item.json");
        fake
    }

    fn serve(&self, path: &str, name: &str) {
        self.server.serve(path, 200, fixture(name));
    }

    fn then(&self, path: &str, name: &str) {
        self.server.then_serve(path, 200, fixture(name));
    }

    /// The bodies sent to `path`, as JSON.
    fn sent_to(&self, path: &str) -> Vec<Value> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.path.starts_with(&format!("{path}?")))
            .map(|request| serde_json::from_str(&request.body).unwrap())
            .collect()
    }

    /// The bodies sent to `path`, as they went.
    fn raw_sent_to(&self, path: &str) -> Vec<String> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.path.starts_with(&format!("{path}?")))
            .map(|request| request.body)
            .collect()
    }

    fn calls_to(&self, path: &str) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|request| request.starts_with(&format!("{path}?")))
            .count()
    }

    fn pauses(&self) -> Vec<Duration> {
        self.pauses.0.lock().unwrap().clone()
    }
}

/// The T-shirt as it goes to Shopee, ready, without variations.
fn tshirt() -> ListingToPublish {
    ListingToPublish {
        title: "Camiseta Básica 100% Algodão".into(),
        description: "Malha 30.1, gola careca.".into(),
        category: CAMISETAS.into(),
        listing_type: ListingType::Classic,
        condition: Condition::New,
        price: brl("49.90"),
        available_quantity: 6,
        seller_sku: "CAM-PRE-M".into(),
        warranty: None,
        attributes: vec![
            value("brand", "malwee"),
            value("100134", "Algodão"),
            value("100200", "Casual"),
            value("100300", "160 g"),
            value("weight", "0,35"),
            value("gtin_code", "7891234567895"),
        ],
        pictures: vec![
            "br-11134207-7r98o-camiseta01".into(),
            "br-11134207-7r98o-camiseta02".into(),
            "br-11134207-7r98o-camiseta03".into(),
        ],
        variations: None,
    }
}

fn in_colours() -> ListingToPublish {
    ListingToPublish {
        variations: Some(VariationsToPublish {
            tier: "Cor".into(),
            options: vec![
                VariationToPublish {
                    option: "Preto".into(),
                    seller_sku: "CAM-PRE-M".into(),
                    available_quantity: 4,
                },
                VariationToPublish {
                    option: "Branco".into(),
                    seller_sku: "CAM-BRA-M".into(),
                    available_quantity: 2,
                },
            ],
        }),
        ..tshirt()
    }
}

fn with_attribute(listing: ListingToPublish, id: &str, typed: &str) -> ListingToPublish {
    let mut attributes: Vec<AttributeValue> = listing
        .attributes
        .into_iter()
        .filter(|attribute| attribute.id != id)
        .collect();
    attributes.push(value(id, typed));
    ListingToPublish {
        attributes,
        ..listing
    }
}

fn messages(issues: &[mascate_commerce::ChannelIssue]) -> Vec<(&str, bool)> {
    issues
        .iter()
        .map(|issue| (issue.message.as_str(), issue.blocks))
        .collect()
}

#[test]
fn the_categories_shopee_recommends_for_the_title_come_with_their_names() {
    let fake = Fake::connected();

    let predicted = fake
        .adapter
        .predict_categories("Camiseta básica preta algodão")
        .unwrap();

    let categories: Vec<ChannelCategory> = predicted.into_iter().map(|p| p.category).collect();
    assert_eq!(
        categories,
        [
            ChannelCategory {
                id: "100017".into(),
                name: "Camisetas".into()
            },
            ChannelCategory {
                id: "100636".into(),
                name: "Fones de Ouvido".into()
            },
        ]
    );
    let asked = fake
        .server
        .requests()
        .into_iter()
        .find(|path| path.starts_with(RECOMMEND))
        .unwrap();
    assert!(asked.contains("item_name=Camiseta"), "{asked}");
}

#[test]
fn a_category_asks_its_mandatory_attributes_with_the_brand_the_weight_and_the_gtin() {
    let fake = Fake::connected();

    let attributes = fake.adapter.category_attributes(CAMISETAS).unwrap();

    let attribute = |id: &str, name: &str, requirement| CategoryAttribute {
        id: id.into(),
        name: name.into(),
        requirement,
    };
    assert_eq!(
        attributes,
        [
            attribute("brand", "Marca", Requirement::Recommended),
            attribute("100134", "Material", Requirement::Required),
            attribute("100200", "Estilo", Requirement::Optional),
            attribute("100300", "Gramatura", Requirement::Optional),
            attribute("weight", "Peso com embalagem (kg)", Requirement::Required),
            attribute(
                "gtin_code",
                "GTIN (código de barras)",
                Requirement::Recommended
            ),
        ]
    );

    // Where Shopee requires a brand, so does the draft.
    fake.serve(&brands(0, 1), "brand-list-mandatory.json");
    let attributes = fake.adapter.category_attributes(CAMISETAS).unwrap();
    assert_eq!(attributes[0].requirement, Requirement::Required);
}

#[test]
fn a_category_id_that_is_not_shopees_never_reaches_the_api() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter.category_attributes("../order"),
        Err(PlatformError::NotFound)
    );
    assert_eq!(fake.calls_to(ATTRIBUTES), 0);
}

#[test]
fn a_picture_goes_up_to_the_media_space_as_a_multipart_image() {
    let fake = Fake::connected();

    let id = fake
        .adapter
        .upload_picture("frente.jpg", b"\xff\xd8\xff jpeg bytes")
        .unwrap();

    assert_eq!(id, "br-11134207-7r98o-camiseta01");
    let sent = fake
        .server
        .received()
        .into_iter()
        .find(|request| request.path.starts_with(UPLOAD))
        .unwrap();
    let content_type = sent
        .headers
        .iter()
        .find(|(name, _)| name == "content-type")
        .map(|(_, value)| value.clone())
        .unwrap();
    assert!(content_type.starts_with("multipart/form-data; boundary="));
    assert!(
        sent.body
            .contains(r#"Content-Disposition: form-data; name="image"; filename="frente.jpg""#)
    );
    assert!(sent.body.contains("Content-Type: image/jpeg"));
    assert!(
        sent.bytes
            .windows(b"\xff\xd8\xff jpeg bytes".len())
            .any(|window| window == b"\xff\xd8\xff jpeg bytes")
    );
}

#[test]
fn a_ready_listing_has_nothing_shopee_would_turn_down_and_nothing_is_created() {
    let fake = Fake::connected();

    assert_eq!(fake.adapter.validate(&tshirt()).unwrap(), []);
    assert_eq!(fake.calls_to(ADD_ITEM), 0);
}

#[test]
fn what_shopee_would_turn_down_blocks_the_listing_in_its_own_words() {
    let fake = Fake::connected();
    fake.serve(&shop(CHANNELS, ""), "channel-list-none.json");
    let mut listing = ListingToPublish {
        description: " ".into(),
        pictures: (0..10).map(|at| format!("foto-{at}")).collect(),
        ..tshirt()
    };
    for (id, typed) in [
        ("brand", "Marca Inventada"),
        ("100134", "Seda"),
        ("100300", "160"),
        ("weight", "leve"),
        ("gtin_code", "7891234567890"),
    ] {
        listing = with_attribute(listing, id, typed);
    }

    let issues = fake.adapter.validate(&listing).unwrap();

    assert_eq!(
        messages(&issues),
        [
            ("A Shopee pede uma descrição.", true),
            ("A Shopee aceita até 9 fotos; marque no máximo 9.", true),
            ("O peso leve não é um número de kg; digite como 0,35.", true),
            (
                "O GTIN 7891234567890 não confere: são 8, 12, 13 ou 14 números do código de \
                 barras.",
                true
            ),
            (
                "Em Material, a Shopee aceita só valores da lista dela, como Algodão, \
                 Poliéster.",
                true
            ),
            (
                "Em Gramatura, digite o número e a unidade, como 500 g.",
                true
            ),
            (
                "A marca Marca Inventada não está na lista da Shopee para esta categoria. \
                 Escreva como a Shopee lista, ou deixe a marca vazia para anunciar sem marca.",
                true
            ),
            (
                "Nenhum canal de envio ligado na loja da Shopee leva este anúncio. Ligue um em \
                 Minha loja › Configurações de envio, no Seller Center, e valide de novo.",
                true
            ),
        ]
    );
}

#[test]
fn a_brand_shopee_requires_cannot_be_left_out() {
    let fake = Fake::connected();
    fake.serve(&brands(0, 100), "brand-list-mandatory.json");

    let issues = fake
        .adapter
        .validate(&with_attribute(tshirt(), "brand", ""))
        .unwrap();

    assert_eq!(
        messages(&issues),
        [(
            "A Shopee pede a marca nesta categoria; escreva como ela aparece na lista de marcas \
             da Shopee.",
            true
        )]
    );
}

#[test]
fn a_heavy_item_goes_only_by_the_channels_that_carry_its_weight() {
    let fake = Fake::connected();

    fake.adapter
        .publish(&with_attribute(tshirt(), "weight", "2"))
        .unwrap();

    let body = &fake.sent_to(ADD_ITEM)[0];
    assert_eq!(
        body["logistic_info"],
        json!([{"logistic_id": 90001, "enabled": true}])
    );
}

#[test]
fn the_item_goes_out_once_with_shopees_ids_its_decimal_price_and_the_shops_channels() {
    let fake = Fake::connected();

    let published = fake.adapter.publish(&tshirt()).unwrap();

    assert_eq!(
        published,
        PublishedListing {
            id: ITEM.into(),
            status: ListingStatus::Active,
            link: Some(format!("https://shopee.com.br/product/{SHOP}/{ITEM}")),
        }
    );
    assert_eq!(fake.calls_to(ADD_ITEM), 1);
    let raw = &fake.raw_sent_to(ADD_ITEM)[0];
    assert!(raw.contains(r#""original_price":49.90,"#), "{raw}");
    assert!(raw.contains(r#""weight":0.35,"#), "{raw}");
    let body: Value = serde_json::from_str(raw).unwrap();
    assert_eq!(
        body,
        json!({
            "item_name": "Camiseta Básica 100% Algodão",
            "description": "Malha 30.1, gola careca.",
            "original_price": 49.90,
            "weight": 0.35,
            "category_id": 100017,
            "condition": "NEW",
            "item_sku": "CAM-PRE-M",
            "item_status": "NORMAL",
            "image": {"image_id_list": [
                "br-11134207-7r98o-camiseta01",
                "br-11134207-7r98o-camiseta02",
                "br-11134207-7r98o-camiseta03"
            ]},
            "logistic_info": [
                {"logistic_id": 90001, "enabled": true},
                {"logistic_id": 90005, "enabled": true}
            ],
            "attribute_list": [
                {"attribute_id": 100134, "attribute_value_list": [
                    {"value_id": 3001, "original_value_name": "Cotton"}
                ]},
                {"attribute_id": 100200, "attribute_value_list": [
                    {"value_id": 4001, "original_value_name": "Casual"}
                ]},
                {"attribute_id": 100300, "attribute_value_list": [
                    {"value_id": 0, "original_value_name": "160", "value_unit": "g"}
                ]}
            ],
            "brand": {"brand_id": 1200, "original_brand_name": "Malwee"},
            "seller_stock": [{"stock": 6}],
            "gtin_code": "7891234567895"
        })
    );
}

#[test]
fn a_used_item_without_a_brand_goes_as_shopees_no_brand() {
    let fake = Fake::connected();

    fake.adapter
        .publish(&ListingToPublish {
            condition: Condition::Used,
            ..with_attribute(tshirt(), "brand", "Sem marca")
        })
        .unwrap();

    let body = &fake.sent_to(ADD_ITEM)[0];
    assert_eq!(body["condition"], "USED");
    assert_eq!(
        body["brand"],
        json!({"brand_id": 0, "original_brand_name": "NoBrand"})
    );
}

#[test]
fn shopees_refusal_says_why_and_a_lost_answer_is_never_sent_again() {
    let fake = Fake::connected();
    fake.serve(&shop(ADD_ITEM, ""), "add-item-refused.json");

    assert_eq!(
        fake.adapter.publish(&tshirt()),
        Err(PlatformError::Refused("Invalid Weight.".into()))
    );

    fake.server.hang_up(&shop(ADD_ITEM, ""));
    assert!(matches!(
        fake.adapter.publish(&tshirt()),
        Err(PlatformError::Failed(_))
    ));
    assert_eq!(fake.calls_to(ADD_ITEM), 2);
}

#[test]
fn a_listing_shopee_would_turn_down_is_never_sent() {
    let fake = Fake::connected();

    let refused = fake
        .adapter
        .publish(&with_attribute(tshirt(), "100134", "Seda"));

    assert!(matches!(refused, Err(PlatformError::Refused(why)) if why.starts_with("Em Material")));
    assert_eq!(fake.calls_to(ADD_ITEM), 0);
}

#[test]
fn an_item_with_variations_waits_unlisted_then_gets_its_models_once_and_goes_on_sale() {
    let fake = Fake::connected();
    fake.serve(&shop(ADD_ITEM, ""), "add-item-unlisted.json");
    fake.then(&models(), "model-list-new.json");

    let published = fake.adapter.publish(&in_colours()).unwrap();
    let variations = fake.adapter.add_variations(ITEM, &in_colours()).unwrap();

    assert_eq!(published.status, ListingStatus::Paused);
    assert_eq!(fake.sent_to(ADD_ITEM)[0]["item_status"], "UNLIST");
    assert_eq!(
        fake.sent_to(ADD_ITEM)[0]["seller_stock"],
        json!([{"stock": 6}])
    );
    assert_eq!(fake.pauses(), [Duration::from_secs(5)]);
    let raw = &fake.raw_sent_to(INIT_TIERS)[0];
    assert!(raw.contains(r#""original_price":49.90,"#), "{raw}");
    assert_eq!(
        serde_json::from_str::<Value>(raw).unwrap(),
        json!({
            "item_id": 3000142341_u64,
            "standardise_tier_variation": [{
                "variation_id": 0,
                "variation_group_id": 0,
                "variation_name": "Cor",
                "variation_option_list": [
                    {"variation_option_id": 0, "variation_option_name": "Preto"},
                    {"variation_option_id": 0, "variation_option_name": "Branco"}
                ]
            }],
            "model": [
                {"tier_index": [0], "original_price": 49.90, "model_sku": "CAM-PRE-M",
                 "seller_stock": [{"stock": 4}]},
                {"tier_index": [1], "original_price": 49.90, "model_sku": "CAM-BRA-M",
                 "seller_stock": [{"stock": 2}]}
            ]
        })
    );
    assert_eq!(
        fake.sent_to(UNLIST),
        [json!({"item_list": [{"item_id": 3000142341_u64, "unlist": false}]})]
    );
    assert_eq!(
        variations,
        [
            PublishedVariation {
                id: "910001".into(),
                seller_sku: "CAM-PRE-M".into(),
                name: "Cor: Preto".into(),
                status: ListingStatus::Active,
            },
            PublishedVariation {
                id: "910002".into(),
                seller_sku: "CAM-BRA-M".into(),
                name: "Cor: Branco".into(),
                status: ListingStatus::Active,
            },
        ]
    );
}

#[test]
fn an_item_that_has_its_models_already_keeps_them_and_only_goes_on_sale() {
    let fake = Fake::connected();
    fake.serve(&models(), "model-list-new.json");

    let variations = fake.adapter.add_variations(ITEM, &in_colours()).unwrap();

    assert_eq!(variations.len(), 2);
    assert_eq!(fake.calls_to(INIT_TIERS), 0);
    assert!(fake.pauses().is_empty());
    assert_eq!(fake.calls_to(UNLIST), 1);
}

#[test]
fn models_whose_answer_was_lost_are_never_sent_twice_by_the_adapter() {
    let fake = Fake::connected();
    fake.server.hang_up(&shop(INIT_TIERS, ""));

    assert!(matches!(
        fake.adapter.add_variations(ITEM, &in_colours()),
        Err(PlatformError::Failed(_))
    ));
    assert_eq!(fake.calls_to(INIT_TIERS), 1);
    assert_eq!(fake.calls_to(UNLIST), 0);
}

#[test]
fn the_seller_sku_finds_only_the_items_with_exactly_that_sku() {
    let fake = Fake::connected();
    fake.serve(
        &shop(SEARCH, "&page_size=100&item_sku=CAM-PRE-M"),
        "search-item.json",
    );
    fake.serve(
        &shop(BASE_INFO, "&item_id_list=3000142341%2C3000142399"),
        "published-base-info.json",
    );

    assert_eq!(
        fake.adapter.find_by_seller_sku("CAM-PRE-M").unwrap(),
        [PublishedListing {
            id: ITEM.into(),
            status: ListingStatus::Active,
            link: Some(format!("https://shopee.com.br/product/{SHOP}/{ITEM}")),
        }]
    );
}

#[test]
fn the_description_goes_by_updating_the_item() {
    let fake = Fake::connected();

    fake.adapter
        .describe(ITEM, "Malha 30.1, gola careca.")
        .unwrap();

    assert_eq!(
        fake.sent_to(UPDATE_ITEM),
        [json!({"item_id": 3000142341_u64, "description": "Malha 30.1, gola careca."})]
    );
}

/// A commerce draft of the black T-shirt on Shopee, with its pictures in a
/// temporary folder.
struct Shop {
    dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    listings: Listings,
    fake: Fake,
}

impl Shop {
    fn open() -> Self {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let clock = Arc::new(ManualClock::at(
                Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
            ));
            let database = Arc::new(
                Database::open(&dir.path().join("mascate.db"))
                    .await
                    .unwrap(),
            );
            migrate(&database, clock.as_ref(), &[mascate_commerce::MIGRATIONS])
                .await
                .unwrap();
            Self {
                listings: Listings::new(
                    database,
                    clock.clone(),
                    Arc::new(SequentialIds::default()),
                ),
                dir,
                clock,
                fake: Fake::connected(),
            }
        })
    }

    fn pictures(&self) -> Vec<std::path::PathBuf> {
        let folder = self.dir.path().join("CAM-PRE-M");
        std::fs::create_dir_all(&folder).unwrap();
        ["frente.jpg", "costas.jpg", "gola.png"]
            .iter()
            .map(|name| {
                let path = folder.join(name);
                std::fs::write(&path, name.as_bytes()).unwrap();
                path
            })
            .collect()
    }

    /// The draft as Shopee suggests it, filled in and saved.
    async fn ready(&self, variations: Vec<DraftVariation>) -> ListingDraft {
        let draft = self
            .listings
            .create_draft(
                &CatalogProduct {
                    id: RecordId::from_u128(1),
                    sku: "CAM-PRE-M".into(),
                    name: "Camiseta Básica Preta M".into(),
                },
                DraftStart {
                    available_quantity: 4,
                    pictures: self.pictures(),
                },
                self.fake.adapter.as_ref(),
            )
            .await
            .unwrap();
        self.clock.advance(TimeDelta::minutes(1));
        self.listings
            .save_draft(
                draft.id,
                DraftEdit {
                    title: "Camiseta Básica 100% Algodão".into(),
                    description: "Malha 30.1, gola careca.".into(),
                    listing_type: draft.listing_type,
                    condition: Condition::New,
                    price: brl("49.90"),
                    available_quantity: 4,
                    warranty: String::new(),
                    attributes: vec![
                        value("brand", "Malwee"),
                        value("100134", "Algodão"),
                        value("weight", "0,35"),
                    ],
                    pictures: draft.pictures.iter().map(|p| p.path.clone()).collect(),
                    variation_tier: "Cor".into(),
                    variations,
                },
            )
            .await
            .unwrap()
    }
}

fn colours() -> Vec<DraftVariation> {
    vec![
        DraftVariation {
            product: RecordId::from_u128(1),
            seller_sku: "CAM-PRE-M".into(),
            option: "Preto".into(),
            available_quantity: 4,
        },
        DraftVariation {
            product: RecordId::from_u128(2),
            seller_sku: "CAM-BRA-M".into(),
            option: "Branco".into(),
            available_quantity: 2,
        },
    ]
}

#[test]
fn a_draft_starts_in_the_category_shopee_recommends_with_what_it_asks() {
    let store = Shop::open();
    block_on(async {
        let draft = store.ready(Vec::new()).await;

        assert_eq!(
            draft.category,
            Some(ChannelCategory {
                id: CAMISETAS.into(),
                name: "Camisetas".into()
            })
        );
        let ids: Vec<&str> = draft.attributes.iter().map(|a| a.id.as_str()).collect();
        // The optional ones stay out until they have a value.
        assert_eq!(ids, ["brand", "100134", "weight", "gtin_code"]);
        assert!(draft.checklist().iter().all(|item| !item.blocks));
    });
}

#[test]
fn a_draft_without_variations_publishes_its_pictures_once_and_its_item_once() {
    let store = Shop::open();
    block_on(async {
        let draft = store.ready(Vec::new()).await;

        let checklist = store
            .listings
            .validate_draft(draft.id, store.fake.adapter.as_ref())
            .await
            .unwrap();
        assert!(checklist.iter().all(|item| !item.blocks), "{checklist:?}");
        store.clock.advance(TimeDelta::minutes(1));
        let listing = store
            .listings
            .publish(draft.id, store.fake.adapter.as_ref())
            .await
            .unwrap();

        assert_eq!(store.fake.calls_to(UPLOAD), 3);
        assert_eq!(store.fake.calls_to(ADD_ITEM), 1);
        assert_eq!(listing.listed.id, ITEM);
        assert_eq!(listing.listed.status, ListingStatus::Active);
        assert_eq!(listing.product, Some(RecordId::from_u128(1)));
        assert_eq!(listing.listed.variation, None);
        assert_eq!(store.fake.calls_to(INIT_TIERS), 0);
        assert_eq!(store.fake.sent_to(UPDATE_ITEM).len(), 1);
        assert!(store.listings.drafts().await.unwrap().is_empty());
    });
}

#[test]
fn a_draft_with_variations_publishes_one_item_whose_models_are_each_products_listing() {
    let store = Shop::open();
    store
        .fake
        .serve(&shop(ADD_ITEM, ""), "add-item-unlisted.json");
    store.fake.then(&models(), "model-list-new.json");
    block_on(async {
        let draft = store.ready(colours()).await;

        store
            .listings
            .publish(draft.id, store.fake.adapter.as_ref())
            .await
            .unwrap();

        assert_eq!(store.fake.calls_to(ADD_ITEM), 1);
        assert_eq!(store.fake.calls_to(INIT_TIERS), 1);
        let mut listings: Vec<_> = store
            .listings
            .listings()
            .await
            .unwrap()
            .into_iter()
            .map(|listing| {
                (
                    listing.product,
                    listing.listed.variation.map(|v| (v.id, v.name)),
                    listing.listed.available_quantity,
                    listing.listed.status,
                )
            })
            .collect();
        listings.sort_by_key(|(product, ..)| *product);
        assert_eq!(
            listings,
            [
                (
                    Some(RecordId::from_u128(1)),
                    Some(("910001".to_owned(), "Cor: Preto".to_owned())),
                    4,
                    ListingStatus::Active
                ),
                (
                    Some(RecordId::from_u128(2)),
                    Some(("910002".to_owned(), "Cor: Branco".to_owned())),
                    2,
                    ListingStatus::Active
                ),
            ]
        );
    });
}

#[test]
fn a_lost_add_item_answer_is_adopted_by_the_retry_through_the_sku_search() {
    let store = Shop::open();
    store.fake.server.hang_up(&shop(ADD_ITEM, ""));
    block_on(async {
        let draft = store.ready(Vec::new()).await;
        assert!(matches!(
            store
                .listings
                .publish(draft.id, store.fake.adapter.as_ref())
                .await,
            Err(ListingError::Platform(PlatformError::Failed(_)))
        ));
        // Shopee had created it: the search by SKU finds it.
        store.fake.serve(
            &shop(SEARCH, "&page_size=100&item_sku=CAM-PRE-M"),
            "search-item.json",
        );
        store.fake.serve(
            &shop(BASE_INFO, "&item_id_list=3000142341%2C3000142399"),
            "published-base-info.json",
        );
        store.clock.advance(TimeDelta::minutes(1));

        let listing = store
            .listings
            .publish(draft.id, store.fake.adapter.as_ref())
            .await
            .unwrap();

        assert_eq!(store.fake.calls_to(ADD_ITEM), 1);
        assert_eq!(listing.listed.id, ITEM);
        assert_eq!(store.fake.calls_to(UPLOAD), 3);
        assert_eq!(store.listings.listings().await.unwrap().len(), 1);
    });
}

#[test]
fn shopees_validation_errors_show_as_checklist_errors() {
    let store = Shop::open();
    store
        .fake
        .serve(&shop(ADD_ITEM, ""), "add-item-refused.json");
    block_on(async {
        let draft = store.ready(Vec::new()).await;

        match store
            .listings
            .publish(draft.id, store.fake.adapter.as_ref())
            .await
        {
            Err(ListingError::Blocked(checklist)) => {
                assert!(
                    checklist.iter().any(|item| item.blocks
                        && item.check == Check::Channel("Invalid Weight.".into())),
                    "{checklist:?}"
                )
            }
            other => panic!("expected the checklist, got {other:?}"),
        }

        // And the adapter's own check, before anything is created.
        store
            .fake
            .serve(&shop(CHANNELS, ""), "channel-list-none.json");
        let checklist = store
            .listings
            .validate_draft(draft.id, store.fake.adapter.as_ref())
            .await
            .unwrap();
        assert!(checklist.iter().any(|item| item.blocks
            && matches!(&item.check, Check::Channel(message)
                if message.starts_with("Nenhum canal de envio"))));
    });
}
