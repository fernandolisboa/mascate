//! Drafts published through Mercado Livre's adapter: the adapter against
//! the fake server serving the answers in `fixtures/mercado-livre`, written
//! from the documentation (see its README), and commerce's drafts end to
//! end on a real temporary database.

use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    AttributeValue, CatalogProduct, CategoryAttribute, ChannelCategory, ChannelIssue, Condition,
    DraftEdit, DraftStart, ListingPublisher, ListingStatus, ListingToPublish, Listings,
    PublishedListing, Requirement,
};
use mascate_integrations::MercadoLivre;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, ListingType, Money, PlatformError, RecordId};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore, Request};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;
use serde_json::{Value, json};

const PREDICT: &str =
    "/sites/MLB/domain_discovery/search?limit=3&q=Fone%20Bluetooth%20TWS%20Lenovo";
const ATTRIBUTES: &str = "/categories/MLB196208/attributes";
const UPLOAD: &str = "/pictures/items/upload";
const DESCRIPTION: &str = "/items/MLB4200000001/description";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/mercado-livre/{name}",
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

struct Fake {
    server: FakeHttpServer,
    clock: Arc<ManualClock>,
    adapter: MercadoLivre,
}

impl Fake {
    /// A connected seller account where publishing works.
    fn connected() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        for (name, value) in [
            ("ML_CLIENT_ID", "1234567890"),
            ("ML_CLIENT_SECRET", "s3cr3t"),
            ("ML_REFRESH_TOKEN", "TG-first"),
        ] {
            store.write(name, &Secret::new(value)).unwrap();
        }
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
        ));
        let adapter = MercadoLivre::new(server.url(), "Mascate/test", store, clock.clone());
        let fake = Self {
            server,
            clock,
            adapter,
        };
        fake.serve("/oauth/token", "token.json");
        fake.serve("/users/me", "users-me.json");
        fake.serve(PREDICT, "domain-discovery.json");
        fake.serve(ATTRIBUTES, "category-attributes-MLB196208.json");
        fake.serve(UPLOAD, "picture-upload.json");
        fake.server.serve("/items/validate", 204, "");
        fake.server
            .serve_method("POST", "/items", 201, fixture("item-created.json"));
        fake.server
            .serve_method("POST", DESCRIPTION, 201, fixture("item-description.json"));
        fake
    }

    fn serve(&self, path: &str, name: &str) {
        self.server.serve(path, 200, fixture(name));
    }

    fn posted(&self, path: &str) -> Vec<Request> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.method == "POST" && request.path == path)
            .collect()
    }
}

fn earphones() -> ListingToPublish {
    ListingToPublish {
        title: "Fone de Ouvido Bluetooth TWS Lenovo LP40".into(),
        category: "MLB196208".into(),
        listing_type: ListingType::Premium,
        condition: Condition::New,
        price: brl("89.90"),
        available_quantity: 4,
        seller_sku: "FON-TWS-001".into(),
        warranty: Some("90 dias".into()),
        attributes: vec![value("BRAND", "Lenovo"), value("MODEL", "LP40")],
        pictures: vec!["123-MLB456_102026".into(), "789-MLB456_102026".into()],
    }
}

#[test]
fn the_predictor_suggests_categories_with_the_values_it_read_in_the_title() {
    let fake = Fake::connected();

    let predicted = fake
        .adapter
        .predict_categories("Fone Bluetooth TWS Lenovo")
        .unwrap();

    assert_eq!(predicted.len(), 2);
    assert_eq!(
        predicted[0].category,
        ChannelCategory {
            id: "MLB196208".into(),
            name: "Fones de Ouvido".into()
        }
    );
    assert_eq!(
        predicted[0].attributes,
        [value("BRAND", "Lenovo"), value("LINE", "LivePods")]
    );
    assert!(predicted[1].attributes.is_empty());
}

