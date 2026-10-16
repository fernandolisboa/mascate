//! The owner's Amazon Orders, their Fees and their shipment through the
//! Selling Partner API adapter: the adapter against the fake server serving
//! the answers in `fixtures/amazon-seller`, written from the documentation
//! (see its README).

use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use mascate_commerce::{
    Buyer, ChannelBilling, ChannelFee, ChannelOrder, ChannelOrderLine, ChannelOrders,
    ChannelShipments, FeeKind, OrderStatus, Receiver, SentShipment, Shipment, ShipmentStatus,
};
use mascate_integrations::{Amazon, Pause};
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{Currency, ListingType, Money, PlatformError, Timestamp};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};
use rust_decimal::Decimal;

const TOKEN: &str = "/auth/o2/token";
const WITH_RECIPIENT: &str = "RECIPIENT%2CPROCEEDS%2CFULFILLMENT%2CPACKAGES";
const WITHOUT_RECIPIENT: &str = "PROCEEDS%2CFULFILLMENT%2CPACKAGES";
const AFTER: &str = "2026-10-16T11%3A50%3A00Z";
const PAGE_TOKEN: &str = "eyJwYWdlIjoyfQ";
const UNSHIPPED: &str = "701-1234567-1234567";
const DELIVERED: &str = "704-1234567-1234567";
const CONFIRM: &str = "/orders/v0/orders/701-1234567-1234567/shipmentConfirmation";

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

fn at(text: &str) -> Timestamp {
    chrono::DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

/// The search of the Orders the seller ships changed since 11:50, with
/// `included`, on the page `token`.
fn search(included: &str, token: Option<&str>) -> String {
    let mut path = format!(
        "/orders/2026-01-01/orders?marketplaceIds=A2Q3Y263D00KWC&fulfilledBy=MERCHANT\
         &lastUpdatedAfter={AFTER}&includedData={included}&maxResultsPerPage=100"
    );
    if let Some(token) = token {
        path.push_str(&format!("&paginationToken={token}"));
    }
    path
}

fn transactions(order: &str, token: Option<&str>) -> String {
    let mut path = format!(
        "/finances/2024-06-19/transactions?relatedIdentifierName=ORDER_ID\
         &relatedIdentifierValue={order}&marketplaceId=A2Q3Y263D00KWC"
    );
    if let Some(token) = token {
        path.push_str(&format!("&nextToken={token}"));
    }
    path
}

fn get_order(order: &str) -> String {
    format!("/orders/2026-01-01/orders/{order}?includedData=FULFILLMENT%2CPACKAGES")
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
    adapter: Amazon,
}

impl Fake {
    /// The owner's keys pasted, on 2026-10-16 12:00 UTC, and two pages of
    /// Orders.
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
        let pauses = Arc::new(RecordedPauses::default());
        let adapter = Amazon::new(server.url(), server.url(), "Mascate/test", store, clock)
            .with_pause(pauses.clone());
        let fake = Self {
            server,
            pauses,
            adapter,
        };
        fake.serve(TOKEN, "token.json");
        fake.serve(&search(WITH_RECIPIENT, None), "orders-page-1.json");
        fake.serve(
            &search(WITH_RECIPIENT, Some(PAGE_TOKEN)),
            "orders-page-2.json",
        );
        fake
    }

    fn serve(&self, path: &str, name: &str) {
        self.server.serve(path, 200, fixture(name));
    }

    fn orders(&self) -> Result<Vec<ChannelOrder>, PlatformError> {
        self.adapter
            .orders_changed_since(at("2026-10-16T11:50:00Z"))
    }

    /// The requests but the token's, in order.
    fn calls(&self) -> Vec<String> {
        self.server
            .requests()
            .into_iter()
            .filter(|path| path != TOKEN)
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

fn found<'a>(orders: &'a [ChannelOrder], id: &str) -> &'a ChannelOrder {
    orders
        .iter()
        .find(|order| order.id == id)
        .unwrap_or_else(|| panic!("no Order {id}"))
}

