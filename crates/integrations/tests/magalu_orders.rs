//! The owner's Magalu Orders and their Fees through the Open API adapter:
//! the adapter against the fake server serving the answers in
//! `fixtures/magalu`, written from the documentation (see its README).

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use mascate_commerce::{
    BilledOrder, Buyer, ChannelBilling, ChannelFee, ChannelOrder, ChannelOrderLine, ChannelOrders,
    ChannelReturn, FeeKind, OrderStatus, Receiver, ReturnStatus, ReturnedItem, Shipment,
    ShipmentStatus,
};
use mascate_integrations::{Magalu, Pause};
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{Currency, ListingType, Money, PlatformError, Timestamp};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};
use rust_decimal::Decimal;

const ORDERS: &str = "/seller/v1/orders";
const ANALYSIS: &str = "/seller/v1/financial-analysis/orders";

const WAITING: &str = "1502070665817670";
const DELIVERED: &str = "1502070665817671";
const CANCELLED: &str = "1502070665817672";
const NEW: &str = "1502070665817673";
const PARTLY: &str = "1502070665817674";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/magalu/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

fn at(text: &str) -> Timestamp {
    DateTime::parse_from_rfc3339(text).unwrap().to_utc()
}

/// 2026-10-16 14:00 UTC, the fake clock's time.
fn now() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 16, 14, 0, 0).unwrap()
}

/// The time as the adapter puts it in a query.
fn query_time(at: Timestamp) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
        .replace(':', "%3A")
        .replace('+', "%2B")
}

/// The Orders search for a span, at `offset`.
fn search(from: Timestamp, to: Timestamp, offset: usize) -> String {
    format!(
        "{ORDERS}?updated_at__gte={}&updated_at__lte={}&_offset={offset}&_limit=100&_sort=purchased_at%3Aasc",
        query_time(from),
        query_time(to)
    )
}

fn analysis_of(order: &str) -> String {
    format!("{ANALYSIS}?order_id={order}&_offset=0&_limit=100")
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
    adapter: Magalu,
}

impl Fake {
    /// The store's login stored, at 14:00.
    fn connected() -> Self {
        let server = FakeHttpServer::start();
        server.serve("/oauth/token", 200, fixture("token.json"));
        let store = Arc::new(MemorySecretStore::default());
        for (name, value) in [
            ("MAGALU_CLIENT_ID", "mascate-client-id"),
            ("MAGALU_CLIENT_SECRET", "mascate-client-secret"),
            ("MAGALU_REFRESH_TOKEN", "primeiro-refresh"),
        ] {
            store.write(name, &Secret::new(value)).unwrap();
        }
        let pauses = Arc::new(RecordedPauses::default());
        let adapter = Magalu::new(
            server.url(),
            server.url(),
            "Mascate/test",
            store,
            Arc::new(ManualClock::at(now())),
        )
        .with_pause(pauses.clone());
        Self {
            server,
            pauses,
            adapter,
        }
    }

    /// Every search in the span from `since` answers the fixture `name`.
    fn selling(since: Timestamp, name: &str) -> Self {
        let fake = Self::connected();
        fake.server
            .serve(&search(since, now(), 0), 200, fixture(name));
        fake
    }

    fn calls_to(&self, path: &str) -> Vec<String> {
        self.server
            .requests()
            .into_iter()
            .filter(|request| request.starts_with(&format!("{path}?")))
            .collect()
    }

    fn pauses(&self) -> Vec<u64> {
        self.pauses
            .0
            .lock()
            .unwrap()
            .iter()
            .map(Duration::as_secs)
            .collect()
    }
}

fn since() -> Timestamp {
    now() - TimeDelta::hours(2)
}

fn sold() -> (Fake, Vec<ChannelOrder>) {
    let fake = Fake::selling(since(), "orders-page.json");
    let orders = fake.adapter.orders_changed_since(since()).unwrap();
    (fake, orders)
}

fn found<'a>(orders: &'a [ChannelOrder], id: &str) -> &'a ChannelOrder {
    orders
        .iter()
        .find(|order| order.id == id)
        .unwrap_or_else(|| panic!("no Order {id}"))
}

fn line(sku: &str, title: &str, quantity: u32, unit: &str, fee: &str) -> ChannelOrderLine {
    ChannelOrderLine {
        item: sku.into(),
        variation: None,
        title: title.into(),
        variation_name: None,
        quantity,
        unit_price: brl(unit),
        sale_fee: Some(brl(fee)),
        listing_type: Some(ListingType::default()),
    }
}

