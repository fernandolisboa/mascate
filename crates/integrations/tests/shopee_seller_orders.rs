//! The owner's Shopee Orders, their returns, their Fees and their money
//! through the Open Platform v2 seller adapter: the adapter against the
//! fake server serving the answers in `fixtures/shopee-seller`, written
//! from the documentation (see its README).

use std::str::FromStr;
use std::sync::Arc;

use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use hmac::{Hmac, Mac};
use mascate_commerce::{
    Buyer, ChannelBilling, ChannelFee, ChannelOrder, ChannelOrderLine, ChannelOrders,
    ChannelPayments, ChannelReturn, FeeKind, OrderStatus, PaymentRelease, ReturnStatus,
    ReturnedItem, Shipment, ShipmentStatus,
};
use mascate_integrations::ShopeeSeller;
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{Currency, ListingType, Money, PlatformError, Timestamp};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};
use rust_decimal::Decimal;
use sha2::Sha256;

const PARTNER: &str = "2001887";
const KEY: &str = "shpk-mascate-partner-key";
const SHOP: &str = "14701711";
/// 2026-10-16 12:00 UTC, the fake clock's time.
const NOW: i64 = 1_792_152_000;
const DAY: i64 = 86_400;
const FIRST_ACCESS: &str = "59777174636562737266615546704c6d";
const SECOND_ACCESS: &str = "6c4f6d5a4c4a5a6e4f5a7a4d6b4e6c4a";

const RENEW: &str = "/api/v2/auth/access_token/get";
const ORDER_LIST: &str = "/api/v2/order/get_order_list";
const ORDER_DETAIL: &str = "/api/v2/order/get_order_detail";
const RETURN_LIST: &str = "/api/v2/returns/get_return_list";
const ESCROW: &str = "/api/v2/payment/get_escrow_detail";

const DONE: &str = "260920DNE0005E";
const CANCEL: &str = "261010CNC0003C";
const READY: &str = "261014RDY0001A";
const INVOICE: &str = "261015NFE0002B";
const RETURN: &str = "260925RET0004D";

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

fn at(seconds: i64) -> Timestamp {
    DateTime::from_timestamp(seconds, 0).unwrap()
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

/// A shop API call to `path` with `token`, then `query`.
fn shop(path: &str, token: &str, query: &str) -> String {
    format!(
        "{path}?partner_id={PARTNER}&timestamp={NOW}&access_token={token}&shop_id={SHOP}&sign={}{query}",
        sign(&format!("{PARTNER}{path}{NOW}{token}{SHOP}"))
    )
}

fn order_list(token: &str, from: i64, to: i64, cursor: Option<&str>) -> String {
    let mut query =
        format!("&time_range_field=update_time&time_from={from}&time_to={to}&page_size=100");
    if let Some(cursor) = cursor {
        query.push_str(&format!("&cursor={cursor}"));
    }
    shop(ORDER_LIST, token, &query)
}

fn return_list(from: i64, to: i64) -> String {
    shop(
        RETURN_LIST,
        FIRST_ACCESS,
        &format!("&page_no=0&page_size=100&update_time_from={from}&update_time_to={to}"),
    )
}

fn order_detail(orders: &[&str]) -> String {
    shop(
        ORDER_DETAIL,
        FIRST_ACCESS,
        &format!(
            "&order_sn_list={}\
             &response_optional_fields=buyer_username%2Citem_list%2Cpickup_done_time%2Ctotal_amount",
            orders.join("%2C")
        ),
    )
}

fn escrow(order: &str) -> String {
    shop(ESCROW, FIRST_ACCESS, &format!("&order_sn={order}"))
}

struct Fake {
    server: FakeHttpServer,
    adapter: Arc<ShopeeSeller>,
}

impl Fake {
    /// The partner's keys and the shop's login stored, Shopee answering
    /// the first Order Sync: 30 days in two windows.
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
        let adapter = Arc::new(ShopeeSeller::new(
            server.url(),
            "Mascate/test",
            store,
            clock,
        ));
        let fake = Self { server, adapter };
        fake.serve(&public(RENEW), "access-token-get.json");
        let (first, second) = first_sync_windows();
        fake.serve(
            &order_list(FIRST_ACCESS, first.0, first.1, None),
            "order-list-window-1-page-1.json",
        );
        fake.serve(
            &order_list(FIRST_ACCESS, first.0, first.1, Some("1")),
            "order-list-window-1-page-2.json",
        );
        fake.serve(
            &order_list(FIRST_ACCESS, second.0, second.1, None),
            "order-list-window-2.json",
        );
        fake.serve(&return_list(first.0, first.1), "return-list-empty.json");
        fake.serve(&return_list(second.0, second.1), "return-list.json");
        fake.serve(
            &order_detail(&[DONE, CANCEL, READY, INVOICE, RETURN]),
            "order-detail.json",
        );
        fake
    }

    fn serve(&self, path: &str, name: &str) {
        self.server.serve(path, 200, fixture(name));
    }

    fn calls_to(&self, path: &str) -> Vec<String> {
        self.server
            .requests()
            .into_iter()
            .filter(|request| request.starts_with(&format!("{path}?")))
            .collect()
    }
}