fn fone_line(quantity: u32, unit_price: &str, sale_fee: &str) -> ChannelOrderLine {
    ChannelOrderLine {
        item: "FONE-BT-001".into(),
        variation: None,
        title: "Fone de Ouvido Bluetooth Mascate Preto".into(),
        variation_name: None,
        quantity,
        unit_price: brl(unit_price),
        sale_fee: Some(brl(sale_fee)),
        listing_type: Some(ListingType::Classic),
    }
}

fn caneca_line() -> ChannelOrderLine {
    ChannelOrderLine {
        item: "CANECA-01".into(),
        title: "Caneca de Cerâmica 350 ml".into(),
        // Amazon's table: 15% of R$ 39,90.
        ..fone_line(1, "39.90", "5.985")
    }
}

#[test]
fn the_orders_the_seller_ships_are_read_page_by_page_with_the_buyers_address() {
    let fake = Fake::connected();

    let orders = fake.orders().unwrap();

    assert_eq!(
        fake.calls(),
        [
            search(WITH_RECIPIENT, None),
            search(WITH_RECIPIENT, Some(PAGE_TOKEN))
        ]
    );
    // The first Order comes again on the second page: once.
    let ids: Vec<&str> = orders.iter().map(|order| order.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            UNSHIPPED,
            "702-1234567-1234567",
            "703-1234567-1234567",
            DELIVERED,
            "705-1234567-1234567"
        ]
    );
    let unshipped = found(&orders, UNSHIPPED);
    assert_eq!(
        *unshipped,
        ChannelOrder {
            id: UNSHIPPED.into(),
            pack: None,
            status: OrderStatus::Paid,
            ordered_at: at("2026-10-16T10:15:00Z"),
            updated_at: at("2026-10-16T10:20:00Z"),
            // R$ 259,80 less the R$ 20,00 discount, for 2 units; the sale
            // fee is Amazon's table until the finances bring the real one.
            lines: vec![fone_line(2, "119.90", "17.985"), caneca_line()],
            total: brl("279.70"),
            paid: Some(brl("294.70")),
            shipping_paid: Some(brl("15.00")),
            refunded: None,
            // No phone: shipping needs only who and where.
            buyer: Some(Buyer {
                nickname: None,
                receiver: Some(Receiver {
                    name: "Ana Souza".into(),
                    address: Some("Rua da Aurora, 100, ap 12".into()),
                    city: Some("Recife".into()),
                    state: Some("PE".into()),
                    zip_code: Some("50050-000".into()),
                }),
            }),
            shipment: Some(Shipment {
                id: UNSHIPPED.into(),
                status: ShipmentStatus::ReadyToShip,
                dispatch_by: Some(at("2026-10-17T23:59:59Z")),
                seller_cost: None,
            }),
            returns: Vec::new(),
        }
    );
}

#[test]
fn each_fulfillment_status_is_an_order_and_shipment_status() {
    let fake = Fake::connected();

    let orders = fake.orders().unwrap();

    let statuses = |id: &str| {
        let order = found(&orders, id);
        (order.status, order.shipment.as_ref().unwrap().status)
    };
    assert_eq!(
        statuses("702-1234567-1234567"),
        (OrderStatus::AwaitingPayment, ShipmentStatus::Pending)
    );
    assert_eq!(
        statuses("703-1234567-1234567"),
        (OrderStatus::Cancelled, ShipmentStatus::Cancelled)
    );
    assert_eq!(
        statuses(DELIVERED),
        (OrderStatus::Paid, ShipmentStatus::Delivered)
    );
    // Cancelled once its package left: the units wait for the owner.
    assert_eq!(
        statuses("705-1234567-1234567"),
        (OrderStatus::Cancelled, ShipmentStatus::Shipped)
    );
    // Pending, it is priced by the unit and has no address yet.
    let pending = found(&orders, "702-1234567-1234567");
    assert_eq!(pending.lines, [fone_line(1, "129.90", "19.485")]);
    assert_eq!((pending.total, pending.paid), (brl("129.90"), None));
    assert_eq!(pending.buyer, None);
    // A cancelled item listed with no units leaves no line.
    assert_eq!(found(&orders, "703-1234567-1234567").lines, [caneca_line()]);
}

