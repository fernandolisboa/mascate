mod common;

use futures::executor::block_on;
use mascate_catalog::{CatalogError, InvalidLink, NewSupplierOffer, ProductSource};
use mascate_kernel::{Currency, Money};
use rust_decimal::Decimal;

use common::{Fixture, brl, new_offer};

const SHOPEE_FONE: &str = "https://shopee.com.br/Fone-Bluetooth-TWS-i.123.456";

#[test]
fn registers_suppliers_by_name_and_refuses_the_same_name_in_another_case() {
    block_on(async {
        let f = Fixture::new().await;
        f.supplier("Loja Zeta").await;
        let alpha = f.catalog.add_supplier("  Atacado   Alfa ").await.unwrap();

        assert_eq!(alpha.name, "Atacado Alfa");
        let names: Vec<_> = f
            .catalog
            .suppliers()
            .await
            .unwrap()
            .into_iter()
            .map(|supplier| supplier.name)
            .collect();
        assert_eq!(names, ["Atacado Alfa", "Loja Zeta"]);
        assert!(matches!(
            f.catalog.add_supplier("loja zeta").await,
            Err(CatalogError::SupplierExists(_))
        ));
        assert!(matches!(
            f.catalog.add_supplier("   ").await,
            Err(CatalogError::MissingSupplierName)
        ));
    });
}

#[test]
fn a_manual_offer_keeps_its_decimal_price_shipping_currency_and_time() {
    block_on(async {
        let f = Fixture::new().await;
        let shop = f.supplier("Loja Fones").await;

        let offer = f
            .catalog
            .register_offer(NewSupplierOffer {
                supplier: shop.id,
                link: format!("  {SHOPEE_FONE}  "),
                title: " Fone  TWS ".into(),
                price: brl("29.90"),
                shipping: brl("0.10"),
            })
            .await
            .unwrap();

        assert_eq!(offer.supplier, shop);
        assert_eq!(offer.link, SHOPEE_FONE);
        assert_eq!(offer.title, "Fone TWS");
        assert_eq!(offer.source, ProductSource::Manual);
        assert_eq!(offer.price, brl("29.90"));
        assert_eq!(offer.price.amount().scale(), 2);
        assert_eq!(offer.total(), brl("30.00"));
        assert_eq!(offer.observed_at, f.clock_now());
        assert_eq!(offer.product, None);
        assert_eq!(f.catalog.offer(offer.id).await.unwrap(), offer);
    });
}

#[test]
fn an_offer_in_dollars_stays_in_dollars() {
    block_on(async {
        let f = Fixture::new().await;
        let shop = f.supplier("AliExpress Store").await;
        let usd = |amount: i64| Money::new(Decimal::new(amount, 2), Currency::Usd);

        let offer = f
            .catalog
            .register_offer(NewSupplierOffer {
                supplier: shop.id,
                link: "https://pt.aliexpress.com/item/1005.html".into(),
                title: "Cabo USB-C".into(),
                price: usd(399),
                shipping: usd(150),
            })
            .await
            .unwrap();

        assert_eq!(offer.total(), usd(549));
    });
}

#[test]
fn refuses_offers_it_cannot_trust() {
    block_on(async {
        let f = Fixture::new().await;
        let shop = f.supplier("Loja").await;
        let refused = |offer: NewSupplierOffer| {
            let catalog = &f.catalog;
            async move { catalog.register_offer(offer).await.unwrap_err() }
        };

        let mut negative = new_offer(&shop, SHOPEE_FONE, "Fone", "-1");
        assert!(matches!(
            refused(negative.clone()).await,
            CatalogError::Negative
        ));
        negative.price = brl("1");
        negative.shipping = brl("-0.01");
        assert!(matches!(refused(negative).await, CatalogError::Negative));

        let mut mixed = new_offer(&shop, SHOPEE_FONE, "Fone", "1");
        mixed.shipping = Money::zero(Currency::Usd);
        assert!(matches!(refused(mixed).await, CatalogError::Currencies(_)));

        for (link, why) in [
            ("", InvalidLink::Empty),
            ("shopee.com.br/item", InvalidLink::NotWeb),
            ("javascript:alert(1)", InvalidLink::NotWeb),
            ("file:///C:/Windows/notepad.exe", InvalidLink::NotWeb),
            ("https:///path", InvalidLink::NoHost),
            ("https://shopee.com.br/a b", InvalidLink::Space),
        ] {
            let error = refused(new_offer(&shop, link, "Fone", "1")).await;
            assert!(
                matches!(&error, CatalogError::InvalidLink(found) if *found == why),
                "{link}: {error}"
            );
        }

        assert!(matches!(
            refused(new_offer(&shop, SHOPEE_FONE, "  ", "1")).await,
            CatalogError::MissingTitle
        ));
        let mut unknown = new_offer(&shop, SHOPEE_FONE, "Fone", "1");
        unknown.supplier = uuid_of(999);
        assert!(matches!(
            refused(unknown).await,
            CatalogError::UnknownSupplier(_)
        ));
        assert!(f.catalog.offers().await.unwrap().is_empty());
    });
}

