//! The catalog's Sync of demand through Mercado Livre's adapter, end to end
//! against the fake server and a real temporary database.

use std::str::FromStr;
use std::sync::Arc;

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_catalog::{Catalog, DemandCategory, NewSupplierOffer, OpportunityFilter};
use mascate_integrations::MercadoLivre;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, Percentage};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use rust_decimal::Decimal;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/mercado-livre/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

#[test]
fn a_supplier_offer_becomes_an_opportunity_priced_from_mercado_livre() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
        ));
        let database = Arc::new(Database::open(&dir.path().join("m.db")).await.unwrap());
        migrate(&database, clock.as_ref(), &[mascate_catalog::MIGRATIONS])
            .await
            .unwrap();
        let catalog = Catalog::new(
            database,
            dir.path().join("produtos"),
            clock.clone(),
            Arc::new(SequentialIds::default()),
        );
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        for (name, value) in [
            ("ML_CLIENT_ID", "1"),
            ("ML_CLIENT_SECRET", "2"),
            ("ML_REFRESH_TOKEN", "TG-first"),
        ] {
            store.write(name, &Secret::new(value)).unwrap();
        }
        let serve = |path: &str, name: &str| server.serve(path, 200, fixture(name));
        serve("/oauth/token", "token.json");
        serve(
            "/highlights/MLB/category/MLB1000",
            "highlights-MLB1000.json",
        );
        serve("/products/MLB19615318", "product-MLB19615318.json");
        serve(
            "/products/search?status=active&site_id=MLB&q=Fone%20Bluetooth%20TWS&limit=5",
            "products-search.json",
        );
        serve(
            "/products/MLB19615318/items?limit=50",
            "product-items-MLB19615318.json",
        );
        serve("/categories/MLB3697", "category-MLB3697.json");
        serve(
            "/sites/MLB/listing_prices?price=103.23&listing_type_id=gold_special&category_id=MLB3697",
            "listing-prices.json",
        );
        let mercado_livre = MercadoLivre::new(server.url(), "Mascate/test", store, clock);

        let shop = catalog.add_supplier("Loja Fones").await.unwrap();
        catalog
            .register_offer(NewSupplierOffer {
                supplier: shop.id,
                link: "https://shopee.com.br/Fone-i.1.2".into(),
                title: "Fone Bluetooth TWS".into(),
                price: brl("30"),
                shipping: brl("5"),
            })
            .await
            .unwrap();
        catalog
            .follow_categories(&[DemandCategory {
                id: "MLB1000".into(),
                name: "Eletrônicos, Áudio e Vídeo".into(),
            }])
            .await
            .unwrap();

        let report = catalog.sync_demand(&mercado_livre).await.unwrap();
        let again = catalog.sync_demand(&mercado_livre).await.unwrap();
        let listed = catalog
            .opportunities(&OpportunityFilter::default(), Percentage::ZERO)
            .await
            .unwrap();

        assert_eq!(report, again);
        assert_eq!(report.best_sellers, 1);
        assert_eq!(report.opportunities, 1);
        let [opportunity] = &listed[..] else {
            panic!("one opportunity: {listed:?}");
        };
        assert_eq!(opportunity.product_name, "Fone de Ouvido Bluetooth TWS Pro");
        assert_eq!(opportunity.competitors, 7);
        assert_eq!(opportunity.sale_price.rounded(), brl("103.23"));
        assert_eq!(opportunity.sale_fee.amount, brl("13.99"));
        assert!(!opportunity.sale_fee.estimated);
        assert_eq!(opportunity.best_seller, Some(1));
        assert_eq!(
            opportunity.category.as_ref().map(|c| c.name.as_str()),
            Some("Eletrônicos, Áudio e Vídeo")
        );
        // 103,233... - 13,99 fee - 20 shipping - 35 cost.
        assert_eq!(opportunity.margin.amount.rounded(), brl("34.24"));
        let best_sellers = catalog.best_sellers().await.unwrap();
        assert!(best_sellers[0].best_sellers[0].has_opportunity);
    });
}
