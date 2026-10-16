//! Shopee as a Sales Channel (#129): where the seller adapter points, the
//! address Shopee's authorization page sends the browser back to, and how a
//! failed login reads in Portuguese.

use std::sync::Arc;

use gpui_kit::{App, Global};
use mascate_integrations::ShopeeSeller;
use mascate_kernel::PlatformError;
use mascate_platform::{Build, Environment};

/// Where Shopee sends the browser after the owner authorizes the app. The
/// app has no address of its own yet (#10), so the owner copies the one
/// the browser lands on, code and shop id included, back into the app; the
/// app's redirect domain in Shopee's console is this one.
pub const REDIRECT: &str = "https://github.com/fernandolisboa";

/// The Shopee seller adapter, for the login in Configurações › Conexões.
pub struct AppShopeeSeller(pub Arc<ShopeeSeller>);

impl Global for AppShopeeSeller {}

pub fn adapter(cx: &App) -> Option<Arc<ShopeeSeller>> {
    cx.try_global::<AppShopeeSeller>().map(|app| app.0.clone())
}

/// Where the app reaches the Open Platform: Shopee's host for the Americas
/// or, in a development build only, the URL in
/// `MASCATE_SHOPEE_SELLER_API_URL`: Shopee's sandbox or a local fake
/// server.
pub fn api_url(build: Build, environment: &Environment) -> String {
    crate::mercado_livre::local_fake(build, environment, "MASCATE_SHOPEE_SELLER_API_URL")
        .unwrap_or_else(|| mascate_integrations::SHOPEE_SELLER_API_URL.to_owned())
}