#[test]
fn a_new_price_at_the_same_link_adds_to_its_history() {
    block_on(async {
        let f = Fixture::new().await;
        let shop = f.supplier("Loja Fones").await;
        let first = f.offer(&shop, SHOPEE_FONE, "39.90").await;
        f.a_day_later();
        let other = f
            .offer(&shop, "https://shopee.com.br/Carregador-i.123.789", "19.90")
            .await;
        f.a_day_later();
        // The same item shared again, with fresh tracking in the query.
        let again = f
            .offer(
                &shop,
                &format!("{SHOPEE_FONE}?sp_atk=abc&xptdk=def#reviews"),
                "34.90",
            )
            .await;

        let histories = f.catalog.offers().await.unwrap();

        assert_eq!(histories.len(), 2);
        assert_eq!(histories[0].latest, again);
        assert_eq!(histories[0].earlier, std::slice::from_ref(&first));
        let prices: Vec<_> = histories[0].offers().map(|offer| offer.price).collect();
        assert_eq!(prices, [brl("34.90"), brl("39.90")]);
        assert_eq!(histories[0].lowest_total(), brl("39.90"));
        assert_eq!(histories[1].latest, other);
        assert!(histories[1].earlier.is_empty());
    });
}

#[test]
fn links_count_as_the_same_item_only_when_they_name_it() {
    block_on(async {
        let f = Fixture::new().await;
        let shop = f.supplier("Atacado").await;
        for link in [
            "https://atacado.example/produto?id=1&utm_source=whatsapp",
            "HTTP://Atacado.Example/produto/?id=1",
            "https://atacado.example/produto?id=2",
            "https://www.mercadolivre.com.br/fone/p/MLB123?pdp_filters=x",
            "https://www.mercadolivre.com.br/fone/p/MLB123",
        ] {
            f.offer(&shop, link, "10").await;
        }

        let sizes: Vec<_> = f
            .catalog
            .offers()
            .await
            .unwrap()
            .iter()
            .map(|history| history.offers().count())
            .collect();

        // Newest first: the Mercado Livre item twice, id=2 once, id=1 twice.
        assert_eq!(sizes, [2, 1, 2]);
    });
}

#[test]
fn a_product_made_from_an_offer_takes_its_whole_link_and_a_folder() {
    block_on(async {
        let f = Fixture::new().await;
        let shop = f.supplier("Loja Fones").await;
        let first = f.offer(&shop, SHOPEE_FONE, "39.90").await;
        let second = f.offer(&shop, SHOPEE_FONE, "34.90").await;
        let unrelated = f
            .offer(&shop, "https://shopee.com.br/Outro-i.1.2", "9.90")
            .await;

        let product = f
            .catalog
            .create_product(first.id, "  Fone   TWS ", "fon-blu-001")
            .await
            .unwrap();

        assert_eq!(product.name, "Fone TWS");
        assert_eq!(product.sku.as_str(), "FON-BLU-001");
        assert!(f.catalog.folder(&product).is_dir());
        assert!(f.catalog.folder(&product).ends_with("produtos/FON-BLU-001"));
        assert_eq!(
            f.catalog.products().await.unwrap(),
            std::slice::from_ref(&product)
        );
        assert_eq!(f.catalog.product(product.id).await.unwrap(), product);
        for offer in [first.id, second.id] {
            assert_eq!(
                f.catalog.offer(offer).await.unwrap().product,
                Some(product.id)
            );
        }
        assert_eq!(f.catalog.offer(unrelated.id).await.unwrap().product, None);
        let history = f.catalog.product_offers(product.id).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].offers().count(), 2);

        // A later price at that link joins the Product on its own.
        let later = f.offer(&shop, SHOPEE_FONE, "32.00").await;
        assert_eq!(later.product, Some(product.id));
    });
}

