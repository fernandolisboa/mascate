//! Mascate Categories (#237): the owner's own categories grouping channel
//! categories at any level, with optional keywords, and the best sellers
//! each one shows.

mod common;

use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_catalog::{
    BestSeller, Catalog, CatalogError, CatalogMatch, CategoryNode, Competition, DemandCategory,
    DemandSource, MIGRATIONS, MascateCategory, MascateCategoryDraft, RankedBestSeller,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, ListingType, Money, PlatformError, RecordId};
use mascate_platform::{Database, ModuleMigrations, migrate};

use common::{Fixture, brl};

fn category(id: &str, name: &str) -> DemandCategory {
    DemandCategory {
        id: id.into(),
        name: name.into(),
    }
}

fn grains() -> DemandCategory {
    category("MLB1040", "Café")
}

fn coffee_makers() -> DemandCategory {
    category("MLB9234", "Cafeteiras")
}

fn small_appliances() -> DemandCategory {
    category("MLB5726", "Eletroportáteis")
}

fn draft(name: &str, channel: &[DemandCategory], keywords: &[&str]) -> MascateCategoryDraft {
    MascateCategoryDraft {
        name: name.into(),
        channel_categories: channel.to_vec(),
        keywords: keywords.iter().map(|keyword| (*keyword).into()).collect(),
    }
}

async fn create(f: &Fixture, draft: MascateCategoryDraft) -> MascateCategory {
    f.catalog.create_mascate_category(draft).await.unwrap()
}

fn best_seller(id: &str, position: u32, title: &str) -> BestSeller {
    BestSeller {
        id: id.into(),
        position,
        title: title.into(),
        catalog_product: Some(id.into()),
        price: Some(brl("49.90")),
    }
}

/// A Sales Channel whose rankings the test sets, counting what it was asked.
#[derive(Default)]
struct Rankings {
    ranked: Mutex<Vec<(String, Vec<BestSeller>)>>,
    calls: Mutex<Vec<String>>,
}

impl Rankings {
    fn rank(&self, category: &DemandCategory, best_sellers: Vec<BestSeller>) {
        self.ranked
            .lock()
            .unwrap()
            .push((category.id.clone(), best_sellers));
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl DemandSource for Rankings {
    fn currency(&self) -> Currency {
        Currency::Brl
    }
    fn categories(&self) -> Result<Vec<DemandCategory>, PlatformError> {
        Ok(Vec::new())
    }
    fn category(&self, _: &str) -> Result<CategoryNode, PlatformError> {
        Err(PlatformError::NotFound)
    }
    fn find_categories(&self, _: &str) -> Result<Vec<CategoryNode>, PlatformError> {
        Ok(Vec::new())
    }
    fn best_sellers(&self, category: &str) -> Result<Vec<BestSeller>, PlatformError> {
        self.calls.lock().unwrap().push(category.into());
        Ok(self
            .ranked
            .lock()
            .unwrap()
            .iter()
            .find(|(id, _)| id == category)
            .map(|(_, ranked)| ranked.clone())
            .unwrap_or_default())
    }
    fn search(&self, _: &str) -> Result<Vec<CatalogMatch>, PlatformError> {
        Ok(Vec::new())
    }
    fn competition(&self, _: &str) -> Result<Competition, PlatformError> {
        Err(PlatformError::NotFound)
    }
    fn root_category(&self, _: &str) -> Result<DemandCategory, PlatformError> {
        Err(PlatformError::NotFound)
    }
    fn sale_fee(&self, _: &str, _: Money, _: ListingType) -> Result<Money, PlatformError> {
        Err(PlatformError::NotFound)
    }
}

/// The ids of the best sellers `category` shows, best place first, each
/// with that place.
fn shown(ranked: &[RankedBestSeller], category: RecordId) -> Vec<(String, u32)> {
    let mut shown: Vec<(String, u32)> = ranked
        .iter()
        .filter_map(|r| Some((r.best_seller.id.clone(), r.position_in(category)?)))
        .collect();
    shown.sort_by_key(|(_, position)| *position);
    shown
}

#[test]
fn a_category_of_the_owners_joins_the_best_sellers_of_several_channel_categories() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = Rankings::default();
        let coffee = create(&f, draft("Café", &[grains(), coffee_makers()], &[])).await;
        channel.rank(
            &grains(),
            vec![
                best_seller("MLB1", 1, "Café em Grãos 1kg"),
                best_seller("MLB2", 2, "Café Moído Tradicional 500g"),
            ],
        );
        channel.rank(
            &coffee_makers(),
            vec![
                best_seller("MLB3", 1, "Cafeteira Elétrica 30 Xícaras"),
                best_seller("MLB4", 3, "Cafeteira Italiana Moka"),
            ],
        );

        let report = f.catalog.sync_demand(&channel).await.unwrap();
        let ranked = f.catalog.best_sellers().await.unwrap();

        assert_eq!(report.categories, 1);
        assert_eq!(report.channel_categories, 2);
        assert_eq!(report.best_sellers, 4);
        assert_eq!(
            shown(&ranked, coffee.id),
            [
                ("MLB1".to_owned(), 1),
                ("MLB3".to_owned(), 1),
                ("MLB2".to_owned(), 2),
                ("MLB4".to_owned(), 3),
            ]
        );
        let maker = ranked.iter().find(|r| r.best_seller.id == "MLB3").unwrap();
        assert_eq!(maker.rankings[0].category, coffee_makers());
    });
}

