use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures::executor::block_on;
use mascate_catalog::Catalog;
use mascate_kernel::{SystemClock, UuidV7Generator};
use mascate_platform::{
    Appearance, BackupSettings, Backups, Database, Flag, Flags, Installation, ModuleMigrations,
    OpenError, Opened, Registry, ReleaseChannel, Reminder, Reminders, UpdateSettings, Updater,
    Version, apply_staged_restore, default_backup_folder, default_database_path,
    default_owner_folder, load_appearance, load_update_settings, open_and_migrate,
    undo_applied_restore,
};

/// Every module's migrations, in dependency order. Modules add theirs here.
pub const MODULE_MIGRATIONS: &[ModuleMigrations] =
    &[mascate_platform::MIGRATIONS, mascate_catalog::MIGRATIONS];

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
            let backup_settings = backups.settings().await?;
            let update_settings = load_update_settings(&database).await?;
            Ok::<_, Box<dyn std::error::Error>>(Started {
                database: database.clone(),
                appearance,
                flags: Arc::new(flags),
                reminders: Arc::new(reminders),
                backups: Arc::new(backups),
                catalog: Arc::new(catalog),
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
    use mascate_catalog::NewSupplierOffer;
    use mascate_kernel::{Currency, Money};
    use mascate_platform::{LayoutId, UiTheme, UiThemePreference, save_appearance};

    use super::*;

    const SAMPLE_SKU: &str = "EXEMPLO-001";

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
            catalog
                .create_product(offer.id, "Produto de exemplo", SAMPLE_SKU)
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
                if Version::parse(released) >= Version::parse("0.2.0") {
                    assert!(
                        products
                            .iter()
                            .any(|product| product.sku.as_str() == SAMPLE_SKU),
                        "{name}"
                    );
                }
                catalog.add_supplier("Loja nova").await.unwrap();
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
