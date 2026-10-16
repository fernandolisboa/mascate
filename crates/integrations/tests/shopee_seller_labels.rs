//! Arranging a Shopee Order's shipment and printing its label through the
//! Open Platform v2 seller adapter: the adapter against the fake server
//! serving the answers in `fixtures/shopee-seller`, written from the
//! documentation (see its README).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use hmac::{Hmac, Mac};
use mascate_commerce::{LabelError, ShippingLabels};
use mascate_integrations::ShopeeSeller;
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{Pause, PlatformError};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};
use serde_json::{Value, json};
use sha2::Sha256;

const PARTNER: &str = "2001887";
const KEY: &str = "shpk-mascate-partner-key";
const SHOP: &str = "14701711";
/// 2026-10-16 12:00 UTC, the fake clock's time.
const NOW: i64 = 1_792_152_000;
const ACCESS: &str = "59777174636562737266615546704c6d";

const RENEW: &str = "/api/v2/auth/access_token/get";
const ORDER_DETAIL: &str = "/api/v2/order/get_order_detail";
const PACKAGE_DETAIL: &str = "/api/v2/order/get_package_detail";
const SHIPPING_PARAMETER: &str = "/api/v2/logistics/get_shipping_parameter";
const SHIP_ORDER: &str = "/api/v2/logistics/ship_order";
const TRACKING_NUMBER: &str = "/api/v2/logistics/get_tracking_number";
const DOCUMENT_PARAMETER: &str = "/api/v2/logistics/get_shipping_document_parameter";
const CREATE_DOCUMENT: &str = "/api/v2/logistics/create_shipping_document";
const DOCUMENT_RESULT: &str = "/api/v2/logistics/get_shipping_document_result";
const DOWNLOAD_DOCUMENT: &str = "/api/v2/logistics/download_shipping_document";

const ORDER: &str = "261016ETQ0006F";
const PACKAGE: &str = "OFG1610000000061";
const TRACKING: &str = "BR266123456789X";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/shopee-seller/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn label_pdf() -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/shopee-seller/shipping-document.pdf",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
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

fn order_detail() -> String {
    shop(
        ORDER_DETAIL,
        &format!("&order_sn_list={ORDER}&response_optional_fields=package_list"),
    )
}

fn package_detail() -> String {
    shop(PACKAGE_DETAIL, &format!("&package_number_list={PACKAGE}"))
}