#[test]
fn without_the_shipping_role_the_orders_come_without_the_address() {
    let fake = Fake::connected();
    fake.server.serve(
        &search(WITH_RECIPIENT, None),
        403,
        fixture("error-403.json"),
    );
    fake.serve(
        &search(WITHOUT_RECIPIENT, None),
        "orders-without-recipient.json",
    );

    let orders = fake.orders().unwrap();
    fake.orders().unwrap();

    // Amazon tells only the city: nothing to ship to.
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].buyer, None);
    assert_eq!(orders[0].lines.len(), 2);
    // Turned down even with a renewed token, the next searches go without
    // asking again.
    assert_eq!(
        fake.calls(),
        [
            search(WITH_RECIPIENT, None),
            search(WITH_RECIPIENT, None),
            search(WITHOUT_RECIPIENT, None),
            search(WITHOUT_RECIPIENT, None)
        ]
    );
}

#[test]
fn a_page_token_amazon_turns_down_starts_the_search_over_once() {
    let fake = Fake::connected();
    let second = search(WITH_RECIPIENT, Some(PAGE_TOKEN));
    fake.server
        .serve(&second, 400, fixture("error-400-page-token.json"));
    fake.server
        .then_serve(&second, 200, fixture("orders-page-2.json"));

    let orders = fake.orders().unwrap();

    assert_eq!(orders.len(), 5);
    assert_eq!(
        fake.calls(),
        [
            search(WITH_RECIPIENT, None),
            second.clone(),
            search(WITH_RECIPIENT, None),
            second
        ]
    );
}

#[test]
fn a_page_token_turned_down_twice_is_amazons_refusal() {
    let fake = Fake::connected();
    fake.server.serve(
        &search(WITH_RECIPIENT, Some(PAGE_TOKEN)),
        400,
        fixture("error-400-page-token.json"),
    );

    assert_eq!(
        fake.orders(),
        Err(PlatformError::Refused(
            "The paginationToken has expired.".into()
        ))
    );
}

#[test]
fn a_throttled_search_waits_and_tries_again() {
    let fake = Fake::connected();
    let first = search(WITH_RECIPIENT, None);
    fake.server.serve(&first, 429, fixture("error-429.json"));
    fake.server
        .then_serve(&first, 200, fixture("orders-page-1.json"));

    assert_eq!(fake.orders().unwrap().len(), 5);
    assert_eq!(fake.pauses(), [1]);
}

#[test]
fn a_login_amazon_keeps_turning_down_is_expired() {
    let fake = Fake::connected();
    for included in [WITH_RECIPIENT, WITHOUT_RECIPIENT] {
        fake.server
            .serve(&search(included, None), 403, fixture("error-403.json"));
    }

    assert_eq!(fake.orders(), Err(PlatformError::Expired));
}