/// The first Sync's 30 days as Shopee takes them: two spans of 15.
fn first_sync_windows() -> ((i64, i64), (i64, i64)) {
    let since = NOW - 30 * DAY;
    ((since, since + 15 * DAY), (since + 15 * DAY, NOW))
}

fn first_sync(fake: &Fake) -> Vec<ChannelOrder> {
    fake.adapter
        .orders_changed_since(at(NOW - 30 * DAY))
        .unwrap()
}

fn found<'a>(orders: &'a [ChannelOrder], id: &str) -> &'a ChannelOrder {
    orders
        .iter()
        .find(|order| order.id == id)
        .unwrap_or_else(|| panic!("no Order {id}"))
}

fn line(
    item: &str,
    variation: Option<(&str, &str)>,
    title: &str,
    quantity: u32,
    unit_price: &str,
    sale_fee: &str,
) -> ChannelOrderLine {
    ChannelOrderLine {
        item: item.into(),
        variation: variation.map(|(id, _)| id.to_owned()),
        title: title.into(),
        variation_name: variation.map(|(_, name)| name.to_owned()),
        quantity,
        unit_price: brl(unit_price),
        sale_fee: Some(brl(sale_fee)),
        listing_type: Some(ListingType::default()),
    }
}

fn caneca(quantity: u32) -> ChannelOrderLine {
    // 20% and R$ 4,00 below R$ 80,00.
    line(
        "800003",
        None,
        "Caneca de Cerâmica 350 ml",
        quantity,
        "39.9",
        "11.98",
    )
}

#[test]
fn the_first_sync_reads_thirty_days_in_two_windows_of_fifteen_page_by_page() {
    let fake = Fake::connected();

    let orders = first_sync(&fake);

    let (first, second) = first_sync_windows();
    assert_eq!(
        fake.calls_to(ORDER_LIST),
        [
            order_list(FIRST_ACCESS, first.0, first.1, None),
            order_list(FIRST_ACCESS, first.0, first.1, Some("1")),
            order_list(FIRST_ACCESS, second.0, second.1, None),
        ]
    );
    assert_eq!(
        fake.calls_to(RETURN_LIST),
        [
            return_list(first.0, first.1),
            return_list(second.0, second.1)
        ]
    );
    // The Order on the line between the windows comes once, and every
    // detail in one call.
    assert_eq!(
        fake.calls_to(ORDER_DETAIL),
        [order_detail(&[DONE, CANCEL, READY, INVOICE, RETURN])]
    );
    let ids: Vec<&str> = orders.iter().map(|order| order.id.as_str()).collect();
    assert_eq!(ids, [DONE, CANCEL, READY, INVOICE, RETURN]);
}

