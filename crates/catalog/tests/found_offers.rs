mod common;

use std::str::FromStr;
use std::sync::Mutex;

use futures::executor::block_on;
use mascate_catalog::{
    BestSeller, CatalogError, CatalogMatch, Competition, DemandCategory, DemandSource, Found,
    FoundOffer, FoundShop, KeptOffers, OfferSearch, OfferSource, OpportunityFilter, ProductSource,
};
use mascate_kernel::{Clock as _, Currency, ListingType, Money, Percentage, PlatformError};
use rust_decimal::Decimal;

use common::{Fixture, brl, new_offer};

const SHOPEE: ProductSource = ProductSource::ShopeeAffiliate;

fn shop(id: &str, name: &str) -> FoundShop {
    FoundShop {
        id: id.into(),
        name: name.into(),
        link: format!("https://shopee.com.br/shop/{id}"),
        commission: Some(Percentage::new(Decimal::from(5)).unwrap()),
        rating: Some(Decimal::from_str("4.8").unwrap()),
    }
}

fn fone(price: &str) -> FoundOffer {
    FoundOffer {
        id: "22334455667".into(),
        title: "Fone de Ouvido Bluetooth TWS".into(),
        link: "https://shopee.com.br/product/339216014/22334455667".into(),
        shop: shop("339216014", "Loja Fones"),
        price: brl(price),
        highest_price: brl(price),
        sales: 12_400,
        commission: Some(Percentage::new(Decimal::from(10)).unwrap()),
        rating: Some(Decimal::from_str("4.9").unwrap()),
    }
}

fn luminaria() -> FoundOffer {
    FoundOffer {
        id: "11223344556".into(),
        title: "Luminária LED de Mesa".into(),
        link: "https://shopee.com.br/product/770011223/11223344556".into(),
        shop: shop("770011223", "Casa Clara"),
        price: brl("45.50"),
        highest_price: brl("59.90"),
        sales: 830,
        commission: None,
        rating: None,
    }
}

/// A Platform's catalog answering from memory, remembering what it was
/// asked.
struct FakeCatalog {
    offers: Mutex<Result<Vec<FoundOffer>, PlatformError>>,
    searches: Mutex<Vec<OfferSearch>>,
}

impl FakeCatalog {
    fn answering(offers: Vec<FoundOffer>) -> Self {
        Self {
            offers: Mutex::new(Ok(offers)),
            searches: Mutex::new(Vec::new()),
        }
    }

    fn failing(error: PlatformError) -> Self {
        Self {
            offers: Mutex::new(Err(error)),
            searches: Mutex::new(Vec::new()),
        }
    }
}

impl OfferSource for FakeCatalog {
    fn source(&self) -> ProductSource {
        SHOPEE
    }

    fn search_offers(&self, search: &OfferSearch) -> Result<Found<FoundOffer>, PlatformError> {
        self.searches.lock().unwrap().push(search.clone());
        Ok(Found {
            items: self.offers.lock().unwrap().clone()?,
            more: true,
        })
    }

    fn search_shops(&self, _: &str, _: u32) -> Result<Found<FoundShop>, PlatformError> {
        Ok(Found {
            items: Vec::new(),
            more: false,
        })
    }
}

fn search(keyword: &str) -> OfferSearch {
    OfferSearch {
        keyword: keyword.into(),
        page: 1,
        ..OfferSearch::default()
    }
}

#[test]
fn a_search_shows_what_the_platform_found_and_nothing_kept_yet() {
    block_on(async {
        let f = Fixture::new().await;
        let shopee = FakeCatalog::answering(vec![fone("29.90"), luminaria()]);

        let found = f
            .catalog
            .search_offers(&shopee, &search("fone"))
            .await
            .unwrap();

        assert!(found.more);
        assert_eq!(found.items.len(), 2);
        assert_eq!(found.items[0].found, fone("29.90"));
        assert!(found.items.iter().all(|item| item.kept.is_none()));
        assert!(!found.items[0].up_to_date());
        assert_eq!(shopee.searches.lock().unwrap()[0], search("fone"));
        assert!(f.catalog.offers().await.unwrap().is_empty());
    });
}