/// A page of `count` copies of the waiting Order, numbered from `first`.
fn full_page(first: usize, count: usize) -> String {
    let page: serde_json::Value = serde_json::from_str(&fixture("orders-page.json")).unwrap();
    let order = &page["results"][0];
    let results: Vec<serde_json::Value> = (first..first + count)
        .map(|number| {
            let mut copy = order.clone();
            copy["code"] = serde_json::Value::String(format!("15020706658{number:05}"));
            copy
        })
        .collect();
    serde_json::json!({ "meta": page["meta"], "results": results }).to_string()
}

#[test]
fn the_search_asks_from_since_until_now_a_hundred_at_a_time_by_purchase() {
    let (fake, orders) = sold();

    assert_eq!(fake.calls_to(ORDERS), [search(since(), now(), 0)]);
    let ids: Vec<&str> = orders.iter().map(|order| order.id.as_str()).collect();
    assert_eq!(ids, [WAITING, DELIVERED, CANCELLED, NEW]);
}

#[test]
fn an_order_comes_with_its_lines_its_money_its_receiver_and_its_time_to_ship() {
    let (_fake, orders) = sold();

    // The commission Magalu put on the Order is the provisional sale fee.
    let mut camiseta = line("CAM-PRETA-M", "Camiseta preta M", 1, "49.9", "14.98");
    camiseta.variation_name = Some("Cor: Preta, Tamanho: M".into());
    assert_eq!(
        *found(&orders, WAITING),
        ChannelOrder {
            id: WAITING.into(),
            pack: None,
            status: OrderStatus::Paid,
            ordered_at: at("2026-10-16T13:05:00Z"),
            updated_at: at("2026-10-16T13:06:00Z"),
            lines: vec![camiseta],
            total: brl("49.9"),
            paid: Some(brl("69.8")),
            shipping_paid: Some(brl("19.9")),
            refunded: None,
            buyer: Some(Buyer {
                nickname: None,
                receiver: Some(Receiver {
                    name: "Ana Souza".into(),
                    address: Some("Avenida Paulista, 1000, Apto 101, Bela Vista".into()),
                    city: Some("São Paulo".into()),
                    state: Some("SP".into()),
                    zip_code: Some("01310-100".into()),
                }),
            }),
            // Paid, the delivery waits for the store's NF-e.
            shipment: Some(Shipment {
                id: "6c764444-436d-4659-8cec-304414b05001".into(),
                status: ShipmentStatus::AwaitingInvoice,
                dispatch_by: Some(at("2026-10-18T00:00:00Z")),
                seller_cost: None,
            }),
            returns: Vec::new(),
        }
    );
    // Two units: the commission is split by unit.
    assert_eq!(
        found(&orders, DELIVERED).lines,
        [line("FONE-TWS", "Fone de ouvido TWS", 2, "89.9", "22.98")]
    );
}

#[test]
fn each_order_and_delivery_status_is_an_order_and_shipment_status() {
    let (_fake, orders) = sold();

    let statuses: Vec<(&str, OrderStatus, ShipmentStatus)> = orders
        .iter()
        .map(|order| {
            (
                order.id.as_str(),
                order.status,
                order.shipment.as_ref().unwrap().status,
            )
        })
        .collect();
    assert_eq!(
        statuses,
        [
            (WAITING, OrderStatus::Paid, ShipmentStatus::AwaitingInvoice),
            (DELIVERED, OrderStatus::Paid, ShipmentStatus::Delivered),
            // Cancelled before it was posted: its units go back by
            // themselves.
            (CANCELLED, OrderStatus::Cancelled, ShipmentStatus::Cancelled),
            (NEW, OrderStatus::AwaitingPayment, ShipmentStatus::Pending),
        ]
    );
}