#[test]
fn a_channel_category_two_of_them_share_is_read_once_and_each_product_shows_once() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = Rankings::default();
        let coffee = create(&f, draft("Café", &[grains(), coffee_makers()], &[])).await;
        let kitchen = create(
            &f,
            draft("Cozinha", &[coffee_makers(), small_appliances()], &[]),
        )
        .await;
        channel.rank(
            &coffee_makers(),
            vec![best_seller("MLB3", 2, "Cafeteira Elétrica")],
        );
        channel.rank(
            &small_appliances(),
            vec![best_seller("MLB3", 5, "Cafeteira Elétrica")],
        );

        let report = f.catalog.sync_demand(&channel).await.unwrap();
        let ranked = f.catalog.best_sellers().await.unwrap();

        assert_eq!(report.categories, 2);
        assert_eq!(report.channel_categories, 3);
        assert_eq!(report.best_sellers, 1);
        let mut calls = channel.calls();
        calls.sort();
        assert_eq!(calls, ["MLB1040", "MLB5726", "MLB9234"]);
        let [only] = &ranked[..] else {
            panic!("one product: {ranked:?}");
        };
        assert_eq!(only.position_in(coffee.id), Some(2));
        assert_eq!(only.position_in(kitchen.id), Some(2));
        assert_eq!(only.rankings.len(), 2);
    });
}

#[test]
fn keywords_keep_only_titles_holding_one_of_them_as_whole_words() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = Rankings::default();
        let manual = create(
            &f,
            draft(
                "Café coado",
                &[coffee_makers()],
                &["coador", "MOEDOR manual"],
            ),
        )
        .await;
        let everything = create(&f, draft("Tudo de café", &[coffee_makers()], &[])).await;
        channel.rank(
            &coffee_makers(),
            vec![
                best_seller("MLB1", 1, "Cafeteira Elétrica Inox"),
                best_seller("MLB2", 2, "Coador de Café de Pano"),
                best_seller("MLB3", 3, "Moedor Manual de Café Cerâmico"),
                best_seller("MLB4", 4, "Moedor Elétrico, manual de uso incluso"),
                best_seller("MLB5", 5, "Suporte para Coadores"),
            ],
        );

        f.catalog.sync_demand(&channel).await.unwrap();
        let ranked = f.catalog.best_sellers().await.unwrap();

        assert_eq!(
            shown(&ranked, manual.id),
            [("MLB2".to_owned(), 2), ("MLB3".to_owned(), 3)]
        );
        assert_eq!(shown(&ranked, everything.id).len(), 5);
        assert!(manual.takes("COADOR de café"));
        assert!(!manual.takes("Moedor elétrico manual"));
    });
}