#[test]
fn a_categorys_attributes_say_which_are_required_and_which_only_recommended() {
    let fake = Fake::connected();

    let attributes = fake.adapter.category_attributes("MLB196208").unwrap();

    let attribute = |id: &str, name: &str, requirement| CategoryAttribute {
        id: id.into(),
        name: name.into(),
        requirement,
    };
    // SELLER_SKU and ITEM_CONDITION come from the draft itself; a read-only
    // one is not the seller's to fill.
    assert_eq!(
        attributes,
        [
            attribute("BRAND", "Marca", Requirement::Required),
            attribute("MODEL", "Modelo", Requirement::Required),
            attribute("LINE", "Linha", Requirement::Optional),
            attribute(
                "GTIN",
                "Código universal de produto",
                Requirement::Recommended
            ),
            attribute("COLOR", "Cor", Requirement::Optional),
            attribute("IS_WIRELESS", "É sem fio", Requirement::Recommended),
        ]
    );
}

#[test]
fn a_category_id_that_is_not_mercado_livres_never_reaches_the_api() {
    let fake = Fake::connected();

    let refused = fake.adapter.category_attributes("../users/me");

    assert_eq!(refused, Err(PlatformError::NotFound));
    assert!(fake.server.requests().iter().all(|p| !p.contains("users")));
}

#[test]
fn a_picture_goes_up_as_a_multipart_file() {
    let fake = Fake::connected();

    let id = fake
        .adapter
        .upload_picture("frente \"1\".PNG", b"\x89PNG fake bytes")
        .unwrap();

    assert_eq!(id, "123-MLB456_102026");
    let upload = &fake.posted(UPLOAD)[0];
    assert!(upload.authorization.is_some());
    let boundary = upload
        .body
        .lines()
        .next()
        .unwrap()
        .trim_start_matches("--")
        .to_owned();
    assert!(boundary.starts_with("mascate-"));
    assert_eq!(
        upload.body,
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; \
             filename=\"frente 1.PNG\"\r\nContent-Type: image/png\r\n\r\n\u{FFFD}PNG fake \
             bytes\r\n--{boundary}--\r\n"
        )
    );
}

#[test]
fn a_listing_the_validator_accepts_has_no_issues_and_nothing_is_created() {
    let fake = Fake::connected();

    let issues = fake.adapter.validate(&earphones()).unwrap();

    assert!(issues.is_empty());
    assert_eq!(fake.posted("/items/validate").len(), 1);
    assert!(fake.posted("/items").is_empty());
}

#[test]
fn the_validators_errors_block_and_its_warnings_do_not() {
    let fake = Fake::connected();
    fake.server
        .serve("/items/validate", 400, fixture("validate-errors.json"));

    let issues = fake.adapter.validate(&earphones()).unwrap();

    assert_eq!(issues.len(), 2);
    assert!(issues[0].blocks);
    assert!(
        issues[0]
            .message
            .starts_with("The attributes [MODEL] are required")
    );
    assert_eq!(
        issues[1],
        ChannelIssue {
            message: "The attribute GTIN is recommended for category MLB196208.".into(),
            blocks: false
        }
    );
}

#[test]
fn a_validation_error_without_causes_blocks_with_its_message() {
    let fake = Fake::connected();
    fake.server.serve(
        "/items/validate",
        400,
        r#"{"message":"body.invalid_fields","error":"bad_request","status":400,"cause":[]}"#,
    );

    let issues = fake.adapter.validate(&earphones()).unwrap();

    assert_eq!(
        issues,
        [ChannelIssue {
            message: "body.invalid_fields".into(),
            blocks: true
        }]
    );
}

#[test]
fn waiting_asked_by_the_validator_is_no_issue_of_the_listing() {
    let fake = Fake::connected();
    fake.server
        .serve("/items/validate", 429, fixture("error-429.json"));

    assert_eq!(
        fake.adapter.validate(&earphones()),
        Err(PlatformError::RateLimited)
    );
}