/// Why opening Shopee's authorization page or trading its code failed.
pub fn login_failure(error: &PlatformError) -> String {
    match error {
        PlatformError::NotConnected => {
            "Salve o Partner ID e a Partner Key antes de entrar na Shopee.".into()
        }
        PlatformError::Expired => {
            "A Shopee não aceitou a assinatura: confira o Partner ID, a Partner Key e se o \
             relógio do computador está certo."
                .into()
        }
        PlatformError::Refused(why) => format!(
            "A Shopee recusou o código ({why}). Ele vale 10 minutos e uma vez só: clique em \
             Entrar na Shopee de novo e cole o endereço novo."
        ),
        PlatformError::RateLimited => {
            "A Shopee pediu para esperar. Tente de novo daqui a alguns minutos.".into()
        }
        PlatformError::NotFound => "A Shopee não encontrou o app ou a loja.".into(),
        PlatformError::Failed(why) => format!("Não consegui falar com a Shopee: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use chrono::NaiveDate;
    use chrono::{TimeZone, Utc};
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
            "../integrations/tests/fixtures/shopee-seller",
            name,
        ]
        .iter()
        .collect();
        std::fs::read_to_string(&path).unwrap()
    }

    /// Shopee with the shop's three items on one page; every call is
    /// signed with the time, so the routes take any query.
    fn shopee() -> (FakeHttpServer, Arc<ShopeeSeller>) {
        let server = FakeHttpServer::start();
        let one_page = fixture("item-list-page-1.json")
            .replace(
                r#"{"item_id": 800002, "item_status": "NORMAL", "update_time": 1792100100}"#,
                r#"{"item_id": 800002, "item_status": "NORMAL", "update_time": 1792100100},
                   {"item_id": 800003, "item_status": "UNLIST", "update_time": 1792100200}"#,
            )
            .replace(r#""has_next_page": true"#, r#""has_next_page": false"#);
        for (path, body) in [
            (
                "/api/v2/auth/access_token/get",
                fixture("access-token-get.json"),
            ),
            ("/api/v2/product/get_item_list", one_page),
            (
                "/api/v2/product/get_item_base_info",
                fixture("item-base-info.json"),
            ),
            (
                "/api/v2/product/get_model_list",
                fixture("model-list-camiseta.json"),
            ),
            ("/api/v2/product/get_category", fixture("categories.json")),
            ("/api/v2/product/update_stock", fixture("update-ok.json")),
        ] {
            server.serve_any_query(path, 200, body);
        }
        let store = Arc::new(MemorySecretStore::default());
        for (name, value) in [
            ("SHOPEE_PARTNER_ID", "2001887"),
            ("SHOPEE_PARTNER_KEY", "shpk-mascate-partner-key"),
            ("SHOPEE_SHOP_ID", "14701711"),
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
        (server, adapter)
    }

    #[test]
    fn a_round_sends_each_linked_model_its_share_lowering_first() {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let clock = Arc::new(ManualClock::at(
                Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
            ));
            let database = Arc::new(
                Database::open(&dir.path().join("mascate.db"))
                    .await
                    .unwrap(),
            );
            migrate(&database, clock.as_ref(), MODULE_MIGRATIONS)
                .await
                .unwrap();
            let clock: Arc<dyn Clock> = clock;
            let ids: Arc<dyn IdGenerator> = Arc::new(SequentialIds::default());
            let inventory = Arc::new(Inventory::new(database.clone(), clock.clone(), ids.clone()));
            let listings = Listings::new(database.clone(), clock.clone(), ids.clone());
            let mirror = StockMirror::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            );
            let orders = Orders::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            );
            let catalog = Catalog::new(
                database.clone(),
                dir.path().join("files"),
                clock.clone(),
                ids.clone(),
            );
            let (server, adapter) = shopee();
            listings.sync(adapter.as_ref()).await.unwrap();

            // The fone shows 7 on Shopee, the black M camiseta is sold out
            // and the owner unlisted the cup.
            let fone = RecordId::from_u128(0xf0);
            let camiseta = RecordId::from_u128(0xca);
            let caneca = RecordId::from_u128(0xcc);
            let purchases = PurchaseOrders::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            );
            for (product, units) in [(fone, 3), (camiseta, 2), (caneca, 4)] {
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
                let product = match (
                    listing.listed.id.as_str(),
                    listing.listed.variation.as_ref().map(|v| v.id.as_str()),
                ) {
                    ("800001", None) => fone,
                    ("800002", Some("900022")) => camiseta,
                    ("800003", None) => caneca,
                    _ => continue,
                };
                listings.link(listing.id, product).await.unwrap();
            }
            let channels = [ChannelPorts {
                connection: Connection::ShopeeSeller,
                listings: adapter,
                orders: None,
                billing: None,
                payments: None,
                labels: None,
                shipments: None,
                publisher: None,
            }];

            let round = orders::round(&orders, &mirror, &catalog, &channels)
                .await
                .unwrap()
                .unwrap();

            assert_eq!((round.sent.sent, round.sent.failed), (2, 0));
            let sent: Vec<String> = server
                .received()
                .into_iter()
                .filter(|request| request.path.starts_with("/api/v2/product/update_stock?"))
                .map(|request| request.body)
                .collect();
            // The fone goes down from 7 before the camiseta goes up from 0;
            // the owner's unlisted cup keeps what it had.
            assert_eq!(
                sent,
                [
                    r#"{"item_id":800001,"stock_list":[{"model_id":0,"seller_stock":[{"stock":3}]}]}"#,
                    r#"{"item_id":800002,"stock_list":[{"model_id":900022,"seller_stock":[{"stock":2}]}]}"#,
                ]
            );
        });
    }

    #[test]
    fn a_refused_code_says_to_log_in_again() {
        assert!(
            login_failure(&PlatformError::Refused("Invalid code.".into()))
                .starts_with("A Shopee recusou o código (Invalid code.).")
        );
    }

    #[test]
    fn a_development_build_can_point_at_the_sandbox() {
        let environment: Environment = Box::new(|name: &str| {
            (name == "MASCATE_SHOPEE_SELLER_API_URL")
                .then(|| mascate_integrations::SHOPEE_SELLER_SANDBOX_URL.to_owned())
        });
        assert_eq!(
            api_url(Build::Development, &environment),
            mascate_integrations::SHOPEE_SELLER_SANDBOX_URL
        );
        assert_eq!(
            api_url(Build::Release, &environment),
            mascate_integrations::SHOPEE_SELLER_API_URL
        );
    }
}
