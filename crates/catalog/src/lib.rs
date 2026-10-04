//! Catalog & Discovery: Products, Supplier Offers and ranked Opportunities.

mod catalog;
mod files;
mod link;
mod sku;

pub use catalog::{
    Catalog, CatalogError, NewSupplierOffer, OfferHistory, Product, ProductSource, Supplier,
    SupplierOffer,
};
pub use files::{AddedFiles, NotCopied, ProductFile};
pub use link::{InvalidLink, OfferLink};
pub use sku::{InvalidSku, Sku};

use mascate_platform::ModuleMigrations;

/// This module's own tables.
pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "catalog",
    migrations: &[catalog::CREATE_CATALOG],
};
