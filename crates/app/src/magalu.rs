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

    use chrono::{NaiveDate, TimeDelta, TimeZone, Utc};
    use futures::executor::block_on;
    use mascate_catalog::Catalog;
    use mascate_commerce::{
        ChannelBilling, ChannelOrders, Listings, NewPurchaseLine, NewPurchaseOrder, Orders,
        PriceAssumptions, Pricing, PurchaseOrders, Receiving, SaleFee, StockMirror,
    };
    use mascate_integrations::Connection;
    use mascate_inventory::{HOME_LOCATION, Inventory};
    use mascate_kernel::testing::{ManualClock, SequentialIds};
    use mascate_kernel::{Clock, Currency, IdGenerator, Money, Percentage, RecordId};
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

    /// The store's SKUs read by a Sync and linked to Products that came in
    /// through a purchase: 2 camisetas, 3 fones and 4 canecas at R$ 20.
    struct LinkedStore {
        _dir: tempfile::TempDir,
        manual: Arc<ManualClock>,
        database: Arc<Database>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
        inventory: Arc<Inventory>,
        listings: Listings,
        server: FakeHttpServer,
        adapter: Arc<Magalu>,
    }

    async fn linked_store() -> LinkedStore {
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
        let clock: Arc<dyn Clock> = manual.clone();
        let ids: Arc<dyn IdGenerator> = Arc::new(SequentialIds::default());
        let inventory = Arc::new(Inventory::new(database.clone(), clock.clone(), ids.clone()));
        let listings = Listings::new(database.clone(), clock.clone(), ids.clone());
        let (server, adapter) = magalu();
        listings.sync(adapter.as_ref()).await.unwrap();

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
        LinkedStore {
            _dir: dir,
            manual,
            database,
            clock,
            ids,
            inventory,
            listings,
            server,
            adapter,
        }
    }

    #[test]
    fn a_round_sends_each_linked_sku_its_share_lowering_first() {
        block_on(async {
            // The camiseta shows 7 on Magalu and the fone none; the owner
            // turned the cup off in Magalu's panel.
            let LinkedStore {
                _dir: dir,
                database,
                clock,
                ids,
                inventory,
                server,
                adapter,
                ..
            } = linked_store().await;
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
                repricing: None,
                quality: None,
                promotions: None,
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

    /// The linked store with Orders and Fees, its Orders read and its
    /// stock sent by each round.
    struct SellingStore {
        store: LinkedStore,
        orders: Orders,
        mirror: StockMirror,
        catalog: Catalog,
    }

    impl SellingStore {
        async fn new() -> Self {
            let store = linked_store().await;
            Self {
                orders: Orders::new(
                    store.database.clone(),
                    store.inventory.clone(),
                    store.clock.clone(),
                    store.ids.clone(),
                ),
                mirror: StockMirror::new(
                    store.database.clone(),
                    store.inventory.clone(),
                    store.clock.clone(),
                    store.ids.clone(),
                ),
                catalog: Catalog::new(
                    store.database.clone(),
                    store._dir.path().join("files"),
                    store.clock.clone(),
                    store.ids.clone(),
                ),
                store,
            }
        }

        /// Magalu answers every Order search with the fixture `orders`, and
        /// the financial analysis of the delivered Order.
        fn sold(&self, orders: &str) {
            let server = &self.store.server;
            server.serve_any_query("/seller/v1/orders", 200, fixture(orders));
            server.serve_any_query(
                "/seller/v1/financial-analysis/orders",
                200,
                fixture("financial-analysis-delivered.json"),
            );
        }

        async fn round(&self) -> orders::Round {
            let adapter = &self.store.adapter;
            let ports = ChannelPorts {
                connection: Connection::Magalu,
                listings: adapter.clone(),
                orders: Some(adapter.clone() as Arc<dyn ChannelOrders>),
                billing: Some(adapter.clone() as Arc<dyn ChannelBilling>),
                payments: None,
                labels: None,
                shipments: None,
                publisher: None,
                repricing: None,
                quality: None,
                promotions: None,
            };
            orders::round(&self.orders, &self.mirror, &self.catalog, &[ports])
                .await
                .unwrap()
                .unwrap()
        }

        /// The last units each SKU was sent.
        fn last_sent(&self, sku: &str) -> Option<String> {
            self.store
                .server
                .received()
                .into_iter()
                .rev()
                .find(|request| {
                    request.method == "PATCH"
                        && request.path == format!("/seller/v1/portfolios/stocks/{sku}")
                })
                .map(|request| request.body)
        }
    }

    #[test]
    fn a_magalu_sale_leaves_the_stock_once_before_the_next_stock_send() {
        block_on(async {
            // The first Order Sync, at noon, finds nothing.
            let shop = SellingStore::new().await;
            shop.sold("orders-empty.json");
            assert_eq!(shop.round().await.synced.taken, 0);

            // By two o'clock a camiseta and three fones sold, and a cup sold
            // and was cancelled before it shipped.
            shop.store.manual.advance(TimeDelta::hours(2));
            shop.sold("orders-page.json");
            let second = shop.round().await;
            assert_eq!((second.synced.new, second.synced.taken), (4, 3));
            let units = |left: u32| {
                Some(format!(
                    r#"{{"quantity":{left},"type":"AVAILABLE","channel":{{"id":"9fe0d853-732b-4e4a-a0b0-cff988ed043d"}}}}"#
                ))
            };
            // What is left is all Magalu is offered: no fone sold twice.
            assert_eq!(shop.last_sent("CAM-PRETA-M"), units(1));
            assert_eq!(shop.last_sent("FONE-TWS"), units(0));

            // Read again, the same Orders change nothing.
            shop.store.manual.advance(TimeDelta::minutes(5));
            let third = shop.round().await;
            assert_eq!(
                (third.synced.new, third.synced.changed, third.synced.taken),
                (0, 0, 0)
            );
            let stock = shop.store.inventory.stock().await.unwrap();
            let left: Vec<i64> = [0xca, 0xf0, 0xcc]
                .into_iter()
                .map(|product| {
                    stock
                        .products
                        .iter()
                        .find(|found| found.product == RecordId::from_u128(product))
                        .map_or(0, |found| found.valuation.quantity())
                })
                .collect();
            assert_eq!(left, [1, 0, 4]);
        });
    }

    #[test]
    fn the_financial_analysis_turns_a_delivered_sales_margin_realized() {
        block_on(async {
            let shop = SellingStore::new().await;
            shop.sold("orders-empty.json");
            shop.round().await;
            shop.store.manual.advance(TimeDelta::hours(2));
            shop.sold("orders-page.json");
            shop.round().await;

            let now = shop.store.clock.now();
            let period = shop
                .orders
                .sales(
                    Percentage::new(0.into()).unwrap(),
                    now - TimeDelta::days(1)..now,
                    &[],
                )
                .await
                .unwrap();
            let margin = |id: &str| {
                period
                    .sales
                    .iter()
                    .find(|sale| sale.order.sold.id == id)
                    .map(|sale| (sale.margin.provisional, sale.margin.fees_total()))
                    .unwrap()
            };
            let brl = |amount: &str| Money::new(amount.parse().unwrap(), Currency::Brl);
            // Delivered: commission, two fixed fees, the payment fee and the
            // freight less what Magalu covered.
            assert_eq!(margin("1502070665817671"), (false, brl("64.46")));
            // Waiting for the NF-e: the commission the Order reports, for
            // now.
            assert_eq!(margin("1502070665817670"), (true, brl("14.98")));
            assert_eq!(
                shop.orders.raised_reminders().await.unwrap(),
                [mascate_commerce::MAGALU_SALE_INVOICE.key]
            );
        });
    }

    #[test]
    fn a_linked_sku_is_priced_with_the_fee_table() {
        block_on(async {
            let store = linked_store().await;
            let pricing = Pricing::new(store.database, store.inventory, store.clock, store.ids);
            let camiseta = store
                .listings
                .listings()
                .await
                .unwrap()
                .into_iter()
                .find(|listing| listing.listed.id == "CAM-PRETA-M")
                .unwrap();
            let assumptions = PriceAssumptions {
                tax: Percentage::new(6.into()).unwrap(),
                estimated_shipping: Money::new(20.into(), Currency::Brl),
            };

            let today = pricing
                .today(camiseta.id, store.adapter.as_ref(), assumptions)
                .await
                .unwrap();

            // R$ 49,90 is above R$ 10, so the table adds R$ 5 to its 20%.
            assert_eq!(
                today.sale_fee,
                SaleFee {
                    rate: Percentage::new(20.into()).unwrap(),
                    fixed: Money::new(5.into(), Currency::Brl),
                }
            );
            assert_eq!(today.cost, Money::new(20.into(), Currency::Brl));
            assert!(
                pricing
                    .suggest(camiseta.id, store.adapter.as_ref(), assumptions)
                    .await
                    .is_ok()
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