#[test]
fn a_category_needs_a_free_name_and_a_channel_category() {
    block_on(async {
        let f = Fixture::new().await;
        let coffee = create(&f, draft("  Café  ", &[grains()], &[])).await;

        assert_eq!(coffee.name, "Café");
        assert!(matches!(
            f.catalog
                .create_mascate_category(draft("   ", &[grains()], &[]))
                .await,
            Err(CatalogError::MissingCategoryName)
        ));
        assert!(matches!(
            f.catalog
                .create_mascate_category(draft("CAFÉ", &[coffee_makers()], &[]))
                .await,
            Err(CatalogError::CategoryNameTaken(name)) if name == "CAFÉ"
        ));
        assert!(matches!(
            f.catalog
                .create_mascate_category(draft("Pets", &[], &["ração"]))
                .await,
            Err(CatalogError::NoChannelCategory)
        ));
        let pets = create(&f, draft("Pets", &[small_appliances()], &[])).await;
        assert!(matches!(
            f.catalog
                .edit_mascate_category(pets.id, draft("café", &[grains()], &[]))
                .await,
            Err(CatalogError::CategoryNameTaken(_))
        ));
        f.catalog.remove_mascate_category(pets.id).await.unwrap();
        assert!(matches!(
            f.catalog.remove_mascate_category(pets.id).await,
            Err(CatalogError::UnknownMascateCategory(id)) if id == pets.id
        ));
        let again = create(&f, draft("Pets", &[small_appliances()], &[])).await;
        assert_ne!(again.id, pets.id);
    });
}

#[test]
fn editing_changes_the_name_channel_categories_and_keywords_and_what_the_sync_reads() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = Rankings::default();
        let coffee = create(
            &f,
            draft(
                "Café",
                &[grains(), coffee_makers(), grains()],
                &["moedor", " Moedor ", "?!", "coado"],
            ),
        )
        .await;
        assert_eq!(coffee.channel_categories, [grains(), coffee_makers()]);
        assert_eq!(coffee.keywords, ["moedor", "coado"]);

        let edited = f
            .catalog
            .edit_mascate_category(
                coffee.id,
                draft(
                    "café",
                    &[
                        category("MLB9234", "Cafeteiras e Moedores"),
                        small_appliances(),
                    ],
                    &[],
                ),
            )
            .await
            .unwrap();
        f.catalog.sync_demand(&channel).await.unwrap();

        assert_eq!(edited.id, coffee.id);
        assert_eq!(edited.name, "café");
        assert_eq!(
            edited.channel_categories,
            [
                category("MLB9234", "Cafeteiras e Moedores"),
                small_appliances()
            ]
        );
        assert!(edited.keywords.is_empty());
        assert_eq!(f.catalog.mascate_categories().await.unwrap(), [edited]);
        let mut calls = channel.calls();
        calls.sort();
        assert_eq!(calls, ["MLB5726", "MLB9234"]);
    });
}

#[test]
fn each_followed_category_becomes_a_category_of_the_owners_with_the_same_name() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 11, 1, 12, 0, 0).unwrap(),
        ));
        let database = Arc::new(Database::open(&dir.path().join("m.db")).await.unwrap());
        let before_237 = ModuleMigrations {
            module: MIGRATIONS.module,
            migrations: &MIGRATIONS.migrations[..3],
        };
        migrate(&database, clock.as_ref(), &[before_237])
            .await
            .unwrap();
        database
            .connection()
            .execute_batch(
                "INSERT INTO catalog_demand_categories
                     (id, ml_category_id, name, created_at, updated_at, deleted_at)
                 VALUES
                     ('0190a000-0000-7000-8000-000000000001', 'MLB1276', 'Esportes e Fitness',
                      '2026-11-08T13:00:00Z', '2026-11-08T13:00:00Z', NULL),
                     ('0190a000-0000-7000-8000-000000000002', 'MLB264586', 'Saúde',
                      '2026-11-08T13:00:00Z', '2026-11-08T13:00:00Z', NULL),
                     ('0190a000-0000-7000-8000-000000000003', 'MLB1000', 'Eletrônicos',
                      '2026-11-08T13:00:00Z', '2026-11-08T13:00:00Z',
                      '2026-11-08T13:30:00Z');",
            )
            .await
            .unwrap();

        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        let catalog = Catalog::new(
            database,
            dir.path().join("produtos"),
            clock,
            Arc::new(SequentialIds::default()),
        );
        let categories = catalog.mascate_categories().await.unwrap();

        let carried: Vec<(String, Vec<DemandCategory>, usize)> = categories
            .into_iter()
            .map(|c| (c.name, c.channel_categories, c.keywords.len()))
            .collect();
        assert_eq!(
            carried,
            [
                (
                    "Esportes e Fitness".to_owned(),
                    vec![category("MLB1276", "Esportes e Fitness")],
                    0
                ),
                ("Saúde".to_owned(), vec![category("MLB264586", "Saúde")], 0),
            ]
        );
    });
}