fn of_the_package(path: &str) -> String {
    shop(path, &format!("&order_sn={ORDER}&package_number={PACKAGE}"))
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
    /// The shop connected, an Order ready to ship whose parcel Shopee
    /// arranges as a drop-off, gives a tracking number on the second read
    /// and makes the label on the second read.
    fn ready_to_ship() -> Self {
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
        fake.serve(&order_detail(), "label-order-detail.json");
        fake.serve(&package_detail(), "package-detail-ready.json");
        fake.then(&package_detail(), "package-detail-arranged.json");
        fake.serve(
            &of_the_package(SHIPPING_PARAMETER),
            "shipping-parameter.json",
        );
        fake.serve(&shop(SHIP_ORDER, ""), "ship-order.json");
        fake.serve(
            &of_the_package(TRACKING_NUMBER),
            "tracking-number-empty.json",
        );
        fake.then(&of_the_package(TRACKING_NUMBER), "tracking-number.json");
        fake.serve(&shop(DOCUMENT_PARAMETER, ""), "document-parameter.json");
        fake.serve(&shop(CREATE_DOCUMENT, ""), "document-created.json");
        fake.serve(&shop(DOCUMENT_RESULT, ""), "document-processing.json");
        fake.then(&shop(DOCUMENT_RESULT, ""), "document-ready.json");
        fake.server
            .serve(&shop(DOWNLOAD_DOCUMENT, ""), 200, label_pdf());
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

#[test]
fn the_label_arranges_a_drop_off_waits_for_the_tracking_number_and_the_document_then_comes_as_a_pdf()
 {
    let fake = Fake::ready_to_ship();

    let pdf = fake.adapter.label_pdf(ORDER).unwrap();

    assert_eq!(pdf, label_pdf());
    // A drop-off needs nothing: the owner takes the parcel at their time.
    assert_eq!(
        fake.sent_to(SHIP_ORDER),
        [json!({ "order_sn": ORDER, "package_number": PACKAGE, "dropoff": {} })]
    );
    // The tracking number and the document came on the second read.
    assert_eq!(fake.calls_to(TRACKING_NUMBER), 2);
    assert_eq!(fake.calls_to(DOCUMENT_RESULT), 2);
    assert_eq!(fake.pauses(), [Duration::from_secs(3); 2]);
    assert_eq!(
        fake.sent_to(CREATE_DOCUMENT),
        [json!({ "order_list": [{
            "order_sn": ORDER,
            "package_number": PACKAGE,
            "tracking_number": TRACKING,
            "shipping_document_type": "THERMAL_AIR_WAYBILL",
        }] })]
    );
    assert_eq!(
        fake.sent_to(DOWNLOAD_DOCUMENT),
        [json!({
            "shipping_document_type": "THERMAL_AIR_WAYBILL",
            "order_list": [{ "order_sn": ORDER, "package_number": PACKAGE }],
        })]
    );
}

#[test]
fn without_a_plain_drop_off_the_parcel_goes_by_pickup_at_the_pickup_address_in_the_recommended_slot()
 {
    let fake = Fake::ready_to_ship();
    fake.serve(
        &of_the_package(SHIPPING_PARAMETER),
        "shipping-parameter-pickup.json",
    );

    fake.adapter.label_pdf(ORDER).unwrap();

    assert_eq!(
        fake.sent_to(SHIP_ORDER),
        [json!({
            "order_sn": ORDER,
            "package_number": PACKAGE,
            "pickup": { "address_id": 235, "pickup_time_id": "1792324800" },
        })]
    );
}

#[test]
fn a_lost_answer_to_ship_order_is_read_back_and_the_parcel_is_never_arranged_twice() {
    let fake = Fake::ready_to_ship();
    // Shopee arranged it, but the answer never came back.
    fake.server.hang_up(&shop(SHIP_ORDER, ""));
    fake.serve(&package_detail(), "package-detail-ready.json");
    fake.then(&package_detail(), "package-detail-arranging.json");

    let pdf = fake.adapter.label_pdf(ORDER).unwrap();

    assert_eq!(pdf, label_pdf());
    assert_eq!(fake.calls_to(SHIP_ORDER), 1);
    assert_eq!(fake.calls_to(PACKAGE_DETAIL), 2);
}

#[test]
fn a_flow_stopped_halfway_goes_on_from_there_without_arranging_again() {
    let fake = Fake::ready_to_ship();
    // The carrier takes longer than the app waits.
    fake.serve(
        &of_the_package(TRACKING_NUMBER),
        "tracking-number-empty.json",
    );

    assert!(matches!(
        fake.adapter.label_pdf(ORDER),
        Err(LabelError::NotReadyYet)
    ));
    assert_eq!(fake.calls_to(SHIP_ORDER), 1);
    assert_eq!(fake.calls_to(CREATE_DOCUMENT), 0);

    // The owner asks again later: the parcel is arranged, the number is in.
    fake.serve(&of_the_package(TRACKING_NUMBER), "tracking-number.json");
    let pdf = fake.adapter.label_pdf(ORDER).unwrap();

    assert_eq!(pdf, label_pdf());
    assert_eq!(fake.calls_to(SHIP_ORDER), 1);
    assert_eq!(fake.calls_to(SHIPPING_PARAMETER), 1);
}

#[test]
fn a_document_still_in_the_making_is_read_again_and_never_taken_as_failed() {
    let fake = Fake::ready_to_ship();
    fake.serve(&shop(DOCUMENT_RESULT, ""), "document-processing.json");

    assert!(matches!(
        fake.adapter.label_pdf(ORDER),
        Err(LabelError::NotReadyYet)
    ));
    assert_eq!(fake.calls_to(DOCUMENT_RESULT), 10);
    assert_eq!(fake.calls_to(DOWNLOAD_DOCUMENT), 0);

    // Ready on the next ask: the parcel is not arranged again.
    fake.serve(&shop(DOCUMENT_RESULT, ""), "document-ready.json");
    assert_eq!(fake.adapter.label_pdf(ORDER).unwrap(), label_pdf());
    assert_eq!(fake.calls_to(SHIP_ORDER), 1);
}

#[test]
fn a_document_shopee_failed_to_make_is_refused() {
    let fake = Fake::ready_to_ship();
    fake.serve(&shop(DOCUMENT_RESULT, ""), "document-failed.json");

    assert!(matches!(
        fake.adapter.label_pdf(ORDER),
        Err(LabelError::Platform(PlatformError::Refused(_)))
    ));
    assert_eq!(fake.calls_to(DOWNLOAD_DOCUMENT), 0);
}

#[test]
fn an_order_waiting_for_its_nfe_says_so_and_ships_nothing() {
    let fake = Fake::ready_to_ship();
    fake.serve(&order_detail(), "label-order-detail-invoice.json");

    assert!(matches!(
        fake.adapter.label_pdf(ORDER),
        Err(LabelError::AwaitingInvoice)
    ));
    assert_eq!(fake.calls_to(PACKAGE_DETAIL), 0);
    assert_eq!(fake.calls_to(SHIP_ORDER), 0);
}

#[test]
fn a_parcel_that_needs_details_the_app_does_not_fill_in_is_arranged_on_shopee() {
    let fake = Fake::ready_to_ship();
    fake.serve(
        &of_the_package(SHIPPING_PARAMETER),
        "shipping-parameter-branch.json",
    );

    assert!(matches!(
        fake.adapter.label_pdf(ORDER),
        Err(LabelError::ArrangeOnChannel)
    ));
    assert_eq!(fake.calls_to(SHIP_ORDER), 0);

    // Arranged in the Seller Center: the label prints.
    fake.serve(&package_detail(), "package-detail-arranged.json");
    assert_eq!(fake.adapter.label_pdf(ORDER).unwrap(), label_pdf());
    assert_eq!(fake.calls_to(SHIP_ORDER), 0);
}

#[test]
fn an_order_that_already_left_has_no_label_and_an_odd_number_never_reaches_shopee() {
    let fake = Fake::ready_to_ship();
    fake.serve(&order_detail(), "label-order-detail-shipped.json");

    assert!(matches!(
        fake.adapter.label_pdf(ORDER),
        Err(LabelError::NotWaiting)
    ));
    assert_eq!(fake.calls_to(SHIP_ORDER), 0);

    let calls = fake.server.requests().len();
    assert!(matches!(
        fake.adapter.label_pdf("2610&order_sn_list=X"),
        Err(LabelError::Platform(PlatformError::NotFound))
    ));
    assert_eq!(fake.server.requests().len(), calls);
}

#[test]
fn a_document_shopee_answers_with_an_error_is_not_taken_for_the_label() {
    let fake = Fake::ready_to_ship();
    fake.serve(&shop(DOWNLOAD_DOCUMENT, ""), "error-limit.json");

    assert!(matches!(
        fake.adapter.label_pdf(ORDER),
        Err(LabelError::Platform(PlatformError::RateLimited))
    ));
}

#[test]
fn ship_order_is_asked_again_only_while_shopee_shows_the_parcel_not_arranged_and_then_gives_up() {
    let fake = Fake::ready_to_ship();
    fake.server.hang_up(&shop(SHIP_ORDER, ""));
    fake.serve(&package_detail(), "package-detail-ready.json");

    assert!(matches!(
        fake.adapter.label_pdf(ORDER),
        Err(LabelError::Platform(PlatformError::Failed(_)))
    ));
    // Each repeat came after a read that showed it still waiting.
    assert_eq!(fake.calls_to(SHIP_ORDER), 3);
    assert_eq!(fake.calls_to(PACKAGE_DETAIL), 3);
    assert_eq!(fake.calls_to(TRACKING_NUMBER), 0);
}