#[test]
fn a_delivery_cancelled_in_an_order_that_goes_on_brings_its_units_back() {
    let fake = Fake::selling(since(), "orders-partly-cancelled.json");

    let orders = fake.adapter.orders_changed_since(since()).unwrap();

    let order = found(&orders, PARTLY);
    assert_eq!(order.status, OrderStatus::Paid);
    assert_eq!(order.lines.len(), 2);
    // The parcel that went is the Order's shipment.
    let shipment = order.shipment.as_ref().unwrap();
    assert_eq!(
        (shipment.id.as_str(), shipment.status),
        (
            "6c764444-436d-4659-8cec-304414b05005",
            ShipmentStatus::Delivered
        )
    );
    assert_eq!(
        order.returns,
        [ChannelReturn {
            id: "6c764444-436d-4659-8cec-304414b05006".into(),
            status: ReturnStatus::Delivered,
            items: vec![ReturnedItem {
                item: "CANECA-300".into(),
                variation: None,
                quantity: 1,
            }],
        }]
    );
    assert_eq!(order.refunded, Some(brl("35")));
}

#[test]
fn a_full_page_asks_for_the_next() {
    let fake = Fake::connected();
    fake.server
        .serve(&search(since(), now(), 0), 200, full_page(0, 100));
    fake.server.serve(
        &search(since(), now(), 100),
        200,
        fixture("orders-empty.json"),
    );

    let orders = fake.adapter.orders_changed_since(since()).unwrap();

    assert_eq!(orders.len(), 100);
    assert_eq!(
        fake.calls_to(ORDERS),
        [search(since(), now(), 0), search(since(), now(), 100)]
    );
}

#[test]
fn a_span_with_more_than_magalu_pages_through_is_read_again_in_halves() {
    let fake = Fake::connected();
    for offset in (0..5_000).step_by(100) {
        fake.server
            .serve(&search(since(), now(), offset), 200, full_page(offset, 100));
    }
    let middle = since() + TimeDelta::hours(1);
    fake.server
        .serve(&search(since(), middle, 0), 200, full_page(0, 60));
    fake.server
        .serve(&search(middle, now(), 0), 200, full_page(5_000, 40));

    let orders = fake.adapter.orders_changed_since(since()).unwrap();

    assert_eq!(fake.calls_to(ORDERS).len(), 52);
    assert_eq!(
        fake.calls_to(ORDERS)[50..],
        [search(since(), middle, 0), search(middle, now(), 0)]
    );
    // The halves bring what the 5,000 left out; none twice.
    assert_eq!(orders.len(), 5_040);
}

