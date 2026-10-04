use std::sync::Arc;

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_integrations::{Connection, Connections};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::testing::MemorySecretStore;
use mascate_platform::{Backups, Database, MIGRATIONS, ModuleMigrations, migrate};

const MODULES: &[ModuleMigrations] = &[MIGRATIONS];

#[test]
fn a_backup_never_holds_a_connection_secret() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
        ));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), MODULES).await.unwrap();
        let connections = Connections::new(Arc::new(MemorySecretStore::default()));
        connections
            .save(
                Connection::Anthropic,
                &[("ANTHROPIC_API_KEY", "sk-ant-never-in-a-backup")],
            )
            .unwrap();
        let backups = Backups::new(
            database,
            MODULES,
            dir.path().join("backups"),
            clock,
            Arc::new(SequentialIds::default()),
        );

        let backup = backups.back_up_now().await.unwrap();

        let bytes = std::fs::read(&backup.path).unwrap();
        let secret = b"sk-ant-never-in-a-backup";
        assert!(!bytes.windows(secret.len()).any(|window| window == secret));
    });
}