#[test]
fn kept_offers_become_supplier_offers_of_their_shops_with_no_shipping() {
    block_on(async {
        let f = Fixture::new().await;
        let shopee = FakeCatalog::answering(vec![fone("29.90"), luminaria()]);

        let kept = f
            .catalog
            .keep_found_offers(SHOPEE, &[fone("29.90"), luminaria()])
            .await
            .unwrap();

        assert_eq!(
            kept,
            KeptOffers {
                added: 2,
                unchanged: 0,
                skipped: 0
            }
        );
        let suppliers: Vec<String> = f
            .catalog
            .suppliers()
            .await
            .unwrap()
            .into_iter()
            .map(|supplier| supplier.name)
            .collect();
        assert_eq!(suppliers, ["Casa Clara", "Loja Fones"]);
        let found = f
            .catalog
            .search_offers(&shopee, &search("fone"))
            .await
            .unwrap();
        let offer = found.items[0].kept.clone().expect("the fone is kept");
        assert_eq!(offer.source, SHOPEE);
        assert_eq!(offer.supplier.name, "Loja Fones");
        assert_eq!(offer.title, "Fone de Ouvido Bluetooth TWS");
        assert_eq!(offer.link, fone("29.90").link);
        assert_eq!(offer.price, brl("29.90"));
        assert_eq!(offer.shipping, Money::zero(Currency::Brl));
        assert_eq!(offer.observed_at, f.clock.now());
        assert_eq!(found.items[1].kept.as_ref().unwrap().price, brl("45.50"));
        assert!(found.items.iter().all(|item| item.up_to_date()));
        let cheaper = f
            .catalog
            .with_kept(SHOPEE, vec![fone("27.50")])
            .await
            .unwrap();
        assert_eq!(cheaper[0].kept.as_ref().unwrap().price, brl("29.90"));
        assert!(!cheaper[0].up_to_date());
    });
}

#[test]
fn keeping_the_same_offers_again_adds_nothing() {
    block_on(async {
        let f = Fixture::new().await;
        f.catalog
            .keep_found_offers(SHOPEE, &[fone("29.90"), luminaria()])
            .await
            .unwrap();
        f.a_day_later();

        let again = f
            .catalog
            .keep_found_offers(SHOPEE, &[fone("29.9"), luminaria(), fone("29.90")])
            .await
            .unwrap();

        assert_eq!(
            again,
            KeptOffers {
                added: 0,
                unchanged: 3,
                skipped: 0
            }
        );
        let histories = f.catalog.offers().await.unwrap();
        assert_eq!(histories.len(), 2);
        assert!(histories.iter().all(|history| history.earlier.is_empty()));
        assert_eq!(f.catalog.suppliers().await.unwrap().len(), 2);
    });
}

#[test]
fn a_new_price_joins_the_item_history_and_its_product() {
    block_on(async {
        let f = Fixture::new().await;
        f.catalog
            .keep_found_offers(SHOPEE, &[fone("29.90")])
            .await
            .unwrap();
        let first = f.catalog.offers().await.unwrap()[0].latest.clone();
        let product = f
            .catalog
            .create_product(first.id, "Fone TWS", "FONE-TWS-1")
            .await
            .unwrap();
        f.a_day_later();
        // The Platform changed the item's page address; its id is the same.
        let moved = FoundOffer {
            link: "https://shopee.com.br/Fone-Bluetooth-TWS-i.339216014.22334455667".into(),
            ..fone("27.50")
        };

        let kept = f.catalog.keep_found_offers(SHOPEE, &[moved]).await.unwrap();

        assert_eq!(kept.added, 1);
        let histories = f.catalog.offers().await.unwrap();
        assert_eq!(histories.len(), 1);
        assert_eq!(histories[0].latest.price, brl("27.50"));
        assert_eq!(histories[0].latest.product, Some(product.id));
        assert_eq!(histories[0].earlier[0].price, brl("29.90"));
        assert_eq!(
            f.catalog.cheapest_offer(product.id).await.unwrap(),
            Some(brl("27.50"))
        );
    });
}

#[test]
fn a_link_pasted_by_hand_shares_the_history_of_the_kept_item() {
    block_on(async {
        let f = Fixture::new().await;
        f.catalog
            .keep_found_offers(SHOPEE, &[fone("29.90")])
            .await
            .unwrap();
        let supplier = f.catalog.suppliers().await.unwrap().remove(0);
        f.a_day_later();

        f.catalog
            .register_offer(new_offer(
                &supplier,
                "https://shopee.com.br/product/339216014/22334455667?sp_atk=abc",
                "Fone TWS",
                "31",
            ))
            .await
            .unwrap();

        let histories = f.catalog.offers().await.unwrap();
        assert_eq!(histories.len(), 1);
        assert_eq!(histories[0].latest.source, ProductSource::Manual);
        assert_eq!(histories[0].earlier[0].source, SHOPEE);
    });
}

#[test]
fn a_shop_is_one_supplier_by_its_id_and_follows_its_new_name() {
    block_on(async {
        let f = Fixture::new().await;
        f.catalog
            .keep_found_offers(SHOPEE, &[fone("29.90")])
            .await
            .unwrap();
        let renamed = FoundOffer {
            shop: shop("339216014", "Fones & Cia"),
            ..fone("28")
        };

        f.catalog
            .keep_found_offers(SHOPEE, &[renamed])
            .await
            .unwrap();

        let suppliers = f.catalog.suppliers().await.unwrap();
        assert_eq!(suppliers.len(), 1);
        assert_eq!(suppliers[0].name, "Fones & Cia");
        let history = &f.catalog.offers().await.unwrap()[0];
        assert!(
            history
                .offers()
                .all(|offer| offer.supplier.id == suppliers[0].id)
        );
    });
}