#[test]
fn a_product_links_offers_from_several_suppliers_and_shows_each_history() {
    block_on(async {
        let f = Fixture::new().await;
        let shopee = f.supplier("Loja Shopee").await;
        let wholesale = f.supplier("Atacado SP").await;
        let first = f.offer(&shopee, SHOPEE_FONE, "39.90").await;
        let product = f
            .catalog
            .create_product(first.id, "Fone TWS", "FONE-TWS")
            .await
            .unwrap();
        f.a_day_later();
        let wholesale_offer = f
            .offer(&wholesale, "https://atacadosp.example/fone-tws", "25.00")
            .await;
        f.a_day_later();
        f.offer(&wholesale, "https://atacadosp.example/fone-tws", "23.50")
            .await;

        f.catalog
            .link_offer(wholesale_offer.id, product.id)
            .await
            .unwrap();

        let histories = f.catalog.product_offers(product.id).await.unwrap();
        let summary: Vec<(String, Vec<Money>)> = histories
            .iter()
            .map(|history| {
                (
                    history.latest.supplier.name.clone(),
                    history.offers().map(|offer| offer.total()).collect(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("Atacado SP".into(), vec![brl("28.50"), brl("30.00")]),
                ("Loja Shopee".into(), vec![brl("44.90")]),
            ]
        );
    });
}

#[test]
fn linking_a_link_to_another_product_moves_its_whole_history() {
    block_on(async {
        let f = Fixture::new().await;
        let shop = f.supplier("Loja").await;
        let a = f.offer(&shop, "https://loja.example/a", "10").await;
        let b = f.offer(&shop, "https://loja.example/b", "20").await;
        f.offer(&shop, "https://loja.example/a", "11").await;
        let first = f.catalog.create_product(a.id, "A", "A").await.unwrap();
        let second = f.catalog.create_product(b.id, "B", "B").await.unwrap();

        f.catalog.link_offer(a.id, second.id).await.unwrap();

        assert!(f.catalog.product_offers(first.id).await.unwrap().is_empty());
        let moved = f.catalog.product_offers(second.id).await.unwrap();
        assert_eq!(moved.len(), 2);
        assert_eq!(moved.iter().map(|h| h.offers().count()).sum::<usize>(), 3);
        assert!(matches!(
            f.catalog.link_offer(uuid_of(999), second.id).await,
            Err(CatalogError::UnknownOffer(_))
        ));
        assert!(matches!(
            f.catalog.link_offer(a.id, uuid_of(999)).await,
            Err(CatalogError::UnknownProduct(_))
        ));
    });
}

#[test]
fn skus_are_unique_and_a_refused_product_leaves_no_folder() {
    block_on(async {
        let f = Fixture::new().await;
        let shop = f.supplier("Loja").await;
        let a = f.offer(&shop, "https://loja.example/a", "10").await;
        let b = f.offer(&shop, "https://loja.example/b", "10").await;
        f.catalog.create_product(a.id, "A", "ABC-1").await.unwrap();

        assert!(matches!(
            f.catalog.create_product(b.id, "B", " abc 1 ").await,
            Err(CatalogError::SkuTaken(sku)) if sku.as_str() == "ABC-1"
        ));
        assert!(matches!(
            f.catalog.create_product(b.id, "B", "CON").await,
            Err(CatalogError::InvalidSku(_))
        ));
        assert!(matches!(
            f.catalog.create_product(b.id, "B", "../../x").await,
            Err(CatalogError::InvalidSku(_))
        ));
        assert!(matches!(
            f.catalog.create_product(b.id, "  ", "NEW").await,
            Err(CatalogError::MissingProductName)
        ));
        assert!(matches!(
            f.catalog.create_product(uuid_of(999), "B", "NEW").await,
            Err(CatalogError::UnknownOffer(_))
        ));
        assert_eq!(f.catalog.products().await.unwrap().len(), 1);
        let folders: Vec<_> = std::fs::read_dir(f.dir.path().join("produtos"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(folders, ["ABC-1"]);
    });
}

#[test]
fn suggests_a_readable_sku_from_the_name_numbered_past_the_taken_ones() {
    block_on(async {
        let f = Fixture::new().await;
        let shop = f.supplier("Loja").await;
        let name = "Fone de Ouvido Bluetooth Ção";

        let first = f.catalog.suggest_sku(name).await.unwrap();
        assert_eq!(first.as_str(), "FON-OUV-BLU-001");

        let offer = f.offer(&shop, "https://loja.example/a", "10").await;
        f.catalog
            .create_product(offer.id, name, first.as_str())
            .await
            .unwrap();
        // A folder left by hand under the next number is not reused.
        std::fs::create_dir_all(f.dir.path().join("produtos/FON-OUV-BLU-002")).unwrap();

        assert_eq!(
            f.catalog.suggest_sku(name).await.unwrap().as_str(),
            "FON-OUV-BLU-003"
        );
        assert_eq!(
            f.catalog.suggest_sku("Pá de Ação").await.unwrap().as_str(),
            "ACA-001"
        );
        assert_eq!(
            f.catalog.suggest_sku("!! ?").await.unwrap().as_str(),
            "PROD-001"
        );
    });
}

#[test]
fn renaming_the_sku_moves_the_folder_with_its_files() {
    block_on(async {
        let f = Fixture::new().await;
        let shop = f.supplier("Loja").await;
        let offer = f.offer(&shop, SHOPEE_FONE, "10").await;
        let product = f
            .catalog
            .create_product(offer.id, "Fone", "FON-001")
            .await
            .unwrap();
        std::fs::write(f.catalog.folder(&product).join("nota.pdf"), b"nota").unwrap();

        let renamed = f
            .catalog
            .rename_sku(product.id, "fone tws 1")
            .await
            .unwrap();

        assert_eq!(renamed.sku.as_str(), "FONE-TWS-1");
        assert_eq!(f.catalog.product(product.id).await.unwrap(), renamed);
        assert!(!f.catalog.folder(&product).exists());
        let files = f.catalog.files(product.id).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(std::fs::read(&files[0].path).unwrap(), b"nota");
        assert!(files[0].path.starts_with(f.catalog.folder(&renamed)));
        assert_eq!(
            f.catalog
                .rename_sku(product.id, "FONE-TWS-1")
                .await
                .unwrap(),
            renamed
        );
    });
}

#[test]
fn a_refused_rename_changes_nothing() {
    block_on(async {
        let f = Fixture::new().await;
        let shop = f.supplier("Loja").await;
        let a = f.offer(&shop, "https://loja.example/a", "10").await;
        let b = f.offer(&shop, "https://loja.example/b", "10").await;
        let first = f.catalog.create_product(a.id, "A", "A-1").await.unwrap();
        f.catalog.create_product(b.id, "B", "B-1").await.unwrap();
        std::fs::create_dir_all(f.dir.path().join("produtos/LEFT-OVER")).unwrap();

        assert!(matches!(
            f.catalog.rename_sku(first.id, "b-1").await,
            Err(CatalogError::SkuTaken(_))
        ));
        assert!(matches!(
            f.catalog.rename_sku(first.id, "left over").await,
            Err(CatalogError::FolderTaken(_))
        ));
        assert!(matches!(
            f.catalog.rename_sku(first.id, "").await,
            Err(CatalogError::InvalidSku(_))
        ));
        assert_eq!(f.catalog.product(first.id).await.unwrap(), first);
        assert!(f.catalog.folder(&first).is_dir());
    });
}

fn uuid_of(n: u128) -> mascate_kernel::RecordId {
    mascate_kernel::RecordId::from_u128(n)
}

impl Fixture {
    fn clock_now(&self) -> mascate_kernel::Timestamp {
        use mascate_kernel::Clock;
        self.clock.now()
    }
}

#[test]
fn a_folder_already_named_after_a_typed_sku_is_adopted_with_its_files() {
    block_on(async {
        let f = Fixture::new().await;
        let shop = f.supplier("Loja").await;
        let offer = f.offer(&shop, SHOPEE_FONE, "10").await;
        let folder = f.dir.path().join("produtos/FONE-1");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("foto.jpg"), b"jpg").unwrap();

        let product = f
            .catalog
            .create_product(offer.id, "Fone", "fone-1")
            .await
            .unwrap();

        let files = f.catalog.files(product.id).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "foto.jpg");
    });
}