#[test]
fn an_order_comes_with_its_lines_its_money_and_its_time_to_ship() {
    let fake = Fake::connected();

    let orders = first_sync(&fake);

    // The price after the seller's discount, with Shopee's table fee:
    // 14% and R$ 20,00 from R$ 100,00.
    assert_eq!(
        *found(&orders, READY),
        ChannelOrder {
            id: READY.into(),
            pack: None,
            status: OrderStatus::Paid,
            ordered_at: at(NOW - 2 * DAY),
            updated_at: at(NOW - 3_600),
            lines: vec![line(
                "800001",
                None,
                "Fone de Ouvido Bluetooth Mascate Preto",
                2,
                "119.9",
                "36.786",
            )],
            total: brl("239.8"),
            paid: Some(brl("254.7")),
            shipping_paid: None,
            refunded: None,
            buyer: Some(Buyer {
                nickname: Some("comprador_fone".into()),
                receiver: None,
            }),
            shipment: Some(Shipment {
                id: READY.into(),
                status: ShipmentStatus::ReadyToShip,
                dispatch_by: Some(at(NOW + DAY)),
                seller_cost: None,
            }),
            returns: Vec::new(),
        }
    );
    // A model sold is the Listing's variation; a masked buyer is no buyer.
    let invoice = found(&orders, INVOICE);
    assert_eq!(
        invoice.lines,
        [line(
            "800002",
            Some(("900021", "Preta,P")),
            "Camiseta Básica Algodão",
            1,
            "49.9",
            "13.98",
        )]
    );
    assert_eq!(invoice.buyer, None);
}

#[test]
fn each_order_status_is_an_order_and_shipment_status() {
    let fake = Fake::connected();
    let since = NOW - 600;
    fake.serve(
        &order_list(FIRST_ACCESS, since, NOW, None),
        "order-list-statuses.json",
    );
    fake.serve(&return_list(since, NOW), "return-list-empty.json");
    let statuses = [
        "261016SHP0006F",
        "261016TCR0007G",
        "261016INC0008H",
        "261016UNP0009J",
        "261016CNP0010K",
        "261016PRC0011L",
        "261016PND0012M",
    ];
    fake.serve(&order_detail(&statuses), "order-detail-statuses.json");

    let mut orders = fake.adapter.orders_changed_since(at(since)).unwrap();
    orders.extend(first_sync(&fake));

    let status_of = |id: &str| {
        let order = found(&orders, id);
        (order.status, order.shipment.as_ref().unwrap().status)
    };
    for (id, expected) in [
        (READY, (OrderStatus::Paid, ShipmentStatus::ReadyToShip)),
        (
            "261016PRC0011L",
            (OrderStatus::Paid, ShipmentStatus::ReadyToShip),
        ),
        // Outside the Correios: waiting for the NF-e, never held back.
        (
            INVOICE,
            (OrderStatus::Paid, ShipmentStatus::AwaitingInvoice),
        ),
        (
            "261016SHP0006F",
            (OrderStatus::Paid, ShipmentStatus::Shipped),
        ),
        (
            "261016TCR0007G",
            (OrderStatus::Paid, ShipmentStatus::Delivered),
        ),
        (DONE, (OrderStatus::Paid, ShipmentStatus::Delivered)),
        (RETURN, (OrderStatus::Paid, ShipmentStatus::Delivered)),
        // Only asked for: the seller may still turn it down.
        (
            "261016INC0008H",
            (OrderStatus::Paid, ShipmentStatus::ReadyToShip),
        ),
        (CANCEL, (OrderStatus::Cancelled, ShipmentStatus::Cancelled)),
        // Cancelled after the carrier took it: its units come back.
        (
            "261016CNP0010K",
            (OrderStatus::Cancelled, ShipmentStatus::Shipped),
        ),
        (
            "261016UNP0009J",
            (OrderStatus::AwaitingPayment, ShipmentStatus::Pending),
        ),
        (
            "261016PND0012M",
            (OrderStatus::AwaitingPayment, ShipmentStatus::Pending),
        ),
    ] {
        assert_eq!(status_of(id), expected, "{id}");
    }
}

