//! Drafts offered on Amazon through the Selling Partner API adapter (#128):
//! the catalog searched by GTIN, the offer previewed and sent with
//! `putListingsItem`, and commerce's draft end to end on a real temporary
//! database, against the fake server serving the answers in
//! `fixtures/amazon-seller`, written from the documentation (see its
//! README).

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    AttributeValue, CatalogItem, CatalogMatch, CatalogProduct, ChannelCategory, ChannelIssue,
    Check, Condition, DraftEdit, DraftStart, Gtin, ListingDraft, ListingError, ListingPublisher,
    ListingStatus, ListingToPublish, Listings, PublishedListing,
};
use mascate_integrations::{Amazon, Pause};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, ListingType, Money, PlatformError, RecordId};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;
use serde_json::json;

const TOKEN: &str = "/auth/o2/token";
const LOOKUP: &str = "/listings/2021-08-01/items/A1MASCATE0SELLER/FON-TWS-001\
                      ?marketplaceIds=A2Q3Y263D00KWC\
                      &includedData=summaries%2CfulfillmentAvailability";
const PUT: &str = "/listings/2021-08-01/items/A1MASCATE0SELLER/FON-TWS-001\
                   ?marketplaceIds=A2Q3Y263D00KWC&issueLocale=pt_BR";
const PREVIEW: &str = "/listings/2021-08-01/items/A1MASCATE0SELLER/FON-TWS-001\
                       ?marketplaceIds=A2Q3Y263D00KWC&issueLocale=pt_BR\
                       &mode=VALIDATION_PREVIEW";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/amazon-seller/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

/// The catalog search for `gtin`, which Amazon names `kind`.
fn search(gtin: &str, kind: &str) -> String {
    format!(
        "/catalog/2022-04-01/items?identifiers={gtin}&identifiersType={kind}\
         &marketplaceIds=A2Q3Y263D00KWC&includedData=summaries%2CproductTypes&locale=pt_BR\
         &pageSize=20"
    )
}

/// Skips the waits between retries.
struct NoPause;

impl Pause for NoPause {
    fn pause(&self, _duration: Duration) {}
}

struct Fake {
    server: FakeHttpServer,
    adapter: Amazon,
}

impl Fake {
    /// The owner's keys pasted, Amazon with no listing of the SKU yet.
    fn connected() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        for (name, value) in [
            (
                "AMAZON_LWA_CLIENT_ID",
                "amzn1.application-oa2-client.mascate",
            ),
            ("AMAZON_LWA_CLIENT_SECRET", "amzn1.oa2-cs.v1.s3cr3t"),
            ("AMAZON_REFRESH_TOKEN", "Atzr|IwEBIA-refresh-token"),
            ("AMAZON_SELLER_ID", "A1MASCATE0SELLER"),
        ] {
            store.write(name, &Secret::new(value)).unwrap();
        }
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
        ));
        let adapter = Amazon::new(server.url(), server.url(), "Mascate/test", store, clock)
            .with_pause(Arc::new(NoPause));
        let fake = Self { server, adapter };
        fake.serve(TOKEN, "token.json");
        fake.server.serve(LOOKUP, 404, fixture("error-404.json"));
        fake.serve(PREVIEW, "put-preview-valid.json");
        fake.serve(PUT, "put-accepted.json");
        fake.serve(&search("7891234567895", "EAN"), "catalog-one.json");
        fake.serve(&search("7891000000014", "EAN"), "catalog-several.json");
        fake.serve(&search("17891234567892", "GTIN"), "catalog-empty.json");
        fake.serve(&search("012345678905", "UPC"), "catalog-empty.json");
        fake.serve(&search("78912342", "EAN"), "catalog-empty.json");
        fake
    }

    fn serve(&self, path: &str, name: &str) {
        self.server.serve(path, 200, fixture(name));
    }

    /// The requests to `path`, by method.
    fn sent(&self, method: &str, path: &str) -> Vec<serde_json::Value> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.method == method && request.path == path)
            .map(|request| serde_json::from_str(&request.body).unwrap_or_default())
            .collect()
    }
}

fn gtin(digits: &str) -> Gtin {
    Gtin::parse(digits).unwrap()
}