#[test]
fn the_fees_are_the_expenses_of_each_orders_transactions() {
    let fake = Fake::connected();
    fake.serve(
        &transactions(UNSHIPPED, None),
        "transactions-701-page-1.json",
    );
    fake.serve(
        &transactions(UNSHIPPED, Some("dHJhbnNhY3Rpb25zLTI")),
        "transactions-701-page-2.json",
    );
    fake.serve(
        &transactions("702-1234567-1234567", None),
        "transactions-empty.json",
    );

    let billed = fake
        .adapter
        .billed_fees(&[UNSHIPPED.into(), "702-1234567-1234567".into()])
        .unwrap();

    let charge = |id: &str, kind: FeeKind, description: &str, amount: &str| ChannelFee {
        id: id.into(),
        kind,
        description: description.into(),
        amount: brl(amount),
    };
    // Not posted yet, the second Order is not billed.
    assert_eq!(billed.len(), 1);
    assert_eq!(billed[0].order, UNSHIPPED);
    assert_eq!(
        billed[0].fees,
        [
            charge(
                "amzn1.tid.701-payment:Expenses/AmazonFees/Commission",
                FeeKind::SaleFee,
                "Commission",
                "41.96"
            ),
            charge(
                "amzn1.tid.701-payment:Expenses/AmazonFees/ShippingChargeback",
                FeeKind::Shipping,
                "ShippingChargeback",
                "9.99"
            ),
            // Given back by Amazon, in the wrapped shape of the reference.
            charge(
                "amzn1.tid.701-adjustment:Expenses/AmazonFees/Commission",
                FeeKind::SaleFee,
                "Commission",
                "-2"
            ),
        ]
    );
    // Two seconds between Orders, as the finances allow.
    assert_eq!(fake.pauses(), [2]);
}

#[test]
fn an_order_id_that_is_not_amazons_is_never_asked_for() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter.billed_fees(&["../sellers".into()]),
        Err(PlatformError::NotFound)
    );
    let sent = SentShipment {
        carrier: "Correios".into(),
        tracking_code: "AA1".into(),
        shipped_at: at("2026-10-16T12:00:00Z"),
    };
    assert_eq!(
        fake.adapter.confirm_shipment("701?x=1", &sent),
        Err(PlatformError::NotFound)
    );
    assert!(fake.calls().is_empty());
}

#[test]
fn a_shipment_is_confirmed_with_the_carrier_tracking_and_every_unit_to_ship() {
    let fake = Fake::connected();
    fake.serve(&get_order(UNSHIPPED), "order-701.json");
    fake.server.serve(CONFIRM, 204, "");

    fake.adapter
        .confirm_shipment(
            UNSHIPPED,
            &SentShipment {
                carrier: "Correios".into(),
                tracking_code: "AA123456789BR".into(),
                shipped_at: at("2026-10-16T12:00:00Z"),
            },
        )
        .unwrap();

    let sent = fake
        .server
        .received()
        .into_iter()
        .find(|request| request.path == CONFIRM)
        .unwrap();
    assert_eq!(sent.method, "POST");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&sent.body).unwrap(),
        serde_json::json!({
            "marketplaceId": "A2Q3Y263D00KWC",
            "packageDetail": {
                "packageReferenceId": "1",
                "carrierCode": "Other",
                "carrierName": "Correios",
                "trackingNumber": "AA123456789BR",
                "shipDate": "2026-10-16T12:00:00Z",
                "orderItems": [
                    {"orderItemId": "10000000000001", "quantity": 2},
                    {"orderItemId": "10000000000002", "quantity": 1}
                ]
            }
        })
    );
}

#[test]
fn an_order_amazon_already_shipped_is_not_confirmed_again() {
    let fake = Fake::connected();
    fake.serve(&get_order(DELIVERED), "order-704.json");
    let sent = SentShipment {
        carrier: "Correios".into(),
        tracking_code: "AA123456785BR".into(),
        shipped_at: at("2026-10-16T12:00:00Z"),
    };

    assert!(matches!(
        fake.adapter.confirm_shipment(DELIVERED, &sent),
        Err(PlatformError::Refused(_))
    ));
    assert_eq!(fake.calls(), [get_order(DELIVERED)]);
}

#[test]
fn a_shipment_amazon_turns_down_is_its_refusal() {
    let fake = Fake::connected();
    fake.serve(&get_order(UNSHIPPED), "order-701.json");
    fake.server.serve(CONFIRM, 400, fixture("error-400.json"));
    let sent = SentShipment {
        carrier: "Correios".into(),
        tracking_code: "AA123456789BR".into(),
        shipped_at: at("2026-10-16T12:00:00Z"),
    };

    assert!(matches!(
        fake.adapter.confirm_shipment(UNSHIPPED, &sent),
        Err(PlatformError::Refused(_))
    ));
}
