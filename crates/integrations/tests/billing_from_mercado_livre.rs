//! The Fees Mercado Livre billed on the owner's Orders, through its
//! adapter, against the fake server serving the answers in
//! `fixtures/mercado-livre`, written from the documentation (see its
//! README). What commerce does with them is tested there.

use std::str::FromStr;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use mascate_commerce::{BilledOrder, ChannelBilling, ChannelFee, FeeKind};
use mascate_integrations::MercadoLivre;
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{Currency, Money, PlatformError};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};
use rust_decimal::Decimal;

const BILLING: &str = "/billing/integration/group/ML/order/details";

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

fn asking(orders: &[&str]) -> String {
    format!("{BILLING}?order_ids={}", orders.join("%2C"))
}

fn ids(orders: &[&str]) -> Vec<String> {
    orders.iter().map(|&order| order.to_owned()).collect()
}

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
            Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
        ));
        let adapter = MercadoLivre::new(server.url(), "Mascate/test", store, clock);
        server.serve("/oauth/token", 200, fixture("token.json"));
        Self { server, adapter }
    }

    fn asked(&self) -> Vec<String> {
        self.server
            .received()
            .into_iter()
            .map(|request| request.path)
            .filter(|path| path.starts_with(BILLING))
            .collect()
    }
}

const FONE: &str = "2000009876543210";
const CAMISETA: &str = "2000009876543500";
/// Sold, but not billed yet: Mercado Livre leaves it out.
const CARRINHO: &str = "2000009876543300";

#[test]
fn the_charges_of_each_order_come_with_their_kind_and_bonuses_given_back() {
    let fake = Fake::connected();
    let orders = [FONE, CARRINHO, CAMISETA];
    fake.server
        .serve(&asking(&orders), 200, fixture("billing-order-details.json"));

    let billed = fake.adapter.billed_fees(&ids(&orders)).unwrap();

    assert_eq!(
        billed,
        [
            BilledOrder {
                order: FONE.into(),
                fees: vec![
                    ChannelFee {
                        id: "7001000001-CV".into(),
                        kind: FeeKind::SaleFee,
                        description: "Tarifa de venda".into(),
                        amount: brl("25.18"),
                    },
                    ChannelFee {
                        id: "7001000002-CXD".into(),
                        kind: FeeKind::Shipping,
                        description: "Tarifa de envio do Mercado Envios".into(),
                        amount: brl("4.6"),
                    },
                ],
            },
            BilledOrder {
                order: CAMISETA.into(),
                fees: vec![
                    ChannelFee {
                        id: "7001000003-CV".into(),
                        kind: FeeKind::SaleFee,
                        description: "Tarifa de venda".into(),
                        amount: brl("25.44"),
                    },
                    // A bonus gives the charge back: it counts against the
                    // Fees.
                    ChannelFee {
                        id: "7001000004-BV".into(),
                        kind: FeeKind::SaleFee,
                        description: "Bonificação da tarifa de venda pela devolução".into(),
                        amount: brl("-8.48"),
                    },
                ],
            },
        ]
    );
}

#[test]
fn a_charge_mercado_livre_does_not_name_counts_as_another_fee() {
    let fake = Fake::connected();
    let mut answer: serde_json::Value =
        serde_json::from_str(&fixture("billing-order-details.json")).unwrap();
    let charge = &mut answer["results"][0]["details"][1];
    charge["charge_info"]["detail_sub_type"] = "CFONPN".into();
    charge["charge_info"]["transaction_detail"] = "Taxa de parcelamento".into();
    charge["marketplace_info"]["marketplace"] = "MP".into();
    fake.server.serve(&asking(&[FONE]), 200, answer.to_string());

    let billed = fake.adapter.billed_fees(&ids(&[FONE])).unwrap();

    assert_eq!(billed[0].fees[1].kind, FeeKind::Other);
    assert_eq!(billed[0].fees[1].description, "Taxa de parcelamento");
}

#[test]
fn orders_go_sixty_at_a_time() {
    let fake = Fake::connected();
    let orders: Vec<String> = (0..61).map(|n| format!("20000098765{n:05}")).collect();
    let first: Vec<&str> = orders[..60].iter().map(String::as_str).collect();
    let rest: Vec<&str> = orders[60..].iter().map(String::as_str).collect();
    fake.server.serve(&asking(&first), 200, r#"{"results":[]}"#);
    fake.server.serve(&asking(&rest), 200, r#"{"results":[]}"#);

    let billed = fake.adapter.billed_fees(&orders).unwrap();

    assert!(billed.is_empty());
    assert_eq!(fake.asked(), [asking(&first), asking(&rest)]);
}

#[test]
fn a_bill_still_being_put_together_counts_as_not_billed_yet() {
    let fake = Fake::connected();
    fake.server
        .serve(&asking(&[FONE]), 206, fixture("billing-order-details.json"));

    assert_eq!(fake.adapter.billed_fees(&ids(&[FONE])).unwrap(), []);
}

#[test]
fn a_limit_or_a_refusal_comes_back_as_a_platform_error() {
    let fake = Fake::connected();
    fake.server
        .serve(&asking(&[FONE]), 429, fixture("error-429.json"));
    assert_eq!(
        fake.adapter.billed_fees(&ids(&[FONE])),
        Err(PlatformError::RateLimited)
    );

    fake.server
        .serve(&asking(&[FONE]), 403, fixture("error-403.json"));
    assert!(matches!(
        fake.adapter.billed_fees(&ids(&[FONE])),
        Err(PlatformError::Refused(_))
    ));
}

#[test]
fn an_order_id_that_is_not_one_is_never_sent() {
    let fake = Fake::connected();

    assert!(fake.adapter.billed_fees(&ids(&["1,2"])).is_err());
    assert!(fake.asked().is_empty());
}
