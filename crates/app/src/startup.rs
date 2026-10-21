use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::executor::block_on;
use mascate_affiliate::Affiliate;
use mascate_catalog::Catalog;
use mascate_commerce::{
    ChannelRanking, Listings, Orders, Pricing, Promotions, PurchaseOrders, Repricing, StockMirror,
};
use mascate_finance::{SalesVolume, Taxes};
use mascate_inventory::Inventory;
use mascate_kernel::{SystemClock, UuidV7Generator};
use mascate_marketing::{
    ListingCopy, ListingQuality, OfferPosts, ProductAds, PromotionChannels, Questions,
    ReplyTemplates, Reputation, SalesEvents,
};
use mascate_platform::{
    Appearance, AppliedMerge, BackupSettings, Backups, Database, Flag, Flags, Installation,
    MergeImport, ModuleMerge, ModuleMigrations, OpenError, Opened, Registry, ReleaseChannel,
    Reminder, Reminders, UpdateSettings, Updater, Version, apply_staged_restore,
    default_backup_folder, default_database_path, default_owner_folder, load_appearance,
    load_update_settings, open_and_migrate, undo_applied_restore,
};

/// Every module's migrations, in dependency order. Modules add theirs here.
pub const MODULE_MIGRATIONS: &[ModuleMigrations] = &[
    mascate_platform::MIGRATIONS,
    mascate_catalog::MIGRATIONS,
    mascate_inventory::MIGRATIONS,
    mascate_commerce::MIGRATIONS,
    mascate_marketing::MIGRATIONS,
    mascate_finance::MIGRATIONS,
    mascate_affiliate::MIGRATIONS,
];

/// How every module's tables come together in a Merge Import (ADR 0032).
/// Modules add theirs here, next to their migrations.
pub const MODULE_MERGES: &[ModuleMerge] = &[
    mascate_platform::MERGE,
    mascate_catalog::MERGE,
    mascate_inventory::MERGE,
    mascate_commerce::MERGE,
    mascate_marketing::MERGE,
    mascate_finance::MERGE,
    mascate_affiliate::MERGE,
];

/// Every module's flags, in the order the settings screen lists them.
const MODULE_FLAGS: &[&[Flag]] = &[
    mascate_marketing::FLAGS,
    mascate_affiliate::FLAGS,
    mascate_commerce::FLAGS,
    mascate_integrations::FLAGS,
];