#[test]
fn a_span_longer_than_fifteen_days_is_read_fifteen_days_at_a_time() {
    let fake = Fake::connected();
    let since = NOW - 40 * DAY;
    fake.server
        .serve_any_query(ORDER_LIST, 200, fixture("order-list-empty.json"));
    fake.server
        .serve_any_query(RETURN_LIST, 200, fixture("return-list-empty.json"));

    assert_eq!(fake.adapter.orders_changed_since(at(since)), Ok(Vec::new()));

    let spans: Vec<(i64, i64)> = fake
        .calls_to(ORDER_LIST)
        .iter()
        .map(|call| {
            let value = |name: &str| -> i64 {
                call.split(['?', '&'])
                    .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
                    .unwrap()
                    .parse()
                    .unwrap()
            };
            (value("time_from"), value("time_to"))
        })
        .collect();
    assert_eq!(
        spans,
        [
            (since, since + 15 * DAY),
            (since + 15 * DAY, since + 30 * DAY),
            (since + 30 * DAY, NOW),
        ]
    );
    assert!(
        spans
            .iter()
            .all(|(from, to)| to - from <= TimeDelta::days(15).num_seconds())
    );
    assert_eq!(fake.calls_to(RETURN_LIST).len(), 3);
    // Nothing changed: no detail to ask for.
    assert!(fake.calls_to(ORDER_DETAIL).is_empty());
}

#[test]
fn a_return_with_units_comes_with_its_order_and_a_refund_alone_brings_none() {
    let fake = Fake::connected();

    let orders = first_sync(&fake);

    let returned = found(&orders, RETURN);
    assert_eq!(returned.lines, [caneca(2)]);
    assert_eq!(
        returned.returns,
        [ChannelReturn {
            id: "2610130000001".into(),
            status: ReturnStatus::OnTheWay,
            items: vec![ReturnedItem {
                item: "800003".into(),
                variation: None,
                quantity: 1,
            }],
        }]
    );
    assert_eq!(returned.refunded, None);

    // Refunded once Shopee accepts it.
    let (_, second) = first_sync_windows();
    fake.serve(
        &return_list(second.0, second.1),
        "return-list-accepted.json",
    );
    let accepted = first_sync(&fake);
    let returned = found(&accepted, RETURN);
    assert_eq!(returned.returns[0].status, ReturnStatus::Delivered);
    assert_eq!(returned.refunded, Some(brl("39.9")));

    // A refund without sending the item back: the buyer keeps it.
    fake.serve(
        &return_list(second.0, second.1),
        "return-list-refund-only.json",
    );
    let refunded = first_sync(&fake);
    assert_eq!(
        found(&refunded, RETURN).returns,
        [ChannelReturn {
            id: "2610130000002".into(),
            status: ReturnStatus::OnTheWay,
            items: Vec::new(),
        }]
    );
}

#[test]
fn an_order_whose_return_changed_before_the_window_reads_back_until_it_finds_it() {
    let fake = Fake::connected();
    let since = NOW - 600;
    let mut order_list_body: serde_json::Value =
        serde_json::from_str(&fixture("order-list-statuses.json")).unwrap();
    order_list_body["response"]["order_list"] =
        serde_json::json!([{"order_sn": RETURN, "order_status": "TO_RETURN"}]);
    fake.server.serve(
        &order_list(FIRST_ACCESS, since, NOW, None),
        200,
        order_list_body.to_string(),
    );
    fake.serve(&return_list(since, NOW), "return-list-empty.json");
    fake.serve(&order_detail(&[RETURN]), "order-detail-return.json");
    let back = since - 15 * DAY;
    fake.serve(&return_list(back, since), "return-list.json");

    let orders = fake.adapter.orders_changed_since(at(since)).unwrap();

    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].returns.len(), 1);
    assert_eq!(orders[0].returns[0].items[0].quantity, 1);
    // Found in the first span back: no older one is asked for.
    assert_eq!(
        fake.calls_to(RETURN_LIST),
        [return_list(since, NOW), return_list(back, since)]
    );
}