#[test]
fn a_supplier_registered_by_hand_with_the_shop_name_is_the_shop() {
    block_on(async {
        let f = Fixture::new().await;
        let by_hand = f.supplier("loja fones").await;

        f.catalog
            .keep_found_offers(SHOPEE, &[fone("29.90")])
            .await
            .unwrap();
        let another_shop = FoundOffer {
            id: "99887766554".into(),
            link: "https://shopee.com.br/product/111/99887766554".into(),
            shop: shop("111", "Loja Fones"),
            ..fone("19.90")
        };
        f.catalog
            .keep_found_offers(SHOPEE, &[another_shop])
            .await
            .unwrap();

        let suppliers = f.catalog.suppliers().await.unwrap();
        assert_eq!(suppliers.len(), 2);
        assert_eq!(suppliers[0], by_hand);
        assert_eq!(suppliers[1].name, "Loja Fones (111)");
    });
}

#[test]
fn offers_without_a_title_a_valid_link_or_a_price_are_skipped() {
    block_on(async {
        let f = Fixture::new().await;
        let untitled = FoundOffer {
            id: "1".into(),
            title: "  ".into(),
            ..fone("10")
        };
        let unlinked = FoundOffer {
            id: "2".into(),
            link: "shopee.com.br/product/1/2".into(),
            ..fone("10")
        };
        let negative = FoundOffer {
            id: "3".into(),
            ..fone("-1")
        };

        let kept = f
            .catalog
            .keep_found_offers(SHOPEE, &[untitled, unlinked, negative, luminaria()])
            .await
            .unwrap();

        assert_eq!(
            kept,
            KeptOffers {
                added: 1,
                unchanged: 0,
                skipped: 3
            }
        );
        assert_eq!(f.catalog.offers().await.unwrap().len(), 1);
    });
}

#[test]
fn a_failed_search_says_why_and_keeps_nothing() {
    block_on(async {
        let f = Fixture::new().await;
        for error in [PlatformError::NotConnected, PlatformError::RateLimited] {
            let shopee = FakeCatalog::failing(error.clone());

            let failed = f.catalog.search_offers(&shopee, &search("fone")).await;

            assert!(matches!(failed, Err(CatalogError::Platform(e)) if e == error));
        }
        assert!(f.catalog.offers().await.unwrap().is_empty());
    });
}

/// A Sales Channel where the fone sells for R$ 100 with a R$ 12 fee.
struct OneMatch;

impl DemandSource for OneMatch {
    fn currency(&self) -> Currency {
        Currency::Brl
    }
    fn categories(&self) -> Result<Vec<DemandCategory>, PlatformError> {
        Ok(Vec::new())
    }
    fn best_sellers(&self, _: &str) -> Result<Vec<BestSeller>, PlatformError> {
        Ok(Vec::new())
    }
    fn search(&self, query: &str) -> Result<Vec<CatalogMatch>, PlatformError> {
        Ok(if query.contains("Fone") {
            vec![CatalogMatch {
                id: "MLB1".into(),
                name: "Fone Bluetooth TWS".into(),
            }]
        } else {
            Vec::new()
        })
    }
    fn competition(&self, _: &str) -> Result<Competition, PlatformError> {
        Ok(Competition {
            sellers: 3,
            prices: vec![brl("100")],
            category: Some("MLB3697".into()),
        })
    }
    fn root_category(&self, _: &str) -> Result<DemandCategory, PlatformError> {
        Ok(DemandCategory {
            id: "MLB1000".into(),
            name: "Eletrônicos".into(),
        })
    }
    fn sale_fee(&self, _: &str, _: Money, _: ListingType) -> Result<Money, PlatformError> {
        Ok(brl("12"))
    }
}

#[test]
fn a_kept_offer_enters_the_opportunities_on_the_next_demand_sync() {
    block_on(async {
        let f = Fixture::new().await;
        f.catalog
            .keep_found_offers(SHOPEE, &[fone("29.90"), luminaria()])
            .await
            .unwrap();

        let synced = f.catalog.sync_demand(&OneMatch).await.unwrap();

        assert_eq!(synced.offers, 2);
        assert_eq!(synced.opportunities, 1);
        let opportunities = f
            .catalog
            .opportunities(&OpportunityFilter::default(), Percentage::ZERO)
            .await
            .unwrap();
        assert_eq!(opportunities.len(), 1);
        assert_eq!(opportunities[0].offer.source, SHOPEE);
        assert_eq!(opportunities[0].cost, brl("29.90"));
    });
}
