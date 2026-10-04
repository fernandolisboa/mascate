//! The owner's Orders through Mercado Livre's adapter, against the fake
//! server serving the answers in `fixtures/mercado-livre`, written from the
//! documentation (see its README). What an Order Sync does with them is
//! tested in commerce, which owns it.

use std::str::FromStr;
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use mascate_commerce::{
    Buyer, ChannelOrder, ChannelOrderLine, ChannelOrders, OrderStatus, Receiver, Shipment,
    ShipmentStatus,
};
use mascate_integrations::MercadoLivre;
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{Currency, ListingType, Money, PlatformError, Timestamp};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};
use rust_decimal::Decimal;

/// Since 11:50 UTC: ten minutes before a Sync at noon.
const SEARCH: &str = "/orders/search?seller=1234567\
                      &order.date_last_updated.from=2026-10-04T11%3A50%3A00.000-00%3A00\
                      &sort=date_asc&offset=0&limit=50";
const SEARCH_NEXT: &str = "/orders/search?seller=1234567\
                           &order.date_last_updated.from=2026-10-04T11%3A50%3A00.000-00%3A00\
                           &sort=date_asc&offset=2&limit=50";
const SHIPMENT: &str = "/shipments/44100000001";
/// Mercado Livre has not made this shipment yet: it answers 404.
const SHIPMENT_NOT_YET: &str = "/shipments/44100000002";

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

fn at(text: &str) -> Timestamp {
    chrono::DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

fn noon() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap()
}

struct Fake {
    server: FakeHttpServer,
    adapter: MercadoLivre,
}

impl Fake {
    /// A connected seller account with three Orders changed since 11:50
    /// UTC, in two pages.
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
        let clock = Arc::new(ManualClock::at(noon()));
        let adapter = MercadoLivre::new(server.url(), "Mascate/test", store, clock);
        let fake = Self { server, adapter };
        fake.serve("/oauth/token", "token.json");
        fake.serve("/users/me", "users-me.json");
        fake.serve(SEARCH, "orders-search.json");
        fake.serve(SEARCH_NEXT, "orders-search-end.json");
        fake.serve(SHIPMENT, "shipment-44100000001.json");
        fake
    }

    fn serve(&self, path: &str, name: &str) {
        self.server.serve(path, 200, fixture(name));
    }

    fn since(&self) -> Timestamp {
        noon() - TimeDelta::minutes(10)
    }
}

fn fone_order() -> ChannelOrder {
    ChannelOrder {
        id: "2000009876543210".into(),
        pack: None,
        status: OrderStatus::Paid,
        ordered_at: at("2026-10-04T09:15:02-03:00"),
        updated_at: at("2026-10-04T09:20:11-03:00"),
        lines: vec![ChannelOrderLine {
            item: "MLB4100000001".into(),
            variation: None,
            title: "Fone De Ouvido Bluetooth Tws Pro Preto".into(),
            variation_name: None,
            quantity: 2,
            unit_price: brl("89.9"),
            sale_fee: Some(brl("12.59")),
            listing_type: Some(ListingType::Classic),
        }],
        total: brl("179.8"),
        paid: Some(brl("199.7")),
        shipping_paid: Some(brl("19.9")),
        buyer: Some(Buyer {
            nickname: Some("ANASOUZA2026".into()),
            receiver: Some(Receiver {
                name: "Ana Souza".into(),
                address: Some("Rua da Aurora 100, ap 12".into()),
                city: Some("Recife".into()),
                state: Some("Pernambuco".into()),
                zip_code: Some("50050000".into()),
            }),
        }),
        shipment: Some(Shipment {
            id: "44100000001".into(),
            status: ShipmentStatus::ReadyToShip,
            dispatch_by: Some(at("2026-10-06T23:59:59-03:00")),
        }),
    }
}

fn camiseta_line(
    variation: &str,
    color: &str,
    quantity: u32,
    price: &str,
    fee: &str,
) -> ChannelOrderLine {
    ChannelOrderLine {
        item: "MLB4100000003".into(),
        variation: Some(variation.into()),
        title: "Camiseta Básica Algodão".into(),
        variation_name: Some(format!("Cor: {color} · Tamanho: M")),
        quantity,
        unit_price: brl(price),
        sale_fee: Some(brl(fee)),
        listing_type: Some(ListingType::Premium),
    }
}