fn fone_item() -> CatalogItem {
    CatalogItem {
        id: "B0CFONE001".into(),
        title: "Fone de Ouvido Bluetooth Mascate Preto".into(),
        brand: Some("Mascate".into()),
        category: ChannelCategory {
            id: "HEADPHONES".into(),
            name: "Fones de ouvido intra-auriculares".into(),
        },
        attributes: vec![AttributeValue {
            id: "merchant_suggested_asin".into(),
            value: "B0CFONE001".into(),
        }],
        link: Some("https://www.amazon.com.br/dp/B0CFONE001".into()),
    }
}

fn offer() -> ListingToPublish {
    ListingToPublish {
        title: "Fone de Ouvido Bluetooth Mascate Preto".into(),
        category: "HEADPHONES".into(),
        listing_type: ListingType::Classic,
        condition: Condition::New,
        price: brl("129.90"),
        available_quantity: 4,
        seller_sku: "FON-TWS-001".into(),
        warranty: None,
        attributes: fone_item().attributes,
        pictures: Vec::new(),
    }
}

/// `offer()` as `putListingsItem` takes it.
fn offer_body() -> serde_json::Value {
    json!({
        "productType": "HEADPHONES",
        "requirements": "LISTING_OFFER_ONLY",
        "attributes": {
            "purchasable_offer": [{
                "marketplace_id": "A2Q3Y263D00KWC",
                "currency": "BRL",
                "audience": "ALL",
                "our_price": [{ "schedule": [{ "value_with_tax": 129.90 }] }],
            }],
            "merchant_suggested_asin": [
                { "value": "B0CFONE001", "marketplace_id": "A2Q3Y263D00KWC" }
            ],
            "condition_type": [{ "value": "new_new", "marketplace_id": "A2Q3Y263D00KWC" }],
            "fulfillment_availability": [
                { "fulfillment_channel_code": "DEFAULT", "quantity": 4 }
            ],
        },
    })
}

#[test]
fn the_catalog_is_searched_by_the_gtin_as_the_kind_amazon_names_it() {
    let fake = Fake::connected();

    let found = fake.adapter.catalog_items(&gtin("7891234567895")).unwrap();

    assert_eq!(found, vec![fone_item()]);
    for (digits, kind) in [
        ("012345678905", "UPC"),
        ("17891234567892", "GTIN"),
        ("78912342", "EAN"),
    ] {
        assert_eq!(fake.adapter.catalog_items(&gtin(digits)).unwrap(), []);
        assert!(fake.server.requests().contains(&search(digits, kind)));
    }
}

#[test]
fn a_product_without_a_product_type_cannot_take_an_offer_and_is_left_out() {
    let fake = Fake::connected();

    let found = fake.adapter.catalog_items(&gtin("7891000000014")).unwrap();

    let ids: Vec<&str> = found.iter().map(|item| item.id.as_str()).collect();
    assert_eq!(ids, ["B0CCANECA1", "B0CCANECA2"]);
    // Without a brand or a classification, the product type names it.
    assert_eq!(found[1].brand, None);
    assert_eq!(found[1].category.name, "DRINKING_CUP");
}

#[test]
fn an_offer_carries_only_its_asin_and_the_drafts_own_fields() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter
            .category_attributes("HEADPHONES")
            .unwrap()
            .iter()
            .map(|attribute| attribute.id.as_str())
            .collect::<Vec<_>>(),
        ["merchant_suggested_asin"]
    );
    assert_eq!(fake.adapter.predict_categories("Fone").unwrap(), []);
    assert_eq!(
        fake.adapter.category_attributes("../orders"),
        Err(PlatformError::NotFound)
    );
}

#[test]
fn the_preview_sends_the_offer_and_creates_nothing() {
    let fake = Fake::connected();

    let issues = fake.adapter.validate(&offer()).unwrap();

    assert_eq!(
        issues,
        [ChannelIssue {
            message: "A oferta não pode ser a oferta em destaque porque o preço não é competitivo."
                .into(),
            blocks: false,
        }]
    );
    assert_eq!(fake.sent("PUT", PREVIEW), [offer_body()]);
    assert!(fake.sent("PUT", PUT).is_empty());
}

