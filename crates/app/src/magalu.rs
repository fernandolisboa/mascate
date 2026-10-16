//! Magalu as a Sales Channel (#133): where the adapter points, and how a
//! failed login reads in Portuguese.

use std::sync::Arc;

use gpui_kit::{App, Global};
use mascate_integrations::Magalu;
use mascate_kernel::PlatformError;
use mascate_platform::{Build, Environment};

/// The Magalu adapter, for the login in Configurações › Conexões.
pub struct AppMagalu(pub Arc<Magalu>);

impl Global for AppMagalu {}

pub fn adapter(cx: &App) -> Option<Arc<Magalu>> {
    cx.try_global::<AppMagalu>().map(|app| app.0.clone())
}

/// Where the app reaches the Open API and ID Magalu: Magalu's own hosts
/// or, in a development build only, the URL in `MASCATE_MAGALU_API_URL`
/// for both, Magalu's sandbox or a local fake server.
pub fn urls(build: Build, environment: &Environment) -> (String, String) {
    match crate::mercado_livre::local_fake(build, environment, "MASCATE_MAGALU_API_URL") {
        Some(url) if url == mascate_integrations::MAGALU_SANDBOX_URL => {
            (url, mascate_integrations::MAGALU_ID_URL.to_owned())
        }
        Some(fake) => (fake.clone(), fake),
        None => (
            mascate_integrations::MAGALU_API_URL.to_owned(),
            mascate_integrations::MAGALU_ID_URL.to_owned(),
        ),
    }
}