#[test]
fn the_same_order_in_both_halves_comes_once_as_it_was_last_changed() {
    let fake = Fake::connected();
    for offset in (0..5_000).step_by(100) {
        fake.server
            .serve(&search(since(), now(), offset), 200, full_page(offset, 100));
    }
    let middle = since() + TimeDelta::hours(1);
    let page = fixture("orders-page.json");
    let later = page.replace(
        r#""updated_at": "2026-10-16T13:06:00+00:00""#,
        r#""updated_at": "2026-10-16T13:56:00+00:00""#,
    );
    let changed = later.replace(r#""status": "approved""#, r#""status": "finished""#);
    fake.server.serve(&search(since(), middle, 0), 200, page);
    fake.server.serve(&search(middle, now(), 0), 200, changed);

    let orders = fake.adapter.orders_changed_since(since()).unwrap();

    assert_eq!(orders.iter().filter(|order| order.id == WAITING).count(), 1);
    assert_eq!(
        found(&orders, WAITING).updated_at,
        at("2026-10-16T13:56:00Z")
    );
}

#[test]
fn too_many_requests_waits_and_then_reads_the_orders() {
    let fake = Fake::connected();
    let path = search(since(), now(), 0);
    fake.server.serve(&path, 429, fixture("error-429.json"));
    fake.server
        .then_serve(&path, 200, fixture("orders-page.json"));

    let orders = fake.adapter.orders_changed_since(since()).unwrap();

    assert_eq!(orders.len(), 4);
    assert_eq!(fake.pauses().len(), 1);
}

#[test]
fn a_search_magalu_finds_invalid_is_refused_with_its_words() {
    let fake = Fake::connected();
    fake.server
        .serve(&search(since(), now(), 0), 422, fixture("error-422.json"));

    let refused = fake.adapter.orders_changed_since(since());

    assert!(matches!(refused, Err(PlatformError::Refused(_))));
}

#[test]
fn the_fees_of_a_delivered_order_come_from_its_financial_analysis() {
    let fake = Fake::connected();
    fake.server.serve(
        &analysis_of(DELIVERED),
        200,
        fixture("financial-analysis-delivered.json"),
    );
    fake.server.serve(
        &analysis_of(WAITING),
        200,
        fixture("financial-analysis-empty.json"),
    );

    let billed = fake
        .adapter
        .billed_fees(&[DELIVERED.into(), WAITING.into()])
        .unwrap();

    let fee = |id: &str, kind: FeeKind, description: &str, amount: &str| ChannelFee {
        id: id.into(),
        kind,
        description: description.into(),
        amount: brl(amount),
    };
    // The sale, the freight the buyer paid and the ICMS for information
    // are no Fees; what Magalu covers of the freight comes off.
    assert_eq!(
        billed,
        [BilledOrder {
            order: DELIVERED.into(),
            fees: vec![
                fee(
                    "COMMISSION:SERVICE:commission:ITEM:i-78431:DEBIT",
                    FeeKind::SaleFee,
                    "Comissão",
                    "35.96"
                ),
                fee(
                    "FEES:PLATFORM:fixed_fee:ITEM:i-78431:DEBIT",
                    FeeKind::SaleFee,
                    "Tarifa fixa",
                    "5"
                ),
                fee(
                    "FEES:PLATFORM:fixed_fee:ITEM:i-78431:DEBIT#2",
                    FeeKind::SaleFee,
                    "Tarifa fixa",
                    "5"
                ),
                fee(
                    "FEES:PAYMENT_PROCESSING:mdr_fee:ORDER:1502070665817671:DEBIT",
                    FeeKind::Other,
                    "Taxa de pagamento",
                    "3.6"
                ),
                fee(
                    "SHIPPING_COST:FREIGHT:freight:SHIPPING:6c764444-436d-4659-8cec-304414b05002:DEBIT",
                    FeeKind::Shipping,
                    "Frete",
                    "24.9"
                ),
                fee(
                    "SHIPPING_SUBSIDY:FREIGHT:freight_subsidy:SHIPPING:6c764444-436d-4659-8cec-304414b05002:CREDIT",
                    FeeKind::Shipping,
                    "Frete pago pela Magalu",
                    "-10"
                ),
            ],
        }]
    );
    // Asked again, the same Fees by the same ids.
    assert_eq!(
        fake.adapter.billed_fees(&[DELIVERED.into()]).unwrap(),
        billed
    );
}

#[test]
fn an_order_number_that_is_not_magalus_is_never_asked_for() {
    let fake = Fake::connected();

    let billed = fake
        .adapter
        .billed_fees(&["1502&_limit=1".into(), String::new()])
        .unwrap();

    assert!(billed.is_empty());
    assert!(fake.calls_to(ANALYSIS).is_empty());
}

#[test]
fn a_bad_window_magalu_refuses_is_its_refusal_with_its_words() {
    let fake = Fake::connected();
    fake.server.serve(
        &analysis_of(DELIVERED),
        422,
        fixture("error-422-window.json"),
    );

    let refused = fake.adapter.billed_fees(&[DELIVERED.into()]);

    let Err(PlatformError::Refused(why)) = refused else {
        panic!("{refused:?}");
    };
    assert!(why.contains("Informe order_id ou um par completo de datas"));
}

#[test]
fn a_throttled_analysis_waits_and_tries_again() {
    let fake = Fake::connected();
    let path = analysis_of(DELIVERED);
    fake.server.serve(&path, 429, fixture("error-429.json"));
    fake.server
        .then_serve(&path, 200, fixture("financial-analysis-delivered.json"));

    let billed = fake.adapter.billed_fees(&[DELIVERED.into()]).unwrap();

    assert_eq!(billed[0].fees.len(), 6);
    assert_eq!(fake.pauses().len(), 1);
}

#[test]
fn an_analysis_naming_its_order_by_a_number_or_another_order_is_read_as_such() {
    let fake = Fake::connected();
    let delivered = fixture("financial-analysis-delivered.json");
    let as_number = delivered.replace(
        r#""order_code": "1502070665817671""#,
        r#""order_code": 1502070665817671"#,
    );
    fake.server.serve(&analysis_of(DELIVERED), 200, as_number);
    // An analysis of another Order is not this one's.
    fake.server.serve(&analysis_of(WAITING), 200, delivered);

    let billed = fake
        .adapter
        .billed_fees(&[DELIVERED.into(), WAITING.into()])
        .unwrap();

    let orders: Vec<&str> = billed.iter().map(|found| found.order.as_str()).collect();
    assert_eq!(orders, [DELIVERED]);
    assert_eq!(billed[0].fees.len(), 6);
}