#[test]
fn the_price_goes_as_its_decimal_text_and_a_used_item_in_good_shape() {
    let fake = Fake::connected();

    fake.adapter
        .validate(&ListingToPublish {
            price: brl("89.905"),
            condition: Condition::Used,
            ..offer()
        })
        .unwrap();

    let body = &fake
        .server
        .received()
        .into_iter()
        .find(|request| request.path == PREVIEW)
        .unwrap()
        .body;
    assert!(body.contains(r#""value_with_tax":89.91"#), "{body}");
    assert!(body.contains(r#""value":"used_good""#), "{body}");
}

#[test]
fn amazons_errors_block_the_offer_and_its_warnings_do_not() {
    let fake = Fake::connected();
    fake.serve(PREVIEW, "put-invalid.json");

    let issues = fake.adapter.validate(&offer()).unwrap();

    assert_eq!(
        issues.iter().map(|issue| issue.blocks).collect::<Vec<_>>(),
        [true, false]
    );
    assert_eq!(
        issues[0].message,
        "Você não tem aprovação para vender nesta marca. Peça a aprovação no Seller Central."
    );
}

#[test]
fn publishing_sends_the_offer_once_amazon_has_no_listing_with_the_sku() {
    let fake = Fake::connected();

    let published = fake.adapter.publish(&offer()).unwrap();

    assert_eq!(
        published,
        PublishedListing {
            id: "FON-TWS-001".into(),
            // Amazon applies it within minutes; a Sync reads its status.
            status: ListingStatus::UnderReview,
            link: Some("https://www.amazon.com.br/dp/B0CFONE001".into()),
        }
    );
    let requests = fake.server.requests();
    let lookup = requests.iter().position(|path| path == LOOKUP).unwrap();
    let put = requests.iter().position(|path| path == PUT).unwrap();
    assert!(lookup < put);
    assert_eq!(fake.sent("PUT", PUT), [offer_body()]);
}

#[test]
fn a_listing_amazon_already_has_with_the_sku_is_never_replaced() {
    let fake = Fake::connected();
    fake.serve(LOOKUP, "listing-item.json");

    let refused = fake.adapter.publish(&offer());

    assert!(matches!(refused, Err(PlatformError::Refused(_))));
    assert!(fake.sent("PUT", PUT).is_empty());
}

#[test]
fn an_offer_amazon_finds_invalid_is_refused_with_its_errors() {
    let fake = Fake::connected();
    fake.serve(PUT, "put-invalid.json");

    let refused = fake.adapter.publish(&offer());

    assert_eq!(
        refused,
        Err(PlatformError::Refused(
            "Você não tem aprovação para vender nesta marca. Peça a aprovação no Seller Central."
                .into()
        ))
    );
}

#[test]
fn the_seller_sku_finds_the_listing_amazon_has_or_none() {
    let fake = Fake::connected();

    assert_eq!(fake.adapter.find_by_seller_sku("FON-TWS-001").unwrap(), []);

    fake.serve(LOOKUP, "listing-item.json");
    assert_eq!(
        fake.adapter.find_by_seller_sku("FON-TWS-001").unwrap(),
        [PublishedListing {
            id: "FON-TWS-001".into(),
            status: ListingStatus::Active,
            link: Some("https://www.amazon.com.br/dp/B0CFONE001".into()),
        }]
    );
}

#[test]
fn an_offer_has_no_pictures_or_description_of_its_own() {
    let fake = Fake::connected();

    assert!(matches!(
        fake.adapter.upload_picture("frente.jpg", b"jpg"),
        Err(PlatformError::Refused(_))
    ));
    fake.adapter.describe("FON-TWS-001", "Texto").unwrap();
    assert!(fake.server.requests().iter().all(|path| path == TOKEN));
}

#[test]
fn a_draft_offers_the_product_its_gtin_finds_and_goes_up_once() {
    block_on(async {
        let fake = Fake::connected();
        let drafted = Drafted::new(&fake.adapter).await;

        let matched = drafted
            .listings
            .match_catalog_item(drafted.draft.id, "7891234567895", &fake.adapter)
            .await
            .unwrap();
        let CatalogMatch::Offered(offered) = matched else {
            panic!("one product is offered at once: {matched:?}");
        };
        assert_eq!(offered.title, "Fone de Ouvido Bluetooth Mascate Preto");
        let ready = drafted.ready(&offered).await;
        assert_eq!(ready.checklist(), []);

        let checklist = drafted
            .listings
            .validate_draft(ready.id, &fake.adapter)
            .await
            .unwrap();
        assert_eq!(
            checklist.iter().map(|item| item.blocks).collect::<Vec<_>>(),
            [false]
        );
        let listing = drafted
            .listings
            .publish(ready.id, &fake.adapter)
            .await
            .unwrap();

        assert_eq!(listing.listed.id, "FON-TWS-001");
        assert_eq!(listing.product, Some(RecordId::from_u128(7)));
        assert_eq!(fake.sent("PUT", PUT).len(), 1);
    });
}

#[test]
fn amazons_issues_show_in_the_checklist_and_hold_the_offer_back() {
    block_on(async {
        let fake = Fake::connected();
        fake.serve(PREVIEW, "put-invalid.json");
        let drafted = Drafted::new(&fake.adapter).await;
        let offered = drafted
            .listings
            .offer_catalog_item(drafted.draft.id, fone_item(), &fake.adapter)
            .await
            .unwrap();
        let ready = drafted.ready(&offered).await;

        let refused = drafted.listings.publish(ready.id, &fake.adapter).await;

        let Err(ListingError::Blocked(checklist)) = refused else {
            panic!("Amazon's error blocks: {refused:?}");
        };
        assert_eq!(
            checklist[0].check,
            Check::Channel(
                "Você não tem aprovação para vender nesta marca. Peça a aprovação no Seller \
                 Central."
                    .into()
            )
        );
        assert!(checklist[0].blocks);
        assert!(fake.sent("PUT", PUT).is_empty());
    });
}

#[test]
fn a_lost_answer_is_followed_by_adopting_the_offer_the_first_attempt_created() {
    block_on(async {
        let fake = Fake::connected();
        let drafted = Drafted::new(&fake.adapter).await;
        let offered = drafted
            .listings
            .offer_catalog_item(drafted.draft.id, fone_item(), &fake.adapter)
            .await
            .unwrap();
        let ready = drafted.ready(&offered).await;
        // The offer reaches Amazon, but every answer is lost on the way.
        fake.server.hang_up(PUT);

        let lost = drafted.listings.publish(ready.id, &fake.adapter).await;
        assert!(matches!(
            lost,
            Err(ListingError::Platform(PlatformError::Failed(_)))
        ));
        let sent = fake.sent("PUT", PUT).len();

        // Amazon now lists the offer by its SKU.
        fake.server.serve(LOOKUP, 200, fixture("listing-item.json"));
        drafted.clock.advance(TimeDelta::minutes(5));
        let listing = drafted
            .listings
            .publish(ready.id, &fake.adapter)
            .await
            .unwrap();

        assert_eq!(listing.listed.id, "FON-TWS-001");
        assert_eq!(listing.listed.status, ListingStatus::Active);
        assert_eq!(fake.sent("PUT", PUT).len(), sent, "nothing sent again");
        assert_eq!(drafted.listings.drafts().await.unwrap(), []);
    });
}

/// A draft on Amazon of the earphones, on a real temporary database.
struct Drafted {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    listings: Listings,
    draft: ListingDraft,
}

impl Drafted {
    async fn new(adapter: &Amazon) -> Self {
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
        let listings = Listings::new(database, clock.clone(), Arc::new(SequentialIds::default()));
        let draft = listings
            .create_draft(
                &CatalogProduct {
                    id: RecordId::from_u128(7),
                    sku: "FON-TWS-001".into(),
                    name: "Fone Bluetooth TWS".into(),
                },
                DraftStart {
                    available_quantity: 4,
                    pictures: Vec::new(),
                },
                adapter,
            )
            .await
            .unwrap();
        Self {
            _dir: dir,
            clock,
            listings,
            draft,
        }
    }

    /// `draft` with the price the owner confirms.
    async fn ready(&self, draft: &ListingDraft) -> ListingDraft {
        self.clock.advance(TimeDelta::minutes(1));
        self.listings
            .save_draft(
                draft.id,
                DraftEdit {
                    title: draft.title.clone(),
                    description: String::new(),
                    listing_type: draft.listing_type,
                    condition: Condition::New,
                    price: brl("129.90"),
                    available_quantity: 4,
                    warranty: String::new(),
                    attributes: Vec::new(),
                    pictures: Vec::new(),
                },
            )
            .await
            .unwrap()
    }
}
