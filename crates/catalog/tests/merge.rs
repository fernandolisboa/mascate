//! Product folders after a Merge Import, through the catalog's public
//! interface: two real temporary databases and folders stand for two
//! machines, merged by the platform's Merge Import.

use std::path::Path;
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_catalog::{Catalog, MERGE, MIGRATIONS};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::{
    Backups, Database, FollowUp, MergeImport, ModuleMerge, ModuleMigrations, migrate,
};

const APP: &[ModuleMigrations] = &[mascate_platform::MIGRATIONS, MIGRATIONS];
const MERGES: &[ModuleMerge] = &[mascate_platform::MERGE, MERGE];

async fn machine(path: &Path, clock: &Arc<ManualClock>, first_id: u64) -> (Arc<Database>, Catalog) {
    let database = Arc::new(Database::open(path).await.unwrap());
    migrate(&database, clock.as_ref(), APP).await.unwrap();
    let ids = SequentialIds::default();
    for _ in 0..first_id {
        mascate_kernel::IdGenerator::next_id(&ids);
    }
    let catalog = Catalog::new(
        database.clone(),
        path.with_extension("files"),
        clock.clone(),
        Arc::new(ids),
    );
    (database, catalog)
}

async fn export(database: &Database, to: &Path) {
    database
        .connection()
        .execute("VACUUM INTO ?1", [to.to_str().unwrap()])
        .await
        .unwrap();
}

#[test]
fn folders_follow_the_skus_a_merge_changed_and_what_cannot_follow_is_said() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap(),
        ));
        let (here, catalog) = machine(&dir.path().join("here.db"), &clock, 0).await;
        let fone = catalog.add_product("Fone", "FONE-01").await.unwrap();
        let capa = catalog.add_product("Capa", "CAPA-01").await.unwrap();
        let cabo = catalog.add_product("Cabo", "CABO-01").await.unwrap();
        std::fs::write(catalog.folder(&fone).join("nota.txt"), b"nota").unwrap();
        export(&here, &dir.path().join("there.db")).await;
        let (there, other) = machine(&dir.path().join("there.db"), &clock, 1_000).await;
        clock.advance(TimeDelta::minutes(5));
        other.rename_sku(fone.id, "FONE-BT").await.unwrap();
        other.rename_sku(capa.id, "CAPA-AZUL").await.unwrap();
        // A chain: the cable takes the fone's old SKU once the fone leaves it.
        other.rename_sku(cabo.id, "FONE-01").await.unwrap();
        other.add_product("Película", "PELICULA").await.unwrap();
        // A folder here already takes the cover's new name.
        std::fs::create_dir_all(dir.path().join("here.files/CAPA-AZUL")).unwrap();
        let file = dir.path().join("there-export.db");
        export(&there, &file).await;
        let before = catalog.products().await.unwrap();
        let merges = MergeImport::new(
            here.clone(),
            APP,
            MERGES,
            clock.clone(),
            Arc::new(SequentialIds::default()),
        );
        let backups = Backups::new(
            here.clone(),
            APP,
            dir.path().join("backups"),
            clock.clone(),
            Arc::new(SequentialIds::default()),
        );
        merges.merge(&file, &backups).await.unwrap().unwrap();

        let follow_ups = catalog.follow_merge(&before).await.unwrap();

        let fone = catalog.product(fone.id).await.unwrap();
        assert_eq!(fone.sku.as_str(), "FONE-BT");
        assert_eq!(
            std::fs::read(catalog.folder(&fone).join("nota.txt")).unwrap(),
            b"nota"
        );
        let cabo = catalog.product(cabo.id).await.unwrap();
        assert!(catalog.folder(&cabo).is_dir());
        assert!(!dir.path().join("here.files/CABO-01").exists());
        // The cover keeps its folder under the old name.
        assert!(dir.path().join("here.files/CAPA-01").is_dir());
        assert_eq!(follow_ups.len(), 2, "{follow_ups:#?}");
        assert!(matches!(
            &follow_ups[0],
            FollowUp::FolderNotRenamed { product, .. } if product == "CAPA-AZUL · Capa"
        ));
        assert_eq!(
            follow_ups[1],
            FollowUp::CopyFolder {
                product: "PELICULA · Película".into()
            }
        );
    });
}