#[test]
fn orders_come_page_by_page_with_their_items_buyer_and_shipment() {
    let fake = Fake::connected();

    let orders = fake.adapter.orders_changed_since(fake.since()).unwrap();

    assert_eq!(
        orders,
        [
            fone_order(),
            ChannelOrder {
                id: "2000009876543300".into(),
                // Bought in one cart with another Order.
                pack: Some("2000004455667788".into()),
                status: OrderStatus::Paid,
                ordered_at: at("2026-10-04T10:02:40-03:00"),
                updated_at: at("2026-10-04T10:02:44-03:00"),
                lines: vec![
                    camiseta_line("175000000031", "Preto", 1, "49.9", "8.48"),
                    camiseta_line("175000000032", "Branco", 2, "44.91", "7.63"),
                ],
                total: brl("139.72"),
                paid: Some(brl("139.72")),
                shipping_paid: Some(brl("0")),
                // No shipment yet, so no receiver: only the nickname.
                buyer: Some(Buyer {
                    nickname: Some("BRUNO.LIMA".into()),
                    receiver: None,
                }),
                shipment: None,
            },
            ChannelOrder {
                id: "2000009876543400".into(),
                pack: None,
                status: OrderStatus::Cancelled,
                ordered_at: at("2026-10-04T10:40:00-03:00"),
                updated_at: at("2026-10-04T11:10:00-03:00"),
                lines: vec![ChannelOrderLine {
                    quantity: 1,
                    ..fone_order().lines[0].clone()
                }],
                total: brl("89.9"),
                paid: Some(brl("0")),
                // The only payment was turned down.
                shipping_paid: None,
                buyer: Some(Buyer {
                    nickname: Some("CARLA_M".into()),
                    receiver: None,
                }),
                shipment: None,
            },
        ]
    );
}

#[test]
fn the_search_is_by_last_change_and_shipments_ask_for_the_new_format() {
    let fake = Fake::connected();

    fake.adapter.orders_changed_since(fake.since()).unwrap();

    let received = fake.server.received();
    let paths: Vec<&str> = received
        .iter()
        .map(|request| request.path.as_str())
        .filter(|path| !path.starts_with("/oauth") && *path != "/users/me")
        .collect();
    assert_eq!(paths, [SEARCH, SHIPMENT, SHIPMENT_NOT_YET, SEARCH_NEXT]);
    let shipment = received
        .iter()
        .find(|request| request.path == SHIPMENT)
        .unwrap();
    assert!(
        shipment
            .headers
            .contains(&("x-format-new".to_owned(), "true".to_owned()))
    );
    assert!(
        received
            .iter()
            .filter(|request| !request.path.starts_with("/oauth"))
            .all(|request| request
                .authorization
                .as_deref()
                .is_some_and(|bearer| bearer.starts_with("Bearer ")))
    );
}

#[test]
fn a_refusal_or_a_limit_comes_back_as_a_platform_error() {
    let fake = Fake::connected();
    fake.server.serve(SEARCH, 429, fixture("error-429.json"));
    assert_eq!(
        fake.adapter.orders_changed_since(fake.since()),
        Err(PlatformError::RateLimited)
    );

    fake.server.serve(SEARCH, 403, fixture("error-403.json"));
    assert!(matches!(
        fake.adapter.orders_changed_since(fake.since()),
        Err(PlatformError::Refused(_))
    ));
}

#[test]
fn a_shipment_that_fails_other_than_missing_fails_the_read() {
    let fake = Fake::connected();
    fake.server.serve(SHIPMENT, 500, "{}");

    assert!(matches!(
        fake.adapter.orders_changed_since(fake.since()),
        Err(PlatformError::Failed(_))
    ));
}

#[test]
fn nothing_changed_is_an_empty_answer() {
    let fake = Fake::connected();
    fake.serve(SEARCH, "orders-search-empty.json");

    assert_eq!(
        fake.adapter.orders_changed_since(fake.since()),
        Ok(Vec::new())
    );
}