#[test]
fn a_return_never_found_reads_back_no_further_than_its_order() {
    let fake = Fake::connected();
    let since = NOW - 600;
    let mut order_list_body: serde_json::Value =
        serde_json::from_str(&fixture("order-list-statuses.json")).unwrap();
    order_list_body["response"]["order_list"] =
        serde_json::json!([{"order_sn": RETURN, "order_status": "TO_RETURN"}]);
    fake.server.serve(
        &order_list(FIRST_ACCESS, since, NOW, None),
        200,
        order_list_body.to_string(),
    );
    fake.server
        .serve_any_query(RETURN_LIST, 200, fixture("return-list-empty.json"));
    fake.serve(&order_detail(&[RETURN]), "order-detail-return.json");

    let orders = fake.adapter.orders_changed_since(at(since)).unwrap();

    assert!(orders[0].returns.is_empty());
    // The Order was made 21 days ago: two spans back reach it.
    let made = NOW - 21 * DAY;
    assert_eq!(
        fake.calls_to(RETURN_LIST),
        [
            return_list(since, NOW),
            return_list(since - 15 * DAY, since),
            return_list(made, since - 15 * DAY),
        ]
    );
}

#[test]
fn a_token_shopee_keeps_turning_down_is_an_expired_login() {
    let fake = Fake::connected();
    let (first, _) = first_sync_windows();
    fake.serve(
        &order_list(FIRST_ACCESS, first.0, first.1, None),
        "error-auth.json",
    );
    fake.server
        .then_serve(&public(RENEW), 200, fixture("access-token-get-again.json"));
    fake.serve(
        &order_list(SECOND_ACCESS, first.0, first.1, None),
        "error-auth.json",
    );

    assert_eq!(
        fake.adapter.orders_changed_since(at(NOW - 30 * DAY)),
        Err(PlatformError::Expired)
    );
    assert!(fake.calls_to(ORDER_DETAIL).is_empty());
}

#[test]
fn the_fees_of_a_completed_order_come_from_its_escrow() {
    let fake = Fake::connected();
    fake.serve(
        &order_detail(&[DONE, READY, CANCEL]),
        "order-detail-escrow.json",
    );
    fake.serve(&escrow(DONE), "escrow-completed.json");

    let billed = fake
        .adapter
        .billed_fees(&[DONE.into(), READY.into(), CANCEL.into()])
        .unwrap();

    let fee = |id: &str, kind, description: &str, amount: &str| ChannelFee {
        id: id.into(),
        kind,
        description: description.into(),
        amount: brl(amount),
    };
    assert_eq!(billed.len(), 1);
    assert_eq!(billed[0].order, DONE);
    // R$ 22,50 of shipping, R$ 18,00 of it on Shopee: R$ 4,50 on the
    // seller.
    assert_eq!(
        billed[0].fees,
        [
            fee("commission_fee", FeeKind::SaleFee, "Comissão", "18.19"),
            fee("service_fee", FeeKind::SaleFee, "Taxa de serviço", "20"),
            fee("shipping", FeeKind::Shipping, "Frete", "4.5"),
        ]
    );
    // Shopee still changes the escrow of an Order not completed: its
    // margin stays provisional, and the escrow is not even asked for.
    assert_eq!(fake.calls_to(ESCROW), [escrow(DONE)]);
}

#[test]
fn the_escrow_is_released_once_the_order_is_completed() {
    let fake = Fake::connected();
    fake.serve(
        &order_detail(&[DONE, READY, CANCEL]),
        "order-detail-escrow.json",
    );
    fake.serve(&escrow(DONE), "escrow-completed.json");
    fake.serve(&escrow(READY), "escrow-ready.json");

    let releases = fake
        .adapter
        .releases(&[DONE.into(), READY.into(), CANCEL.into()])
        .unwrap();

    assert_eq!(
        releases,
        [
            PaymentRelease {
                order: DONE.into(),
                payment: DONE.into(),
                amount: brl("87.21"),
                released: true,
                release_at: Some(at(NOW - 16 * DAY)),
            },
            PaymentRelease {
                order: READY.into(),
                payment: READY.into(),
                amount: brl("165.49"),
                released: false,
                release_at: None,
            },
        ]
    );
    // A cancelled Order pays nothing: its escrow is never asked for.
    assert_eq!(fake.calls_to(ESCROW), [escrow(DONE), escrow(READY)]);
}

#[test]
fn an_order_number_that_is_not_shopees_is_never_asked_for() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter.billed_fees(&["../shop/x".into()]),
        Ok(Vec::new())
    );
    assert_eq!(fake.adapter.releases(&["26 10&x=1".into()]), Ok(Vec::new()));
    assert!(fake.server.requests().is_empty());
}
