//! A Product's files: photos, purchase notes, screenshots, in the folder
//! named after its SKU. The folder is the record: files the owner puts there
//! by hand show up too.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mascate_kernel::RecordId;

use crate::catalog::create_folder;
use crate::{Catalog, CatalogError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductFile {
    pub name: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub modified: Option<SystemTime>,
}

/// Why a dropped path was not copied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotCopied {
    /// Folders are not copied, only files.
    Folder,
    /// It is in the Product's folder already.
    AlreadyThere,
    /// It could not be read or written; the text says why.
    Failed(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddedFiles {
    pub copied: Vec<ProductFile>,
    pub not_copied: Vec<(PathBuf, NotCopied)>,
}

impl Catalog {
    /// The files in the Product's folder, by name, creating the folder if
    /// it went missing.
    pub async fn files(&self, product: RecordId) -> Result<Vec<ProductFile>, CatalogError> {
        let folder = self.folder(&self.product(product).await?);
        create_folder(&folder)?;
        let failed = |source| CatalogError::Folder {
            path: folder.clone(),
            source,
        };
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&folder).map_err(failed)? {
            let entry = entry.map_err(failed)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let metadata = entry.metadata().map_err(failed)?;
            if metadata.is_file() && !is_hidden(&name) {
                files.push(ProductFile {
                    name,
                    path: entry.path(),
                    bytes: metadata.len(),
                    modified: metadata.modified().ok(),
                });
            }
        }
        files.sort_by_key(|file| file.name.to_lowercase());
        Ok(files)
    }

    /// Copies `paths` into the Product's folder, keeping their names; a
    /// name already there gets a number, as `foto (2).jpg`. The originals
    /// stay where they were.
    pub async fn add_files(
        &self,
        product: RecordId,
        paths: &[PathBuf],
    ) -> Result<AddedFiles, CatalogError> {
        let folder = self.folder(&self.product(product).await?);
        create_folder(&folder)?;
        let mut added = AddedFiles::default();
        for path in paths {
            match self.copy_into(&folder, path) {
                Ok(file) => added.copied.push(file),
                Err(reason) => added.not_copied.push((path.clone(), reason)),
            }
        }
        Ok(added)
    }

    fn copy_into(&self, folder: &Path, path: &Path) -> Result<ProductFile, NotCopied> {
        let failed = |error: std::io::Error| NotCopied::Failed(error.to_string());
        let metadata = std::fs::metadata(path).map_err(failed)?;
        if metadata.is_dir() {
            return Err(NotCopied::Folder);
        }
        let source = path.canonicalize().map_err(failed)?;
        if source.parent() == Some(folder.canonicalize().map_err(failed)?.as_path()) {
            return Err(NotCopied::AlreadyThere);
        }
        let name = source
            .file_name()
            .ok_or_else(|| NotCopied::Failed("the path names no file".into()))?;
        // A hidden temporary name, so a copy cut short never shows as a file.
        let partial = folder.join(format!(".{}.partial", self.next_id()));
        std::fs::copy(&source, &partial).map_err(failed)?;
        let target = free_name(folder, Path::new(name));
        if let Err(error) = std::fs::rename(&partial, &target) {
            let _ = std::fs::remove_file(&partial);
            return Err(failed(error));
        }
        Ok(ProductFile {
            name: target
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            bytes: metadata.len(),
            modified: std::fs::metadata(&target).and_then(|m| m.modified()).ok(),
            path: target,
        })
    }
}

/// `name` in `folder`, numbered when taken: `foto.jpg`, `foto (2).jpg`...
/// Compared ignoring case, as Windows does.
fn free_name(folder: &Path, name: &Path) -> PathBuf {
    let stem = name
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = name
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    let taken: Vec<String> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_lowercase())
        .collect();
    (1..)
        .map(|number| match number {
            1 => format!("{stem}{extension}"),
            n => format!("{stem} ({n}){extension}"),
        })
        .find(|candidate| !taken.contains(&candidate.to_lowercase()))
        .map(|candidate| folder.join(candidate))
        .expect("some number is free")
}

/// Dot files, and the files Windows Explorer leaves in folders it shows.
fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
        || name.eq_ignore_ascii_case("desktop.ini")
        || name.eq_ignore_ascii_case("Thumbs.db")
}