/// Every module's Reminders, in the order the home screen shows them.
const MODULE_REMINDERS: &[&[Reminder]] = &[
    mascate_finance::REMINDERS,
    mascate_commerce::REMINDERS,
    mascate_affiliate::REMINDERS,
    mascate_marketing::REMINDERS,
    mascate_integrations::REMINDERS,
];

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
    pub merge_import: Arc<MergeImport>,
    pub catalog: Arc<Catalog>,
    pub inventory: Arc<Inventory>,
    pub purchase_orders: Arc<PurchaseOrders>,
    pub listings: Arc<Listings>,
    pub pricing: Arc<Pricing>,
    pub stock_mirror: Arc<StockMirror>,
    pub channel_ranking: Arc<ChannelRanking>,
    pub orders: Arc<Orders>,
    pub promotions: Arc<Promotions>,
    pub repricing: Arc<Repricing>,
    pub quality: Arc<ListingQuality>,
    pub listing_copy: Arc<ListingCopy>,
    pub questions: Arc<Questions>,
    pub reputation: Arc<Reputation>,
    pub product_ads: Arc<ProductAds>,
    pub reply_templates: Arc<ReplyTemplates>,
    pub promotion_channels: Arc<PromotionChannels>,
    pub offer_posts: Arc<OfferPosts>,
    pub sales_events: Arc<SalesEvents>,
    pub taxes: Arc<Taxes>,
    pub sales_volume: Arc<SalesVolume>,
    pub affiliate: Arc<Affiliate>,
    pub backup_settings: BackupSettings,
    pub update_settings: UpdateSettings,
    /// What became of a restore staged before the restart, if one was.
    pub restore: Option<Result<(), String>>,
    /// The Merge Import applied before the restart, which the app opens on.
    pub merged: Option<AppliedMerge>,
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
            let merge_import = MergeImport::new(
                database.clone(),
                MODULE_MIGRATIONS,
                MODULE_MERGES,
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
            let channel_ranking = ChannelRanking::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let orders = Orders::new(
                database.clone(),
                inventory.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let promotions = Promotions::new(
                database.clone(),
                inventory.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let repricing = Repricing::new(
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
            let listing_copy = ListingCopy::new(
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
            let product_ads = ProductAds::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let reply_templates = ReplyTemplates::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let promotion_channels = PromotionChannels::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let offer_posts = OfferPosts::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            offer_posts.settle_interrupted().await?;
            let sales_events = SalesEvents::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            sales_events.record_defaults().await?;
            let taxes = Taxes::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let sales_volume = SalesVolume::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let affiliate = Affiliate::new(
                database.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            );
            let backup_settings = backups.settings().await?;
            let update_settings = load_update_settings(&database).await?;
            let merged = merge_import.open_unopened().await.unwrap_or_else(|error| {
                // A report that cannot be read is no reason to give up the database.
                eprintln!("could not read the last Merge Import: {error}");
                None
            });
            Ok::<_, Box<dyn std::error::Error>>(Started {
                database: database.clone(),
                appearance,
                flags: Arc::new(flags),
                reminders: Arc::new(reminders),
                backups: Arc::new(backups),
                merge_import: Arc::new(merge_import),
                catalog: Arc::new(catalog),
                inventory,
                purchase_orders: Arc::new(purchase_orders),
                listings: Arc::new(listings),
                pricing: Arc::new(pricing),
                stock_mirror: Arc::new(stock_mirror),
                channel_ranking: Arc::new(channel_ranking),
                orders: Arc::new(orders),
                promotions: Arc::new(promotions),
                repricing: Arc::new(repricing),
                quality: Arc::new(quality),
                listing_copy: Arc::new(listing_copy),
                questions: Arc::new(questions),
                reputation: Arc::new(reputation),
                product_ads: Arc::new(product_ads),
                reply_templates: Arc::new(reply_templates),
                promotion_channels: Arc::new(promotion_channels),
                offer_posts: Arc::new(offer_posts),
                sales_events: Arc::new(sales_events),
                taxes: Arc::new(taxes),
                sales_volume: Arc::new(sales_volume),
                affiliate: Arc::new(affiliate),
                backup_settings,
                update_settings,
                restore,
                merged,
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
    use mascate_affiliate::{
        AffiliatePlatform, CommissionFilter, CommissionStatus, LinkTagger, ManualCommission,
        ManualPayout, NewAffiliateLink, Practice, Promotion, PromotionRules, Standing, TagRefusal,
    };
    use mascate_catalog::{DemandCategory, DiscoverySettings, NewSupplierOffer, OpportunityFilter};
    use mascate_commerce::{
        ChannelCategory, ChannelListing, ChannelOffer, ChannelPromotion, ChannelPromotions,
        ChannelStock, FeeBasis, ListingStatus, NewPurchaseLine, NewPurchaseOrder, OfferToJoin,
        OnSalesChannel, PromotionKind, PromotionPlan, PromotionStatus, PurchaseOrderStatus,
        Receiving, SaleFee, SalesChannel,
    };
    use mascate_integrations::Connections;
    use mascate_inventory::{HOME_LOCATION, LowStock, StockAdjustment};
    use mascate_kernel::{
        Clock, Currency, ListingType, Money, Percentage, PlatformError, RecordId, SalesChannelKind,
    };
    use mascate_marketing::{
        AdMetrics, CampaignStatus, ChannelAd, ChannelAds, ChannelCampaign, ChannelKind,
        ChannelQuality, ChannelQuestion, ChannelQuestions, ChannelReputation, ChannelReview,
        CopySettings, DateRule, ListedItem, ListingReviews, MetricReading, NewPromotionChannel,
        NewReplyTemplate, NewSalesEvent, OfferFacts, OfferPrice, PostStatus, PostTime,
        QualityLevel, QualitySource, QuestionStatus, Rating, ReputationColor, ReputationSource,
        Score, default_posting_times,
    };
    use mascate_platform::testing::MemorySecretStore;
    use mascate_platform::{FollowUp, LayoutId, Side, UiTheme, UiThemePreference, save_appearance};

    use super::*;

    const SAMPLE_SKU: &str = "EXEMPLO-001";
    const SAMPLE_ORDERED: u32 = 3;
    const SAMPLE_RECEIVED: u32 = 2;
    const SAMPLE_LOST: u32 = 1;
    const SAMPLE_REORDER_POINT: u32 = 5;
    const SAMPLE_TAX_PERCENT: u32 = 6;
    const SAMPLE_OTHER_TAX_PERCENT: u32 = 7;
    const SAMPLE_MERGED_SUPPLIER: &str = "Loja do outro computador";

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

    impl OnSalesChannel for SampleChannel {
        fn sales_channel(&self) -> SalesChannelKind {
            SalesChannelKind::MercadoLivre
        }
    }

    impl SalesChannel for SampleChannel {
        fn listing_ids(&self) -> Result<Vec<String>, PlatformError> {
            Ok(vec![sample_listing().id])
        }

        fn listings(&self, _: &[String]) -> Result<Vec<ChannelListing>, PlatformError> {
            Ok(vec![sample_listing()])
        }

        fn sale_fee(&self, _: &FeeBasis, _: Money) -> Result<SaleFee, PlatformError> {
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

        fn category(&self, id: &str) -> Result<ChannelCategory, PlatformError> {
            Ok(ChannelCategory {
                id: id.into(),
                name: "Categoria de exemplo".into(),
            })
        }
    }

    fn sample_quality() -> ChannelQuality {
        ChannelQuality {
            score: Some(Score {
                points: 48,
                level: QualityLevel::Basic,
            }),
            pending: Vec::new(),
        }
    }

    /// Rates every listing like the sample, on Mercado Livre.
    struct SampleQuality;

    impl QualitySource for SampleQuality {
        fn rated_channel(&self) -> SalesChannelKind {
            SalesChannelKind::MercadoLivre
        }

        fn quality(&self, _: &str) -> Result<Option<ChannelQuality>, PlatformError> {
            Ok(Some(sample_quality()))
        }

        fn visits(&self, _: &str, _: u32) -> Result<Option<u32>, PlatformError> {
            Ok(Some(7))
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

    /// One campaign advertising the sample listing, R$ 2,50 a day.
    struct SampleAds;

    impl ChannelAds for SampleAds {
        fn account(&self) -> Result<Option<String>, PlatformError> {
            Ok(Some("1".into()))
        }

        fn campaigns(&self, _: &str) -> Result<Vec<ChannelCampaign>, PlatformError> {
            Ok(vec![ChannelCampaign {
                id: "1".into(),
                name: "Campanha de exemplo".into(),
                status: CampaignStatus::Active,
            }])
        }

        fn ads_on(&self, _: &str, _: chrono::NaiveDate) -> Result<Vec<ChannelAd>, PlatformError> {
            Ok(vec![ChannelAd {
                listing: sample_listing().id,
                campaign: Some("1".into()),
                metrics: AdMetrics {
                    cost: Money::new(rust_decimal::Decimal::new(250, 2), Currency::Brl),
                    ..AdMetrics::zero(Currency::Brl)
                },
            }])
        }
    }

    /// The sample listing with a price discount, and nothing else.
    struct SamplePromotions;

    impl OnSalesChannel for SamplePromotions {
        fn sales_channel(&self) -> SalesChannelKind {
            SalesChannelKind::MercadoLivre
        }
    }

    impl ChannelPromotions for SamplePromotions {
        fn promotions(&self) -> Result<Vec<ChannelPromotion>, PlatformError> {
            Ok(Vec::new())
        }

        fn offers(&self, listing: &str) -> Result<Vec<ChannelOffer>, PlatformError> {
            Ok(vec![ChannelOffer {
                listing: listing.into(),
                kind: PromotionKind::PriceDiscount,
                promotion: None,
                name: "Desconto".into(),
                status: PromotionStatus::Started,
                price: Some(Money::new(55.into(), Currency::Brl)),
                starts: None,
                ends: None,
            }])
        }

        fn create(&self, _: &PromotionPlan) -> Result<ChannelPromotion, PlatformError> {
            Err(PlatformError::NotFound)
        }

        fn change(&self, _: &str, _: &PromotionPlan) -> Result<(), PlatformError> {
            Err(PlatformError::NotFound)
        }

        fn end(&self, _: &str, _: PromotionKind) -> Result<(), PlatformError> {
            Err(PlatformError::NotFound)
        }

        fn join(&self, _: &str, _: &OfferToJoin) -> Result<(), PlatformError> {
            Err(PlatformError::NotFound)
        }

        fn leave(&self, _: &str, _: PromotionKind, _: Option<&str>) -> Result<(), PlatformError> {
            Err(PlatformError::NotFound)
        }
    }

    fn sample_commission() -> ManualCommission {
        ManualCommission {
            platform: AffiliatePlatform::MercadoLivre,
            program: String::new(),
            product: "Produto de exemplo".into(),
            sold_at: chrono::DateTime::parse_from_rfc3339("2026-10-01T10:00:00-03:00")
                .unwrap()
                .with_timezone(&chrono::Utc),
            amount: Money::new(rust_decimal::Decimal::new(1240, 2), Currency::Brl),
            status: CommissionStatus::Approved,
        }
    }

    fn sample_rules() -> PromotionRules {
        let mut rules = PromotionRules::defaults(AffiliatePlatform::MercadoLivre)
            .with(Practice::PaidSearch, Standing::Forbidden)
            .with(Practice::BoostedPosts, Standing::Allowed);
        rules.brand_keywords = vec!["mercado livre".into()];
        rules
    }

    fn sample_channel() -> NewPromotionChannel {
        NewPromotionChannel {
            name: "Canal de exemplo".into(),
            kind: ChannelKind::TelegramChannel,
            default_tag: "exemplo".into(),
            keywords: Vec::new(),
            telegram_chat: Some("@canaldeexemplo".into()),
        }
    }

    fn sample_link(program: RecordId, channel: RecordId) -> NewAffiliateLink {
        NewAffiliateLink {
            program,
            product: "Produto de exemplo".into(),
            pasted: "https://example.com/afiliado/123".into(),
            channel,
            tag: sample_channel().default_tag,
            promotion: Promotion::default(),
        }
    }

    /// Tags the sample Program's links the way the URL taggers do.
    struct SampleTagger;

    impl LinkTagger for SampleTagger {
        fn platform(&self) -> AffiliatePlatform {
            AffiliatePlatform::MercadoLivre
        }

        fn tag(&self, link: &str, tag: &str) -> Result<String, TagRefusal> {
            Ok(format!("{link}?tag={tag}"))
        }
    }

    const SAMPLE_POST_TEMPLATE: &str = "{produto} por {preco}\n{link}";

    fn sample_facts(link: RecordId, tagged: &str) -> OfferFacts {
        OfferFacts {
            link_id: link,
            link: tagged.into(),
            product: "Produto de exemplo".into(),
            platform: "Mercado Livre".into(),
            price: Some(OfferPrice {
                price: Money::new(59.into(), Currency::Brl),
                before: None,
                seen_at: chrono::DateTime::parse_from_rfc3339("2026-10-13T10:00:00-03:00")
                    .unwrap()
                    .with_timezone(&chrono::Utc),
            }),
            image: None,
            price_needs_time: false,
            forbidden_contacts: Vec::new(),
        }
    }

    fn sample_posting_times() -> Vec<chrono::NaiveTime> {
        vec![
            chrono::NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            chrono::NaiveTime::from_hms_opt(19, 30, 0).unwrap(),
        ]
    }

    /// Far enough ahead that writing the sample never finds it past.
    fn sample_scheduled_at() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2099-01-02T12:00:00-03:00")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    fn sample_event() -> NewSalesEvent {
        NewSalesEvent {
            name: "Aniversário da loja".into(),
            rule: DateRule::Yearly { day: 8, month: 7 },
            lead_days: 10,
            notes: "Cupom de 10% no canal".into(),
        }
    }

    /// The default Sales Event the sample deletes, which never comes back.
    const SAMPLE_DELETED_EVENT: &str = "Cyber Monday";

    fn sample_payout() -> ManualPayout {
        ManualPayout {
            platform: AffiliatePlatform::MercadoLivre,
            paid_at: chrono::DateTime::parse_from_rfc3339("2026-10-10T00:00:00-03:00")
                .unwrap()
                .with_timezone(&chrono::Utc),
            amount: Money::new(rust_decimal::Decimal::new(1200, 2), Currency::Brl),
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
            Promotions::new(
                database.clone(),
                inventory.clone(),
                clock.clone(),
                ids.clone(),
            )
            .sync(&SamplePromotions)
            .await
            .unwrap();
            ProductAds::new(database.clone(), clock.clone(), ids.clone())
                .sync(&SampleAds)
                .await
                .unwrap();
            let affiliate = Affiliate::new(database.clone(), clock.clone(), ids.clone());
            let commission = affiliate.record_manual(sample_commission()).await.unwrap();
            affiliate
                .record_payout(sample_payout(), &[commission.id])
                .await
                .unwrap();
            affiliate
                .set_promotion_rules(commission.program, &sample_rules())
                .await
                .unwrap();
            let channels = PromotionChannels::new(database.clone(), clock.clone(), ids.clone());
            let channel = channels.create(&sample_channel()).await.unwrap();
            let channel = channels
                .set_posting_times(channel.id, &sample_posting_times())
                .await
                .unwrap();
            affiliate
                .link_product(commission.program, "Produto de exemplo", product.id)
                .await
                .unwrap();
            let link = affiliate
                .create_link(&sample_link(commission.program, channel.id), &SampleTagger)
                .await
                .unwrap()
                .link;
            let posts = OfferPosts::new(database.clone(), clock.clone(), ids.clone());
            posts
                .set_template(channel.id, SAMPLE_POST_TEMPLATE)
                .await
                .unwrap();
            let post = posts
                .create(
                    &posts
                        .draft(&sample_facts(link.id, &link.tagged), &channel)
                        .await
                        .unwrap(),
                )
                .await
                .unwrap();
            posts.copied(post.id).await.unwrap();
            let request = posts.prepare(post.id).await.unwrap();
            posts
                .schedule(request.confirm(), PostTime::At(sample_scheduled_at()))
                .await
                .unwrap();
            posts.set_queue_paused(true).await.unwrap();
            let events = SalesEvents::new(database.clone(), clock.clone(), ids.clone());
            events.record_defaults().await.unwrap();
            let deleted = events
                .events(clock.now())
                .await
                .unwrap()
                .into_iter()
                .find(|event| event.name == SAMPLE_DELETED_EVENT)
                .unwrap();
            events.delete(deleted.id).await.unwrap();
            events.create(&sample_event()).await.unwrap();
            let backups = Backups::new(
                database.clone(),
                MODULE_MIGRATIONS,
                std::env::temp_dir().join("mascate-sample-backups"),
                clock,
                ids,
            );
            merge_another_computers_copy(&database, &backups, &catalog).await;
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

    /// What 0.4.0 saves: a Merge Import of another computer's copy of the
    /// sample, which brings a Supplier and loses the tax rate both sides
    /// changed, with what is left by hand already seen.
    async fn merge_another_computers_copy(
        database: &Arc<Database>,
        backups: &Backups,
        catalog: &Catalog,
    ) {
        let other = std::env::temp_dir().join("mascate-outro-computador.db");
        let _ = std::fs::remove_file(&other);
        database
            .connection()
            .execute("VACUUM INTO ?1", [other.to_str().unwrap()])
            .await
            .unwrap();
        {
            let other = open(&other, &std::env::temp_dir()).await.unwrap().database;
            Catalog::new(
                other.clone(),
                std::env::temp_dir().join("mascate-outro-computador-produtos"),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            )
            .add_supplier(SAMPLE_MERGED_SUPPLIER)
            .await
            .unwrap();
            Taxes::new(
                other.clone(),
                Arc::new(SystemClock),
                Arc::new(UuidV7Generator),
            )
            .save_rate(Percentage::new(SAMPLE_OTHER_TAX_PERCENT.into()).unwrap())
            .await
            .unwrap();
            other
                .connection()
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .await
                .unwrap();
        }
        // Changed here after the other computer did: this rate stays.
        Taxes::new(
            database.clone(),
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
        )
        .save_rate(Percentage::new(SAMPLE_TAX_PERCENT.into()).unwrap())
        .await
        .unwrap();
        let merge_import = MergeImport::new(
            database.clone(),
            MODULE_MIGRATIONS,
            MODULE_MERGES,
            Arc::new(SystemClock),
            Arc::new(UuidV7Generator),
        );
        let connections = Connections::new(Arc::new(MemorySecretStore::default()));
        crate::merge_import::merge(&other, &merge_import, backups, catalog, &connections)
            .await
            .unwrap()
            .expect("the other computer brings a Supplier");
        merge_import.open_unopened().await.unwrap();
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
                let showing = reminders.showing(&[]).await.unwrap();
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
                    assert_eq!(listing.channel, SalesChannelKind::MercadoLivre, "{name}");
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
                let listed = |channel| ListedItem {
                    channel,
                    listing: sample_listing().id,
                    title: sample_listing().title,
                    link: None,
                    units_sold: 0,
                };
                let ml = listed(SalesChannelKind::MercadoLivre);
                // The rows from before Sales Channels in Marketing (#136)
                // stay as they were, on Mercado Livre.
                if Version::parse(released) >= Version::parse("0.2.0") {
                    let kept = quality.panel(std::slice::from_ref(&ml)).await.unwrap();
                    assert_eq!(kept[0].rating, Rating::Rated(sample_quality()), "{name}");
                    assert_eq!(kept[0].visits, Some(7), "{name}");
                    let elsewhere = quality
                        .panel(&[listed(SalesChannelKind::Shopee)])
                        .await
                        .unwrap();
                    assert_eq!(elsewhere[0].rating, Rating::NotChecked, "{name}");
                }
                quality
                    .sync(&SampleQuality, &[sample_listing().id])
                    .await
                    .unwrap();
                let panel = quality.panel(&[ml]).await.unwrap();
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
                // Promotions arrived after 0.2.0 as well: the minimum margin
                // starts at its default and the next Sync reads them.
                let promotions = Promotions::new(
                    database.clone(),
                    inventory.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                assert_eq!(
                    promotions.minimum_margin().await.unwrap(),
                    Percentage::new(10.into()).unwrap(),
                    "{name}"
                );
                // Their Sync cursor became Mercado Livre's (ADR 0033).
                if Version::parse(released) >= Version::parse("0.2.0") {
                    let ml = SalesChannelKind::MercadoLivre;
                    assert!(promotions.last_sync(ml).await.unwrap().is_some(), "{name}");
                    let offers = promotions.offers().await.unwrap();
                    assert!(offers.iter().all(|offer| offer.channel == ml), "{name}");
                }
                promotions.sync(&SamplePromotions).await.unwrap();
                let offers = promotions.offers().await.unwrap();
                assert_eq!(offers.len(), 1, "{name}");
                assert_eq!(offers[0].channel, SalesChannelKind::MercadoLivre, "{name}");
                assert_eq!(offers[0].listed.listing, sample_listing().id, "{name}");
                // Product Ads arrived after 0.2.0 too, read in the next Sync.
                let ads = ProductAds::new(
                    database.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                ads.sync(&SampleAds).await.unwrap();
                let costs = ads.costs().await.unwrap();
                assert!(!costs.is_empty(), "{name}");
                assert!(
                    costs.iter().all(|day| day.listing == sample_listing().id),
                    "{name}"
                );
                // Category names and the sales volume limit arrived after
                // 0.2.0 too: the next listing Sync names the categories.
                let listings = Listings::new(
                    database.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                listings.sync(&SampleChannel).await.unwrap();
                assert_eq!(
                    listings
                        .category_names(SalesChannelKind::MercadoLivre)
                        .await
                        .unwrap()["MLB1000"],
                    "Categoria de exemplo",
                    "{name}"
                );
                let volume = SalesVolume::new(
                    database.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                let limit = Money::new(90_000.into(), Currency::Brl);
                volume.save_limit(limit).await.unwrap();
                assert_eq!(volume.limit().await.unwrap(), limit, "{name}");
                // The Listing Copy model arrived after 0.2.0: the default
                // until the owner picks another.
                let copy = ListingCopy::new(
                    database.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                assert_eq!(
                    copy.settings().await.unwrap(),
                    CopySettings::default(),
                    "{name}"
                );
                copy.save_settings(CopySettings {
                    model: "claude-opus-5-5".into(),
                })
                .await
                .unwrap();
                assert_eq!(
                    copy.settings().await.unwrap().model,
                    "claude-opus-5-5",
                    "{name}"
                );
                // Commissions arrived after 0.2.0: a sample written after
                // it already has the manual one.
                let affiliate = Affiliate::new(
                    database.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                let everything = CommissionFilter::default();
                if affiliate.commissions(&everything).await.unwrap().is_empty() {
                    affiliate.record_manual(sample_commission()).await.unwrap();
                }
                let commissions = affiliate.commissions(&everything).await.unwrap();
                assert_eq!(commissions.len(), 1, "{name}");
                assert_eq!(commissions[0].amount, sample_commission().amount, "{name}");
                assert!(commissions[0].manual, "{name}");
                // Payouts arrived after 0.2.0 too.
                let ever = chrono::DateTime::<chrono::Utc>::MIN_UTC
                    ..chrono::DateTime::<chrono::Utc>::MAX_UTC;
                if affiliate.payouts(ever.clone()).await.unwrap().is_empty() {
                    affiliate
                        .record_payout(sample_payout(), &[commissions[0].id])
                        .await
                        .unwrap();
                }
                let payouts = affiliate.payouts(ever).await.unwrap();
                assert_eq!(payouts.len(), 1, "{name}");
                assert_eq!(payouts[0].covers.len(), 1, "{name}");
                assert_eq!(
                    payouts[0].difference(),
                    Money::new(rust_decimal::Decimal::new(-40, 2), Currency::Brl),
                    "{name}"
                );
                // Promotion rules arrived after 0.2.0: an older sample's
                // Program reads its Platform's defaults.
                let program = affiliate.program(commissions[0].program).await.unwrap();
                if program.rules == PromotionRules::defaults(program.platform) {
                    affiliate
                        .set_promotion_rules(program.id, &sample_rules())
                        .await
                        .unwrap();
                }
                assert_eq!(
                    affiliate.program(program.id).await.unwrap().rules,
                    sample_rules(),
                    "{name}"
                );
                // Promotion Channels and Affiliate Links arrived after
                // 0.2.0 too.
                let channels = PromotionChannels::new(
                    database.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                if channels.channels().await.unwrap().is_empty() {
                    channels.create(&sample_channel()).await.unwrap();
                }
                let kept = channels.channels().await.unwrap();
                assert_eq!(kept.len(), 1, "{name}");
                assert_eq!(kept[0].default_tag, "exemplo", "{name}");
                assert_eq!(kept[0].kind, ChannelKind::TelegramChannel, "{name}");
                // The board arrived with Pinterest; older channels have none.
                assert_eq!(kept[0].pinterest_board, None, "{name}");
                let created = affiliate
                    .create_link(&sample_link(program.id, kept[0].id), &SampleTagger)
                    .await
                    .unwrap();
                assert_eq!(
                    created.link.tagged, "https://example.com/afiliado/123?tag=exemplo",
                    "{name}"
                );
                assert_eq!(affiliate.links().await.unwrap().len(), 1, "{name}");
                // Offer Posts arrived after 0.2.0 too.
                let posts = OfferPosts::new(
                    database.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                if posts.posts().await.unwrap().is_empty() {
                    posts
                        .set_template(kept[0].id, SAMPLE_POST_TEMPLATE)
                        .await
                        .unwrap();
                    let draft = posts
                        .draft(
                            &sample_facts(created.link.id, &created.link.tagged),
                            &kept[0],
                        )
                        .await
                        .unwrap();
                    posts.create(&draft).await.unwrap();
                }
                let kept_posts = posts.posts().await.unwrap();
                assert_eq!(kept_posts.len(), 1, "{name}");
                assert_eq!(kept_posts[0].sent, None, "{name}");
                // The queue, posting times, Product links and Sales Events
                // arrived in 0.3.0; older samples start with the defaults.
                let since_0_3_0 = Version::parse(released) >= Version::parse("0.3.0");
                if since_0_3_0 {
                    assert_eq!(kept_posts[0].status, PostStatus::Scheduled, "{name}");
                    assert_eq!(
                        kept_posts[0].scheduled_at,
                        Some(sample_scheduled_at()),
                        "{name}"
                    );
                    assert!(posts.queue_paused().await.unwrap(), "{name}");
                    assert_eq!(kept[0].posting_times, sample_posting_times(), "{name}");
                    let product = products
                        .iter()
                        .find(|product| product.sku.as_str() == SAMPLE_SKU)
                        .unwrap_or_else(|| panic!("{name}"));
                    assert_eq!(
                        affiliate
                            .linked_product(program.id, "Produto de exemplo")
                            .await
                            .unwrap(),
                        Some(product.id),
                        "{name}"
                    );
                } else {
                    assert_eq!(kept_posts[0].status, PostStatus::Draft, "{name}");
                    assert!(!posts.queue_paused().await.unwrap(), "{name}");
                    assert_eq!(kept[0].posting_times, default_posting_times(), "{name}");
                }
                let events = SalesEvents::new(
                    database.clone(),
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                assert_eq!(
                    events.record_defaults().await.unwrap(),
                    !since_0_3_0,
                    "{name}"
                );
                let calendar = events.events(chrono::Utc::now()).await.unwrap();
                assert_eq!(
                    calendar
                        .iter()
                        .any(|event| event.name == SAMPLE_DELETED_EVENT),
                    !since_0_3_0,
                    "{name}"
                );
                let own = calendar
                    .iter()
                    .find(|event| event.name == sample_event().name);
                if since_0_3_0 {
                    let own = own.unwrap_or_else(|| panic!("{name}"));
                    assert_eq!(own.rule, sample_event().rule, "{name}");
                    assert_eq!(own.lead_days, sample_event().lead_days, "{name}");
                    assert_eq!(own.notes, sample_event().notes, "{name}");
                } else {
                    assert!(own.is_none(), "{name}");
                }
                assert_eq!(
                    kept_posts[0].draft.text,
                    "Produto de exemplo por R$ 59,00\n\
                     https://example.com/afiliado/123?tag=exemplo\n\n#publi · link de afiliado",
                    "{name}"
                );
                assert_eq!(
                    posts.template(&kept[0]).await.unwrap(),
                    SAMPLE_POST_TEMPLATE,
                    "{name}"
                );
                // Merge Imports arrived in 0.4.0: older samples have none.
                let merge_import = MergeImport::new(
                    database.clone(),
                    MODULE_MIGRATIONS,
                    MODULE_MERGES,
                    Arc::new(SystemClock),
                    Arc::new(UuidV7Generator),
                );
                let merges = merge_import.merges().await.unwrap();
                let merged_supplier = catalog
                    .suppliers()
                    .await
                    .unwrap()
                    .into_iter()
                    .any(|supplier| supplier.name == SAMPLE_MERGED_SUPPLIER);
                if Version::parse(released) >= Version::parse("0.4.0") {
                    assert_eq!(merges.len(), 1, "{name}");
                    let report = &merges[0].report;
                    let suppliers = report
                        .records
                        .iter()
                        .find(|records| records.noun == "Fornecedor")
                        .unwrap_or_else(|| panic!("{name}: {report:#?}"));
                    assert_eq!(suppliers.new, 1, "{name}");
                    let tax = report
                        .records
                        .iter()
                        .find(|records| records.noun == "Alíquota de imposto")
                        .unwrap_or_else(|| panic!("{name}: {report:#?}"));
                    assert_eq!(tax.conflicts.len(), 1, "{name}");
                    assert_eq!(tax.conflicts[0].winner, Side::Here, "{name}");
                    assert!(tax.conflicts[0].there.changed_at.is_some(), "{name}");
                    assert!(!merges[0].follow_ups.is_empty(), "{name}");
                    assert!(
                        merges[0]
                            .follow_ups
                            .iter()
                            .all(|follow_up| matches!(follow_up, FollowUp::ConnectHere { .. })),
                        "{name}"
                    );
                    assert!(merged_supplier, "{name}");
                } else {
                    assert!(merges.is_empty(), "{name}");
                    assert!(!merged_supplier, "{name}");
                }
                assert_eq!(merge_import.open_unopened().await.unwrap(), None, "{name}");
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
