//! When the money of the owner's Orders is released, through Mercado
//! Livre's adapter and the Mercado Pago's payments, against the fake server
//! serving the answers in `fixtures/mercado-livre`, written from the
//! documentation (see its README). What commerce does with them is tested
//! there.

use std::str::FromStr;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use mascate_commerce::{ChannelPayments, PaymentRelease};
use mascate_integrations::MercadoLivre;
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{Currency, Money, PlatformError};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};
use rust_decimal::Decimal;

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

const FONE: &str = "2000009876543210";
const CARRINHO: &str = "2000009876543300";
const CAMISETA: &str = "2000009876543500";
/// Mercado Livre no longer has it.
const GONE: &str = "2000009876549999";

struct Fake {
    server: FakeHttpServer,
    adapter: MercadoLivre,
}

impl Fake {
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
            Utc.with_ymd_and_hms(2026, 10, 5, 12, 0, 0).unwrap(),
        ));
        let adapter = MercadoLivre::new(server.url(), "Mascate/test", store, clock)
            .with_payments_api(server.url());
        server.serve("/oauth/token", 200, fixture("token.json"));
        for order in [FONE, CARRINHO, CAMISETA] {
            server.serve(
                &format!("/orders/{order}"),
                200,
                fixture(&format!("order-{order}.json")),
            );
        }
        for payment in ["91000000001", "91000000002", "91000000004", "91000000005"] {
            server.serve(
                &format!("/v1/payments/{payment}"),
                200,
                fixture(&format!("mp-payment-{payment}.json")),
            );
        }
        Self { server, adapter }
    }

    fn payments_asked(&self) -> Vec<String> {
        self.server
            .received()
            .into_iter()
            .map(|request| request.path)
            .filter(|path| path.starts_with("/v1/payments/"))
            .collect()
    }
}

fn ids(orders: &[&str]) -> Vec<String> {
    orders.iter().map(|&order| order.to_owned()).collect()
}

#[test]
fn each_approved_payment_comes_with_its_net_amount_and_when_it_is_released() {
    let fake = Fake::connected();

    let releases = fake
        .adapter
        .releases(&ids(&[FONE, CARRINHO, CAMISETA, GONE]))
        .unwrap();

    assert_eq!(
        releases,
        [
            PaymentRelease {
                order: FONE.into(),
                payment: "91000000001".into(),
                amount: brl("171.17"),
                released: false,
                release_at: Some(Utc.with_ymd_and_hms(2026, 10, 18, 13, 15, 4).unwrap()),
            },
            PaymentRelease {
                order: CARRINHO.into(),
                payment: "91000000002".into(),
                amount: brl("133.01"),
                released: true,
                release_at: Some(Utc.with_ymd_and_hms(2026, 10, 1, 18, 2, 10).unwrap()),
            },
            // No net amount yet: the amount paid, still without a date.
            PaymentRelease {
                order: CAMISETA.into(),
                payment: "91000000005".into(),
                amount: brl("59.9"),
                released: false,
                release_at: None,
            },
        ]
    );
}

#[test]
fn a_payment_refunded_in_the_order_is_not_asked_about_and_one_refunded_since_is_left_out() {
    let fake = Fake::connected();

    let releases = fake.adapter.releases(&ids(&[FONE, CARRINHO])).unwrap();

    assert!(
        releases
            .iter()
            .all(|release| release.payment != "91000000004")
    );
    assert_eq!(
        fake.payments_asked(),
        [
            "/v1/payments/91000000001",
            "/v1/payments/91000000002",
            "/v1/payments/91000000004"
        ]
    );
}

#[test]
fn a_wallet_that_refuses_fails_the_read() {
    let fake = Fake::connected();
    fake.server
        .serve("/v1/payments/91000000001", 429, fixture("error-429.json"));

    assert_eq!(
        fake.adapter.releases(&ids(&[FONE])),
        Err(PlatformError::RateLimited)
    );
}

#[test]
fn an_order_id_that_is_not_mercado_livres_reaches_no_path() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter.releases(&ids(&["../users/me"])),
        Err(PlatformError::NotFound)
    );
    assert!(
        fake.server
            .received()
            .iter()
            .all(|request| !request.path.starts_with("/users"))
    );
}