#[test]
fn a_new_listing_goes_out_with_its_price_in_cents_sku_warranty_and_pictures() {
    let fake = Fake::connected();

    let published = fake.adapter.publish(&earphones()).unwrap();

    assert_eq!(
        published,
        PublishedListing {
            id: "MLB4200000001".into(),
            status: ListingStatus::Active,
            link: Some(
                "https://produto.mercadolivre.com.br/MLB-4200000001-fone-de-ouvido-bluetooth-tws-\
                 lenovo-lp40-_JM"
                    .into()
            ),
        }
    );
    let sent = &fake.posted("/items")[0].body;
    // The price is the decimal itself, never a float.
    assert!(sent.starts_with(r#"{"price":89.90,"#), "{sent}");
    let body: Value = serde_json::from_str(sent).unwrap();
    assert_eq!(
        body,
        json!({
            "price": 89.90,
            "title": "Fone de Ouvido Bluetooth TWS Lenovo LP40",
            "site_id": "MLB",
            "category_id": "MLB196208",
            "currency_id": "BRL",
            "available_quantity": 4,
            "buying_mode": "buy_it_now",
            "condition": "new",
            "listing_type_id": "gold_pro",
            "pictures": [{ "id": "123-MLB456_102026" }, { "id": "789-MLB456_102026" }],
            "attributes": [
                { "id": "BRAND", "value_name": "Lenovo" },
                { "id": "MODEL", "value_name": "LP40" },
                { "id": "SELLER_SKU", "value_name": "FON-TWS-001" },
            ],
            "sale_terms": [
                { "id": "WARRANTY_TYPE", "value_name": "Garantia do vendedor" },
                { "id": "WARRANTY_TIME", "value_name": "90 dias" },
            ],
        })
    );
}

#[test]
fn a_user_products_seller_sends_the_title_as_the_family_name() {
    let fake = Fake::connected();
    fake.serve("/users/me", "users-me-user-products.json");
    let without_warranty = ListingToPublish {
        warranty: None,
        ..earphones()
    };

    fake.adapter.publish(&without_warranty).unwrap();

    let body: Value = serde_json::from_str(&fake.posted("/items")[0].body).unwrap();
    assert_eq!(
        body["family_name"],
        "Fone de Ouvido Bluetooth TWS Lenovo LP40"
    );
    assert!(body.get("title").is_none());
    assert!(body.get("sale_terms").is_none());
}

#[test]
fn a_listing_mercado_livre_refuses_says_why() {
    let fake = Fake::connected();
    fake.server
        .serve_method("POST", "/items", 400, fixture("error-400-price.json"));

    assert_eq!(
        fake.adapter.publish(&earphones()),
        Err(PlatformError::Refused(
            "The price is lower than the minimum allowed for the category".into()
        ))
    );
}

#[test]
fn the_description_is_posted_and_replaces_one_already_there() {
    let fake = Fake::connected();
    let text = "Fone sem fio.\nBateria de 4 horas.";

    fake.adapter.describe("MLB4200000001", text).unwrap();

    let posted = &fake.posted(DESCRIPTION)[0];
    let body: Value = serde_json::from_str(&posted.body).unwrap();
    assert_eq!(body, json!({ "plain_text": text }));

    fake.server.serve_method(
        "POST",
        DESCRIPTION,
        400,
        fixture("error-400-description.json"),
    );
    fake.server.serve_method(
        "PUT",
        "/items/MLB4200000001/description?api_version=2",
        200,
        fixture("item-description.json"),
    );
    fake.adapter.describe("MLB4200000001", text).unwrap();

    let put: Vec<_> = fake
        .server
        .received()
        .into_iter()
        .filter(|request| request.method == "PUT")
        .collect();
    assert_eq!(put.len(), 1);
    assert_eq!(
        put[0].path,
        "/items/MLB4200000001/description?api_version=2"
    );
    assert_eq!(put[0].body, posted.body);
}

#[test]
fn the_sellers_listings_with_a_sku_are_found_with_their_status() {
    let fake = Fake::connected();
    fake.serve(
        "/users/1234567/items/search?seller_sku=FON-TWS-001",
        "user-items-by-sku.json",
    );
    fake.serve(
        "/items?ids=MLB4200000001&attributes=id%2Cstatus%2Cpermalink",
        "items-states.json",
    );

    let found = fake.adapter.find_by_seller_sku("FON-TWS-001").unwrap();

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, "MLB4200000001");
    assert_eq!(found[0].status, ListingStatus::Active);
    fake.server.serve(
        "/users/1234567/items/search?seller_sku=NONE",
        200,
        r#"{"seller_id":"1234567","results":[],"paging":{"total":0}}"#,
    );
    assert!(fake.adapter.find_by_seller_sku("NONE").unwrap().is_empty());
}

#[test]
fn without_a_connection_nothing_is_published() {
    let server = FakeHttpServer::start();
    let clock = Arc::new(ManualClock::at(
        Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
    ));
    let adapter = MercadoLivre::new(
        server.url(),
        "Mascate/test",
        Arc::new(MemorySecretStore::default()),
        clock,
    );

    assert_eq!(
        adapter.publish(&earphones()),
        Err(PlatformError::NotConnected)
    );
    assert!(server.requests().is_empty());
}

#[test]
fn a_draft_from_a_product_is_published_on_mercado_livre_end_to_end() {
    block_on(async {
        let fake = Fake::connected();
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(Database::open(&dir.path().join("m.db")).await.unwrap());
        migrate(
            &database,
            fake.clock.as_ref(),
            &[mascate_commerce::MIGRATIONS],
        )
        .await
        .unwrap();
        let listings = Listings::new(
            database,
            fake.clock.clone(),
            Arc::new(SequentialIds::default()),
        );
        let pictures: Vec<PathBuf> = ["frente.jpg", "lado.jpg", "caixa.png"]
            .iter()
            .map(|name| {
                let path = dir.path().join(name);
                std::fs::write(&path, name.as_bytes()).unwrap();
                path
            })
            .collect();
        let product = CatalogProduct {
            id: RecordId::from_u128(7),
            sku: "FON-TWS-001".into(),
            name: "Fone Bluetooth TWS Lenovo".into(),
        };

        let draft = listings
            .create_draft(
                &product,
                DraftStart {
                    available_quantity: 4,
                    pictures: pictures.clone(),
                },
                &fake.adapter,
            )
            .await
            .unwrap();
        assert_eq!(draft.attributes[0].value, "Lenovo");
        let ready = listings
            .save_draft(
                draft.id,
                DraftEdit {
                    title: "Fone de Ouvido Bluetooth TWS Lenovo LP40".into(),
                    description: "Fone sem fio com estojo de carga.".into(),
                    listing_type: ListingType::Classic,
                    condition: Condition::New,
                    price: brl("89.90"),
                    available_quantity: 4,
                    warranty: String::new(),
                    attributes: vec![value("MODEL", "LP40")],
                    pictures,
                },
            )
            .await
            .unwrap();
        let listing = listings.publish(ready.id, &fake.adapter).await.unwrap();

        assert_eq!(listing.listed.id, "MLB4200000001");
        assert_eq!(listing.listed.status, ListingStatus::Active);
        assert_eq!(listing.product, Some(product.id));
        let order: Vec<String> = fake
            .server
            .received()
            .into_iter()
            .filter(|request| request.method == "POST" && request.path != "/oauth/token")
            .map(|request| request.path)
            .collect();
        assert_eq!(
            order,
            [
                UPLOAD,
                UPLOAD,
                UPLOAD,
                "/items/validate",
                "/items",
                DESCRIPTION
            ]
        );
        let body: Value = serde_json::from_str(&fake.posted("/items")[0].body).unwrap();
        assert_eq!(body["pictures"].as_array().unwrap().len(), 3);
        assert!(listings.drafts().await.unwrap().is_empty());
    });
}
