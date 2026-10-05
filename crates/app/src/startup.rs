use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::executor::block_on;
use mascate_catalog::Catalog;
use mascate_commerce::{Listings, Orders, Pricing, PurchaseOrders, StockMirror};
use mascate_finance::Taxes;
use mascate_inventory::Inventory;
use mascate_kernel::{SystemClock, UuidV7Generator};
use mascate_marketing::{ListingQuality, Questions, ReplyTemplates, Reputation};
use mascate_platform::{
    Appearance, BackupSettings, Backups, Database, Flag, Flags, Installation, ModuleMigrations,
    OpenError, Opened, Registry, ReleaseChannel, Reminder, Reminders, UpdateSettings, Updater,
    Version, apply_staged_restore, default_backup_folder, default_database_path,
    default_owner_folder, load_appearance, load_update_settings, open_and_migrate,
    undo_applied_restore,
};

/// Every module's migrations, in dependency order. Modules add theirs here.
pub const MODULE_MIGRATIONS: &[ModuleMigrations] = &[
    mascate_platform::MIGRATIONS,
    mascate_catalog::MIGRATIONS,
    mascate_inventory::MIGRATIONS,
    mascate_commerce::MIGRATIONS,
    mascate_marketing::MIGRATIONS,
    mascate_finance::MIGRATIONS,
];

/// Every module's flags, in the order the settings screen lists them.
const MODULE_FLAGS: &[&[Flag]] = &[mascate_marketing::FLAGS];

/// Every module's Reminders, in the order the home screen shows them.
const MODULE_REMINDERS: &[&[Reminder]] = &[mascate_finance::REMINDERS, mascate_commerce::REMINDERS];

/// The ready database and what is read from it before the first window, or
/// why the database is not ready.
pub type Outcome = Result<Started, Problem>;

#[derive(Clone)]
pub struct Started {
    pub database: Arc<Database>,
    pub appearance: Appearance,
    pub flags: Arc<Flags>,
    pub reminders: Arc<Reminders>,
    pub backups: Arc<Backups>,
    pub catalog: Arc<Catalog>,
    pub inventory: Arc<Inventory>,
    pub purchase_orders: Arc<PurchaseOrders>,
    pub listings: Arc<Listings>,
    pub pricing: Arc<Pricing>,
    pub stock_mirror: Arc<StockMirror>,
    pub orders: Arc<Orders>,
    pub quality: Arc<ListingQuality>,
    pub questions: Arc<Questions>,
    pub reputation: Arc<Reputation>,
    pub reply_templates: Arc<ReplyTemplates>,
    pub taxes: Arc<Taxes>,
    pub backup_settings: BackupSettings,
    pub update_settings: UpdateSettings,
    /// What became of a restore staged before the restart, if one was.
    pub restore: Option<Result<(), String>>,
}

/// Why the database is not ready, for the home screen.
#[derive(Debug, Clone)]
pub struct Problem {
    pub message: String,
    /// The database file, when known: a restore can still replace it.
    pub database_path: Option<PathBuf>,
    /// This version could not migrate the database, which is back as it was
    /// (or, if not, its Backup is named in the message).
    pub failed_to_migrate: bool,
}

impl Problem {
    fn new(message: impl Into<String>, database_path: Option<&Path>) -> Self {
        Self {
            message: message.into(),
            database_path: database_path.map(Path::to_path_buf),
            failed_to_migrate: false,
        }
    }
}

/// Every risky migration this app brings, as JSON for the update manifest
/// the release workflow publishes (ADR 0010).
pub fn risky_migrations_json() -> String {
    let listed: Vec<String> = MODULE_MIGRATIONS
        .iter()
        .flat_map(ModuleMigrations::risky)
        .map(|risky| {
            format!(
                r#"{{"module": {:?}, "version": {}}}"#,
                risky.module, risky.version
            )
        })
        .collect();
    format!("[{}]", listed.join(", "))
}