/// Why opening ID Magalu's page or trading its code failed.
pub fn login_failure(error: &PlatformError) -> String {
    match error {
        PlatformError::NotConnected => {
            "Salve o Client ID e o Client Secret antes de entrar na Magalu.".into()
        }
        PlatformError::Expired => {
            "A Magalu não aceitou as chaves: confira o Client ID e o Client Secret do cliente \
             criado no idm."
                .into()
        }
        PlatformError::Refused(why) => format!(
            "A Magalu recusou o código ({why}). Ele vale uma vez só e só para o endereço \
             cadastrado no cliente: clique em Entrar na Magalu de novo e cole o endereço novo."
        ),
        PlatformError::RateLimited => {
            "A Magalu pediu para esperar. Tente de novo daqui a alguns minutos.".into()
        }
        PlatformError::NotFound => "A Magalu não encontrou o cliente ou a loja.".into(),
        PlatformError::Failed(why) => format!("Não consegui falar com a Magalu: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::{NaiveDate, TimeZone, Utc};
    use futures::executor::block_on;
    use mascate_catalog::Catalog;
    use mascate_commerce::{
        Listings, NewPurchaseLine, NewPurchaseOrder, Orders, PurchaseOrders, Receiving, StockMirror,
    };
    use mascate_integrations::Connection;
    use mascate_inventory::{HOME_LOCATION, Inventory};
    use mascate_kernel::testing::{ManualClock, SequentialIds};
    use mascate_kernel::{Clock, Currency, IdGenerator, Money, RecordId};
    use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
    use mascate_platform::{Database, Secret, SecretStore, migrate};

    use super::*;
    use crate::orders;
    use crate::sales_channels::ChannelPorts;
    use crate::startup::MODULE_MIGRATIONS;

    fn fixture(name: &str) -> String {
        let path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "../integrations/tests/fixtures/magalu",
            name,
        ]
        .iter()
        .collect();
        std::fs::read_to_string(&path).unwrap()
    }

    /// The store with its three SKUs, every stock change taken with 202.
    fn magalu() -> (FakeHttpServer, Arc<Magalu>) {
        let server = FakeHttpServer::start();
        server.serve("/oauth/token", 200, fixture("token.json"));
        server.serve("/seller/v1/portfolios/me", 200, fixture("me.json"));
        server.serve_any_query("/seller/v1/portfolios/skus", 200, fixture("skus-page.json"));
        for sku in ["CAM-PRETA-M", "FONE-TWS", "CANECA-300"] {
            let name = sku.to_lowercase();
            for kind in ["skus", "prices", "stocks"] {
                let file = match kind {
                    "skus" => format!("sku-{name}.json"),
                    kind => format!("{kind}-{name}.json"),
                };
                server.serve_method(
                    "GET",
                    &format!("/seller/v1/portfolios/{kind}/{sku}"),
                    200,
                    fixture(&file),
                );
            }
            server.serve_method(
                "PATCH",
                &format!("/seller/v1/portfolios/stocks/{sku}"),
                202,
                fixture("accepted.json"),
            );
        }
        let store = Arc::new(MemorySecretStore::default());
        for (name, value) in [
            ("MAGALU_CLIENT_ID", "mascate-client-id"),
            ("MAGALU_CLIENT_SECRET", "mascate-client-secret"),
            ("MAGALU_REFRESH_TOKEN", "primeiro-refresh"),
        ] {
            store.write(name, &Secret::new(value)).unwrap();
        }
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
        ));
        let adapter = Arc::new(Magalu::new(
            server.url(),
            server.url(),
            "Mascate/test",
            store,
            clock,
        ));
        (server, adapter)
    }

    #[test]
    fn a_round_sends_each_linked_sku_its_share_lowering_first() {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let manual = Arc::new(ManualClock::at(
                Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
            ));
            let database = Arc::new(
                Database::open(&dir.path().join("mascate.db"))
                    .await
                    .unwrap(),
            );
            migrate(&database, manual.as_ref(), MODULE_MIGRATIONS)
                .await
                .unwrap();
            let clock: Arc<dyn Clock> = manual;
            let ids: Arc<dyn IdGenerator> = Arc::new(SequentialIds::default());
            let inventory = Arc::new(Inventory::new(database.clone(), clock.clone(), ids.clone()));
            let listings = Listings::new(database.clone(), clock.clone(), ids.clone());
            let (server, adapter) = magalu();
            listings.sync(adapter.as_ref()).await.unwrap();

            // The camiseta shows 7 on Magalu and the fone none; the owner
            // turned the cup off in Magalu's panel.
            let camiseta = RecordId::from_u128(0xca);
            let fone = RecordId::from_u128(0xf0);
            let caneca = RecordId::from_u128(0xcc);
            let purchases = PurchaseOrders::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            );
            for (product, units) in [(camiseta, 2), (fone, 3), (caneca, 4)] {
                let order = purchases
                    .create(NewPurchaseOrder {
                        supplier: RecordId::from_u128(0x5),
                        ordered_on: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                        freight: Money::zero(Currency::Brl),
                        lines: vec![NewPurchaseLine {
                            product,
                            quantity: units,
                            unit_price: Money::new(20.into(), Currency::Brl),
                        }],
                    })
                    .await
                    .unwrap();
                purchases
                    .receive(
                        order.id,
                        HOME_LOCATION,
                        &[Receiving {
                            line: order.lines[0].id,
                            quantity: units,
                        }],
                    )
                    .await
                    .unwrap();
            }
            for listing in listings.listings().await.unwrap() {
                let product = match listing.listed.id.as_str() {
                    "CAM-PRETA-M" => camiseta,
                    "FONE-TWS" => fone,
                    _ => caneca,
                };
                listings.link(listing.id, product).await.unwrap();
            }
            let mirror = StockMirror::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            );
            let orders = Orders::new(database.clone(), inventory, clock.clone(), ids.clone());
            let catalog = Catalog::new(database, dir.path().join("files"), clock, ids);
            let ports = ChannelPorts {
                connection: Connection::Magalu,
                listings: adapter,
                orders: None,
                billing: None,
                payments: None,
                labels: None,
                shipments: None,
                publisher: None,
            };

            let round = orders::round(&orders, &mirror, &catalog, &[ports])
                .await
                .unwrap()
                .unwrap();

            assert_eq!((round.sent.sent, round.sent.failed), (2, 0));
            let sent: Vec<(String, String)> = server
                .received()
                .into_iter()
                .filter(|request| request.method == "PATCH")
                .map(|request| (request.path, request.body))
                .collect();
            let body = |units: u32| {
                format!(
                    r#"{{"quantity":{units},"type":"AVAILABLE","channel":{{"id":"9fe0d853-732b-4e4a-a0b0-cff988ed043d"}}}}"#
                )
            };
            // The camiseta goes down from 7 before the fone goes up from 0;
            // the owner's cup keeps what it had.
            assert_eq!(
                sent,
                [
                    (
                        "/seller/v1/portfolios/stocks/CAM-PRETA-M".to_owned(),
                        body(2)
                    ),
                    ("/seller/v1/portfolios/stocks/FONE-TWS".to_owned(), body(3)),
                ]
            );
        });
    }

    #[test]
    fn a_development_build_can_point_at_the_sandbox_or_a_fake() {
        let pointing = |url: &'static str| -> Environment {
            Box::new(move |name: &str| (name == "MASCATE_MAGALU_API_URL").then(|| url.to_owned()))
        };
        assert_eq!(
            urls(
                Build::Development,
                &pointing(mascate_integrations::MAGALU_SANDBOX_URL)
            ),
            (
                mascate_integrations::MAGALU_SANDBOX_URL.to_owned(),
                mascate_integrations::MAGALU_ID_URL.to_owned()
            )
        );
        assert_eq!(
            urls(Build::Development, &pointing("http://127.0.0.1:8094")),
            (
                "http://127.0.0.1:8094".to_owned(),
                "http://127.0.0.1:8094".to_owned()
            )
        );
        assert_eq!(
            urls(Build::Release, &pointing("http://127.0.0.1:8094")).0,
            mascate_integrations::MAGALU_API_URL
        );
    }

    #[test]
    fn a_refused_code_says_to_log_in_again() {
        assert!(
            login_failure(&PlatformError::Refused("invalid_grant".into()))
                .starts_with("A Magalu recusou o código (invalid_grant).")
        );
    }
}
