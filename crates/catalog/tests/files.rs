mod common;

use std::path::PathBuf;

use futures::executor::block_on;
use mascate_catalog::{CatalogError, NotCopied, Product};

use common::Fixture;

async fn product(f: &Fixture) -> Product {
    let shop = f.supplier("Loja").await;
    let offer = f.offer(&shop, "https://loja.example/fone", "10").await;
    f.catalog
        .create_product(offer.id, "Fone", "FON-001")
        .await
        .unwrap()
}

/// A file outside the product folder, as dragged from the owner's desktop.
fn outside(f: &Fixture, name: &str, contents: &[u8]) -> PathBuf {
    let folder = f.dir.path().join("Desktop");
    std::fs::create_dir_all(&folder).unwrap();
    let path = folder.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

#[test]
fn dropped_files_are_copied_into_the_folder_and_listed_by_name() {
    block_on(async {
        let f = Fixture::new().await;
        let product = product(&f).await;
        let photo = outside(&f, "foto.jpg", b"jpeg");
        let note = outside(&f, "Nota de compra.pdf", b"pdf!");

        let added = f
            .catalog
            .add_files(product.id, &[photo.clone(), note.clone()])
            .await
            .unwrap();

        assert!(added.not_copied.is_empty());
        let files = f.catalog.files(product.id).await.unwrap();
        assert_eq!(files, added.copied);
        let names: Vec<_> = files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names, ["foto.jpg", "Nota de compra.pdf"]);
        assert_eq!(files[0].bytes, 4);
        assert_eq!(std::fs::read(&files[1].path).unwrap(), b"pdf!");
        assert!(files[0].path.starts_with(f.catalog.folder(&product)));
        // The originals stay where they were.
        assert!(photo.exists() && note.exists());
    });
}

#[test]
fn a_name_already_in_the_folder_gets_a_number_ignoring_case() {
    block_on(async {
        let f = Fixture::new().await;
        let product = product(&f).await;
        let first = outside(&f, "foto.jpg", b"1");
        f.catalog.add_files(product.id, &[first]).await.unwrap();
        let shouting = f.dir.path().join("other");
        std::fs::create_dir_all(&shouting).unwrap();
        std::fs::write(shouting.join("FOTO.jpg"), b"2").unwrap();
        let again = outside(&f, "foto.jpg", b"3");

        let added = f
            .catalog
            .add_files(product.id, &[shouting.join("FOTO.jpg"), again])
            .await
            .unwrap();

        let names: Vec<_> = added.copied.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names, ["FOTO (2).jpg", "foto (3).jpg"]);
        assert_eq!(f.catalog.files(product.id).await.unwrap().len(), 3);
    });
}

#[test]
fn folders_missing_paths_and_files_already_there_are_reported_not_copied() {
    block_on(async {
        let f = Fixture::new().await;
        let product = product(&f).await;
        let inside = f.catalog.folder(&product).join("ja-esta.png");
        std::fs::write(&inside, b"png").unwrap();
        let folder = f.dir.path().join("uma-pasta");
        std::fs::create_dir_all(&folder).unwrap();
        let missing = f.dir.path().join("sumiu.txt");
        let good = outside(&f, "ok.txt", b"ok");

        let added = f
            .catalog
            .add_files(
                product.id,
                &[inside.clone(), folder.clone(), missing.clone(), good],
            )
            .await
            .unwrap();

        assert_eq!(added.copied.len(), 1);
        assert_eq!(added.not_copied[0], (inside, NotCopied::AlreadyThere));
        assert_eq!(added.not_copied[1], (folder, NotCopied::Folder));
        assert_eq!(added.not_copied[2].0, missing);
        assert!(matches!(added.not_copied[2].1, NotCopied::Failed(_)));
        assert_eq!(f.catalog.files(product.id).await.unwrap().len(), 2);
    });
}

#[test]
fn files_put_in_the_folder_by_hand_show_up_and_system_clutter_does_not() {
    block_on(async {
        let f = Fixture::new().await;
        let product = product(&f).await;
        let folder = f.catalog.folder(&product);
        std::fs::write(folder.join("print.png"), b"png").unwrap();
        std::fs::write(folder.join("Thumbs.db"), b"x").unwrap();
        std::fs::write(folder.join("desktop.ini"), b"x").unwrap();
        std::fs::write(folder.join(".partial-copy"), b"x").unwrap();
        std::fs::create_dir_all(folder.join("subpasta")).unwrap();

        let names: Vec<_> = f
            .catalog
            .files(product.id)
            .await
            .unwrap()
            .into_iter()
            .map(|file| file.name)
            .collect();

        assert_eq!(names, ["print.png"]);
    });
}

#[test]
fn a_folder_deleted_by_hand_comes_back_empty() {
    block_on(async {
        let f = Fixture::new().await;
        let product = product(&f).await;
        std::fs::remove_dir_all(f.catalog.folder(&product)).unwrap();

        assert!(f.catalog.files(product.id).await.unwrap().is_empty());
        assert!(f.catalog.folder(&product).is_dir());
        let added = f
            .catalog
            .add_files(product.id, &[outside(&f, "a.txt", b"a")])
            .await
            .unwrap();
        assert_eq!(added.copied.len(), 1);
    });
}

#[test]
fn files_of_an_unknown_product_are_an_error() {
    block_on(async {
        let f = Fixture::new().await;
        let unknown = mascate_kernel::RecordId::from_u128(999);

        assert!(matches!(
            f.catalog.files(unknown).await,
            Err(CatalogError::UnknownProduct(_))
        ));
        assert!(matches!(
            f.catalog.add_files(unknown, &[]).await,
            Err(CatalogError::UnknownProduct(_))
        ));
    });
}