/// This app's version, as its Releases are tagged.
pub fn this_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).expect("the app version is major.minor.patch")
}

/// What finds and installs newer versions of this copy of the app; `None`
/// when the OS reports no data folder to keep downloads in.
pub fn updater() -> Option<Updater> {
    let data = default_database_path()?.parent()?.to_path_buf();
    let installation = Installation::detect();
    Some(Updater::new(
        ReleaseChannel::github(
            &format!("Mascate/{}", env!("CARGO_PKG_VERSION")),
            installation.target(),
        ),
        this_version(),
        installation,
        data.join("updates"),
        MODULE_MIGRATIONS,
    ))
}

/// Swaps in a staged restore, creates the database on first run, brings its
/// schema up to date (a restored file from an older version included) with
/// a Backup first, and reads the saved appearance, before any window opens
/// so the first frame is already in the owner's theme and layout.
pub fn prepare() -> Outcome {
    let path = default_database_path()
        .ok_or_else(|| Problem::new("Não encontrei a pasta de dados do usuário.", None))?;
    let mut restore = match apply_staged_restore(&path) {
        Ok(true) => Some(Ok(())),
        Ok(false) => None,
        // The swap puts the current database back when it fails.
        Err(error) => Some(Err(format!(
            "Não consegui restaurar o Backup escolhido; o banco continua o de antes. \
             Tente restaurar de novo: {error}"
        ))),
    };
    let backup_folder = default_backup_folder()
        .or_else(|| path.parent().map(|data| data.join("backups")))
        .ok_or_else(|| Problem::new("Não encontrei uma pasta para os Backups.", Some(&path)))?;
    let product_files = default_owner_folder("produtos")
        .or_else(|| path.parent().map(|data| data.join("produtos")))
        .ok_or_else(|| {
            Problem::new(
                "Não encontrei uma pasta para os arquivos dos produtos.",
                Some(&path),
            )
        })?;
    block_on(async {
        let opened = match open(&path, &backup_folder).await {
            Err(error) if matches!(restore, Some(Ok(()))) => {
                undo_applied_restore(&path).map_err(|undo| {
                    Problem::new(
                        format!(
                            "O arquivo restaurado não abriu ({error}) e não consegui pôr o \
                             banco de antes de volta: {undo}"
                        ),
                        Some(&path),
                    )
                })?;
                restore = Some(Err(format!(
                    "O arquivo restaurado não abriu nesta versão do app, então o banco de \
                     antes voltou: {error}"
                )));
                open(&path, &backup_folder).await
            }
            opened => opened,
        }
        .map_err(|error| not_opened(error, &path))?;
        let database = opened.database;
        let ready = async {
            let appearance = load_appearance(&database).await.unwrap_or_else(|error| {
                // A look that cannot be read is no reason to give up the database.
                eprintln!("could not read the saved appearance: {error}");
                Appearance::default()
            });
            let flags = Flags::new(
                database.clone(),
                Registry::new(MODULE_FLAGS)?,
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let reminders = Reminders::new(
                database.clone(),
                Registry::new(MODULE_REMINDERS)?,
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let backups = Backups::new(
                database.clone(),
                MODULE_MIGRATIONS,
                backup_folder,
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let catalog = Catalog::new(
                database.clone(),
                product_files,
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let inventory = Arc::new(Inventory::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            ));
            let purchase_orders = PurchaseOrders::new(
                database.clone(),
                inventory.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let listings = Listings::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let pricing = Pricing::new(
                database.clone(),
                inventory.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let stock_mirror = StockMirror::new(
                database.clone(),
                inventory.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let orders = Orders::new(
                database.clone(),
                inventory.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let quality = ListingQuality::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let questions = Questions::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let reputation = Reputation::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let reply_templates = ReplyTemplates::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let taxes = Taxes::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let backup_settings = backups.settings().await?;
            let update_settings = load_update_settings(&database).await?;
            Ok::<_, Box<dyn std::error::Error>>(Started {
                database: database.clone(),
                appearance,
                flags: Arc::new(flags),
                reminders: Arc::new(reminders),
                backups: Arc::new(backups),
                catalog: Arc::new(catalog),
                inventory,
                purchase_orders: Arc::new(purchase_orders),
                listings: Arc::new(listings),
                pricing: Arc::new(pricing),
                stock_mirror: Arc::new(stock_mirror),
                orders: Arc::new(orders),
                quality: Arc::new(quality),
                questions: Arc::new(questions),
                reputation: Arc::new(reputation),
                reply_templates: Arc::new(reply_templates),
                taxes: Arc::new(taxes),
                backup_settings,
                update_settings,
                restore,
            })
        };
        ready.await.map_err(|error| {
            Problem::new(
                format!(
                    "Não consegui preparar o banco em {}: {error}",
                    path.display()
                ),
                Some(&path),
            )
        })
    })
}

async fn open(path: &Path, backup_folder: &Path) -> Result<Opened, OpenError> {
    open_and_migrate(
        path,
        MODULE_MIGRATIONS,
        backup_folder.to_path_buf(),
        Arc::new(SystemClock),
        Arc::new(UuidV7Generator),
    )
    .await
}

fn not_opened(error: OpenError, path: &Path) -> Problem {
    let failed_to_migrate = matches!(
        error,
        OpenError::PutBack { .. } | OpenError::NotPutBack { .. }
    );
    let message = match error {
        OpenError::PutBack { error, backup } => format!(
            "Esta versão não conseguiu migrar o banco ({error}). O banco voltou exatamente \
             como estava, a partir do Backup {}.",
            backup.path.display()
        ),
        OpenError::NotPutBack {
            error,
            backup,
            put_back,
        } => format!(
            "Esta versão não conseguiu migrar o banco ({error}) nem pôr de volta o Backup \
             feito antes ({put_back}). Restaure {} em Configurações › Backup.",
            backup.path.display()
        ),
        OpenError::NoBackupBeforeMigrating(error) => format!(
            "Não consegui fazer o Backup antes de migrar o banco, então não mexi nele. \
             Confira a pasta de Backups e abra o app de novo: {error}"
        ),
        error => format!(
            "Não consegui preparar o banco em {}: {error}",
            path.display()
        ),
    };
    Problem {
        message,
        database_path: Some(path.to_path_buf()),
        failed_to_migrate,
    }
}

#[cfg(test)]
mod tests {
    use mascate_catalog::{DemandCategory, DiscoverySettings, NewSupplierOffer, OpportunityFilter};
    use mascate_commerce::{
        ChannelListing, ChannelStock, ListingStatus, NewPurchaseLine, NewPurchaseOrder,
        PurchaseOrderStatus, Receiving, SaleFee, SalesChannel,
    };
    use mascate_inventory::{HOME_LOCATION, LowStock, StockAdjustment};
    use mascate_kernel::{Currency, ListingType, Money, Percentage, PlatformError, RecordId};
    use mascate_marketing::{
        ChannelQuality, ChannelQuestion, ChannelQuestions, ChannelReputation, ChannelReview,
        ListedItem, ListingReviews, MetricReading, NewReplyTemplate, QualityLevel, QualitySource,
        QuestionStatus, Rating, ReputationColor, ReputationSource,
    };
    use mascate_platform::{LayoutId, UiTheme, UiThemePreference, save_appearance};

    use super::*;

    const SAMPLE_SKU: &str = "EXEMPLO-001";
    const SAMPLE_ORDERED: u32 = 3;
    const SAMPLE_RECEIVED: u32 = 2;
    const SAMPLE_LOST: u32 = 1;
    const SAMPLE_REORDER_POINT: u32 = 5;
    const SAMPLE_TAX_PERCENT: u32 = 6;

    fn sample_discovery_settings() -> DiscoverySettings {
        DiscoverySettings {
            listing_type: ListingType::Premium,
            estimated_fee: Percentage::new(17.into()).unwrap(),
            estimated_shipping: Money::new(25.into(), Currency::Brl),
        }
    }

    fn sample_listing() -> ChannelListing {
        ChannelListing {
            id: "MLB4100000001".into(),
            variation: None,
            title: "Produto de exemplo".into(),
            price: Money::new(59.into(), Currency::Brl),
            available_quantity: 1,
            status: ListingStatus::Paused,
            link: Some("https://produto.mercadolivre.com.br/MLB-4100000001-exemplo-_JM".into()),
            listing_type: Some(ListingType::Classic),
            category: Some("MLB1000".into()),
            seller_sku: Some(SAMPLE_SKU.into()),
        }
    }

    /// A Sales Channel with the sample listing only.
    struct SampleChannel;

    impl SalesChannel for SampleChannel {
        fn listing_ids(&self) -> Result<Vec<String>, PlatformError> {
            Ok(vec![sample_listing().id])
        }

        fn listings(&self, _: &[String]) -> Result<Vec<ChannelListing>, PlatformError> {
            Ok(vec![sample_listing()])
        }

        fn sale_fee(&self, _: &str, _: Money, _: ListingType) -> Result<SaleFee, PlatformError> {
            Err(PlatformError::NotFound)
        }

        fn set_price(&self, _: &str, _: Money) -> Result<(), PlatformError> {
            Err(PlatformError::NotFound)
        }

        fn set_stock(&self, _: &str, _: &[ChannelStock]) -> Result<(), PlatformError> {
            Err(PlatformError::NotFound)
        }

        fn pause(&self, _: &str) -> Result<(), PlatformError> {
            Err(PlatformError::NotFound)
        }

        fn activate(&self, _: &str) -> Result<(), PlatformError> {
            Err(PlatformError::NotFound)
        }
    }

    fn sample_quality() -> ChannelQuality {
        ChannelQuality {
            score: 48,
            level: QualityLevel::Basic,
            pending: Vec::new(),
        }
    }

    /// Rates every listing like the sample.
    struct SampleQuality;

    impl QualitySource for SampleQuality {
        fn quality(&self, _: &str) -> Result<Option<ChannelQuality>, PlatformError> {
            Ok(Some(sample_quality()))
        }

        fn visits(&self, _: &str, _: u32) -> Result<u32, PlatformError> {
            Ok(7)
        }
    }

    fn sample_question() -> ChannelQuestion {
        ChannelQuestion {
            id: "13001000001".into(),
            listing: sample_listing().id,
            text: "Funciona com iPhone?".into(),
            status: QuestionStatus::Unanswered,
            asked_at: chrono::DateTime::parse_from_rfc3339("2026-10-04T08:15:02-03:00")
                .unwrap()
                .with_timezone(&chrono::Utc),
            answer: None,
        }
    }

    /// One buyer waiting for an answer.
    struct SampleQuestions;

    impl ChannelQuestions for SampleQuestions {
        fn unanswered(&self) -> Result<Vec<ChannelQuestion>, PlatformError> {
            Ok(vec![sample_question()])
        }

        fn question(&self, _: &str) -> Result<Option<ChannelQuestion>, PlatformError> {
            Ok(Some(sample_question()))
        }

        fn answer(&self, _: &str, _: &str) -> Result<(), PlatformError> {
            Ok(())
        }
    }

    fn sample_reputation() -> ChannelReputation {
        ChannelReputation {
            color: Some(ReputationColor::Yellow),
            real_color: None,
            protected_until: None,
            period_days: Some(60),
            sales: 12,
            transactions: 12,
            claims: MetricReading::NONE,
            cancellations: MetricReading::NONE,
            delayed_handling: MetricReading::NONE,
        }
    }

    /// A yellow seller with one low Review on every listing.
    struct SampleReputation;

    impl ReputationSource for SampleReputation {
        fn reputation(&self) -> Result<ChannelReputation, PlatformError> {
            Ok(sample_reputation())
        }

        fn reviews(&self, _: &str) -> Result<ListingReviews, PlatformError> {
            Ok(ListingReviews {
                stars: [0, 1, 0, 0, 2],
                low: vec![ChannelReview {
                    id: "52001000002".into(),
                    rating: 2,
                    title: "Parou de carregar".into(),
                    text: String::new(),
                    at: sample_question().asked_at,
                }],
            })
        }
    }

    fn sample_template() -> NewReplyTemplate {
        NewReplyTemplate {
            name: "Prazo de envio".into(),
            text: "O {produto} sai em até {prazo}.".into(),
        }
    }

    fn sample_category() -> DemandCategory {
        DemandCategory {
            id: "MLB1000".into(),
            name: "Eletrônicos, Áudio e Vídeo".into(),
        }
    }

    /// One database per published version, written by the version itself
    /// with [`write_this_versions_sample_database`].
    fn released_databases() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/released-databases")
    }

    /// Run when tagging a release: `cargo test -p mascate -- --ignored
    /// write_this_versions_sample_database`, then commit the file.
    #[test]
    #[ignore = "writes the sample database of a release"]
    fn write_this_versions_sample_database() {
        let path = released_databases().join(format!("{}.db", env!("CARGO_PKG_VERSION")));
        let _ = std::fs::remove_file(&path);
        block_on(async {
            let database = open(&path, &std::env::temp_dir()).await.unwrap().database;
            let clock = Arc::new(SystemClock);
            let ids = Arc::new(UuidV7Generator);
            save_appearance(
                &database,
                clock.as_ref(),
                ids.as_ref(),
                Appearance {
                    theme: UiThemePreference::Fixed(UiTheme::BlackGold),
                    layout: LayoutId::Studio,
                },
            )
            .await
            .unwrap();
            let flags = Flags::new(
                database.clone(),
                Registry::new(MODULE_FLAGS).unwrap(),
                clock.clone(),
                ids.clone(),
            );
            let key = MODULE_FLAGS[0][0].key;
            flags
                .turn_on(flags.request_turn_on(key).unwrap().confirm("sample"))
                .await
                .unwrap();
            let reminders = Reminders::new(
                database.clone(),
                Registry::new(MODULE_REMINDERS).unwrap(),
                clock.clone(),
                ids.clone(),
            );
            reminders.dismiss(MODULE_REMINDERS[0][0].key).await.unwrap();
            let catalog = Catalog::new(
                database.clone(),
                std::env::temp_dir().join("mascate-sample-products"),
                clock.clone(),
                ids.clone(),
            );
            let supplier = catalog.add_supplier("Loja de exemplo").await.unwrap();
            let offer = catalog
                .register_offer(NewSupplierOffer {
                    supplier: supplier.id,
                    link: "https://shopee.com.br/exemplo-i.1.2".into(),
                    title: "Produto de exemplo".into(),
                    price: Money::new(29.into(), Currency::Brl),
                    shipping: Money::zero(Currency::Brl),
                })
                .await
                .unwrap();
            let product = catalog
                .create_product(offer.id, "Produto de exemplo", SAMPLE_SKU)
                .await
                .unwrap();
            let inventory = Arc::new(Inventory::new(database.clone(), clock.clone(), ids.clone()));
            let orders = PurchaseOrders::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            );
            let order = orders
                .create(NewPurchaseOrder {
                    supplier: supplier.id,
                    ordered_on: chrono::NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
                    freight: Money::new(10.into(), Currency::Brl),
                    lines: vec![NewPurchaseLine {
                        product: product.id,
                        quantity: SAMPLE_ORDERED,
                        unit_price: Money::new(29.into(), Currency::Brl),
                    }],
                })
                .await
                .unwrap();
            orders
                .receive(
                    order.id,
                    HOME_LOCATION,
                    &[Receiving {
                        line: order.lines[0].id,
                        quantity: SAMPLE_RECEIVED,
                    }],
                )
                .await
                .unwrap();
            inventory
                .adjust(
                    product.id,
                    HOME_LOCATION,
                    StockAdjustment::Loss(SAMPLE_LOST),
                    Some("amostra"),
                )
                .await
                .unwrap();
            inventory
                .set_reorder_point(product.id, Some(SAMPLE_REORDER_POINT))
                .await
                .unwrap();
            catalog
                .save_discovery_settings(sample_discovery_settings())
                .await
                .unwrap();
            catalog
                .follow_categories(&[sample_category()])
                .await
                .unwrap();
            Taxes::new(database.clone(), clock.clone(), ids.clone())
                .save_rate(Percentage::new(SAMPLE_TAX_PERCENT.into()).unwrap())
                .await
                .unwrap();
            let listings = Listings::new(database.clone(), clock.clone(), ids.clone());
            listings.sync(&SampleChannel).await.unwrap();
            let listing = listings.listings().await.unwrap().remove(0);
            listings.link(listing.id, product.id).await.unwrap();
            ListingQuality::new(database.clone(), clock.clone(), ids.clone())
                .sync(&SampleQuality, &[sample_listing().id])
                .await
                .unwrap();
            Questions::new(database.clone(), clock.clone(), ids.clone())
                .sync(&SampleQuestions)
                .await
                .unwrap();
            ReplyTemplates::new(database.clone(), clock.clone(), ids.clone())
                .create(sample_template())
                .await
                .unwrap();
            Reputation::new(database.clone(), clock.clone(), ids.clone())
                .sync(&SampleReputation, &[sample_listing().id])
                .await
                .unwrap();
            let backups = Backups::new(
                database.clone(),
                MODULE_MIGRATIONS,
                PathBuf::from("/backups"),
                clock,
                ids,
            );
            backups
                .save_settings(&BackupSettings {
                    folder: PathBuf::from(if cfg!(windows) {
                        r"C:\Users\owner\Documents\Mascate\Backups"
                    } else {
                        "/home/owner/Documents/Mascate/Backups"
                    }),
                    keep: 7,
                })
                .await
                .unwrap();
            database
                .connection()
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .await
                .unwrap();
        });
    }

    /// Every published version's database migrates to this one, after a
    /// Backup, and keeps what the owner saved.
    #[test]
    fn every_released_database_migrates_to_this_version() {
        let mut samples: Vec<PathBuf> = std::fs::read_dir(released_databases())
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "db"))
            .collect();
        samples.sort();
        assert!(!samples.is_empty());
        for sample in samples {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("mascate.db");
            std::fs::copy(&sample, &path).unwrap();
            let backups_folder = dir.path().join("backups");
            block_on(async {
                // The sample's own Backup folder belongs to the machine that made it.
                {
                    let database = Arc::new(Database::open(&path).await.unwrap());
                    Backups::new(
                        database,
                        MODULE_MIGRATIONS,
                        backups_folder.clone(),
                        Arc::new(SystemClock),
                        Arc::new(UuidV7Generator),
                    )
                    .save_settings(&BackupSettings {
                        folder: backups_folder.clone(),
                        keep: 7,
                    })
                    .await
                    .unwrap();
                }

                let opened = open(&path, &backups_folder).await.unwrap();

                let name = sample.display();
                let released = sample.file_stem().unwrap().to_str().unwrap();
                if Version::parse(released) < Some(this_version()) {
                    assert!(opened.backup_before_migrating.is_some(), "{name}");
                }
                let database = opened.database;
                assert_eq!(
                    load_appearance(&database).await.unwrap(),
                    Appearance {
                        theme: UiThemePreference::Fixed(UiTheme::BlackGold),
                        layout: LayoutId::Studio,
                    },
                    "{name}"
                );
                let flags = Flags::new(
                    database.clone(),
                    Registry::new(MODULE_FLAGS).unwrap(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                assert!(flags.is_on(&MODULE_FLAGS[0][0]).await.unwrap(), "{name}");
                let reminders = Reminders::new(
                    database.clone(),
                    Registry::new(MODULE_REMINDERS).unwrap(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                let showing = reminders.showing().await.unwrap();
                assert!(
                    showing
                        .iter()
                        .all(|reminder| reminder.key != MODULE_REMINDERS[0][0].key),
                    "{name}"
                );
                assert_eq!(
                    load_update_settings(&database).await.unwrap(),
                    UpdateSettings::default(),
                    "{name}"
                );
                let catalog = Catalog::new(
                    database.clone(),
                    dir.path().join("produtos"),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                let products = catalog.products().await.unwrap();
                // The catalog arrived in 0.2.0; older samples have none.
                let inventory = Arc::new(Inventory::new(
                    database.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                ));
                let orders = PurchaseOrders::new(
                    database.clone(),
                    inventory.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                let taxes = Taxes::new(
                    database.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                let listings = Listings::new(
                    database.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                // Listings arrived in 0.2.0, like the catalog.
                if Version::parse(released) >= Version::parse("0.2.0") {
                    let listing = listings.listings().await.unwrap().remove(0);
                    assert_eq!(listing.listed, sample_listing(), "{name}");
                    let product = products
                        .iter()
                        .find(|product| product.sku.as_str() == SAMPLE_SKU)
                        .unwrap_or_else(|| panic!("{name}"));
                    assert_eq!(listing.product, Some(product.id), "{name}");
                }
                listings.sync(&SampleChannel).await.unwrap();
                assert_eq!(listings.listings().await.unwrap().len(), 1, "{name}");
                // The catalog, Purchase Orders, stock, its adjustments,
                // Reorder Points, discovery settings and the tax rate arrived
                // in 0.2.0; older samples have none.
                if Version::parse(released) >= Version::parse("0.2.0") {
                    assert_eq!(
                        catalog.discovery_settings().await.unwrap(),
                        sample_discovery_settings(),
                        "{name}"
                    );
                    assert_eq!(
                        catalog.demand_categories().await.unwrap(),
                        [sample_category()],
                        "{name}"
                    );
                    assert_eq!(
                        taxes.rate().await.unwrap(),
                        Percentage::new(SAMPLE_TAX_PERCENT.into()).unwrap(),
                        "{name}"
                    );
                    let product = products
                        .iter()
                        .find(|product| product.sku.as_str() == SAMPLE_SKU)
                        .unwrap_or_else(|| panic!("{name}"));
                    let order = orders.purchase_orders().await.unwrap().remove(0);
                    assert_eq!(
                        order.status(),
                        PurchaseOrderStatus::PartlyReceived,
                        "{name}"
                    );
                    let stock = inventory.stock().await.unwrap();
                    let on_hand = i64::from(SAMPLE_RECEIVED - SAMPLE_LOST);
                    assert_eq!(stock.products[0].valuation.quantity(), on_hand, "{name}");
                    assert_eq!(stock.products[0].product, product.id, "{name}");
                    assert_eq!(
                        inventory.low_stock().await.unwrap(),
                        [LowStock {
                            product: product.id,
                            quantity: on_hand,
                            reorder_point: SAMPLE_REORDER_POINT,
                        }],
                        "{name}"
                    );
                }
                catalog.add_supplier("Loja nova").await.unwrap();
                assert!(!inventory.locations().await.unwrap().is_empty(), "{name}");
                let any_product = RecordId::from_u128(1);
                inventory
                    .set_reorder_point(any_product, Some(0))
                    .await
                    .unwrap();
                assert!(
                    inventory
                        .low_stock()
                        .await
                        .unwrap()
                        .iter()
                        .any(|low| low.product == any_product),
                    "{name}"
                );
                orders.purchase_orders().await.unwrap();
                // Target margins arrived in 0.2.0; every older database starts
                // with the default and takes a Product's own.
                let pricing = Pricing::new(
                    database.clone(),
                    inventory.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                assert_eq!(
                    pricing.default_target_margin().await.unwrap(),
                    Percentage::new(20.into()).unwrap(),
                    "{name}"
                );
                pricing
                    .set_target_margin(any_product, Some(Percentage::new(30.into()).unwrap()))
                    .await
                    .unwrap();
                assert!(
                    pricing.target_margin(any_product).await.unwrap().own,
                    "{name}"
                );
                catalog
                    .opportunities(&OpportunityFilter::default(), taxes.rate().await.unwrap())
                    .await
                    .unwrap();
                catalog
                    .follow_categories(&[sample_category()])
                    .await
                    .unwrap();
                // Listing Quality arrived after 0.2.0: older samples start
                // without it, and it is read in the next Sync.
                let quality = ListingQuality::new(
                    database.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                quality
                    .sync(&SampleQuality, &[sample_listing().id])
                    .await
                    .unwrap();
                let panel = quality
                    .panel(&[ListedItem {
                        listing: sample_listing().id,
                        title: sample_listing().title,
                        link: None,
                        units_sold: 0,
                    }])
                    .await
                    .unwrap();
                assert_eq!(panel[0].rating, Rating::Rated(sample_quality()), "{name}");
                // Questions and reply templates arrived after 0.2.0 as well.
                let questions = Questions::new(
                    database.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                questions.sync(&SampleQuestions).await.unwrap();
                let inbox = questions.inbox().await.unwrap();
                assert_eq!(inbox.len(), 1, "{name}");
                assert_eq!(inbox[0].asked, sample_question(), "{name}");
                let templates = ReplyTemplates::new(
                    database.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                // A sample written after 0.2.0 already has the template.
                if templates.templates().await.unwrap().is_empty() {
                    templates.create(sample_template()).await.unwrap();
                }
                assert_eq!(templates.templates().await.unwrap().len(), 1, "{name}");
                // The Reputation and the Reviews arrived after 0.2.0 too.
                let reputation = Reputation::new(
                    database.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                reputation
                    .sync(&SampleReputation, &[sample_listing().id])
                    .await
                    .unwrap();
                let standing = reputation.standing().await.unwrap().unwrap();
                assert_eq!(standing.reputation, sample_reputation(), "{name}");
                assert_eq!(reputation.low_reviews().await.unwrap().len(), 1, "{name}");
            });
        }
    }

    #[test]
    fn the_risky_migrations_list_is_json_of_every_risky_one() {
        let expected: Vec<String> = MODULE_MIGRATIONS
            .iter()
            .flat_map(|module| {
                module
                    .migrations
                    .iter()
                    .filter(|migration| migration.risky)
                    .map(|migration| format!("{}:{}", module.module, migration.version))
            })
            .collect();
        let json = risky_migrations_json();
        assert!(json.starts_with('[') && json.ends_with(']'), "{json}");
        for risky in &expected {
            let (module, version) = risky.split_once(':').unwrap();
            assert!(
                json.contains(&format!(
                    r#"{{"module": "{module}", "version": {version}}}"#
                )),
                "{json}"
            );
        }
        if expected.is_empty() {
            assert_eq!(json, "[]");
        }
    }

    #[test]
    fn this_version_reads_as_a_release_version() {
        assert_eq!(this_version().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn every_module_declares_flags_and_reminders_under_distinct_keys() {
        assert!(Registry::new(MODULE_FLAGS).is_ok());
        assert!(Registry::new(MODULE_REMINDERS).is_ok());
    }
}
