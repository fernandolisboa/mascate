//! Demand in a Sales Channel, as a Product Source reports it: the best
//! sellers of each category and the competitors of a catalog product. The
//! catalog defines what it needs here; each Platform's adapter implements it
//! (ADR 0013), so the catalog never knows which Platform answers.

use mascate_kernel::{Currency, ListingType, Money, PlatformError};

/// A category of the Sales Channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DemandCategory {
    pub id: String,
    pub name: String,
}

/// One of the best sellers of a category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BestSeller {
    /// The Platform's id of the entry: a catalog product or a single listing.
    pub id: String,
    /// 1 for the best seller.
    pub position: u32,
    pub title: String,
    /// The catalog product the entry is, or belongs to, when it has one.
    pub catalog_product: Option<String>,
    /// What it sells for, when the Platform says.
    pub price: Option<Money>,
}

/// A catalog product found for a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogMatch {
    pub id: String,
    pub name: String,
}

/// Who else sells a catalog product, and for how much.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Competition {
    /// How many listings sell it.
    pub sellers: u32,
    /// The prices of the listings the Platform returned.
    pub prices: Vec<Money>,
    /// The category those listings are in.
    pub category: Option<String>,
}

/// A Platform that reports demand. Calls block on the network, so they run
/// off the UI thread.
pub trait DemandSource: Send + Sync {
    /// The currency the Sales Channel sells in.
    fn currency(&self) -> Currency;

    /// The categories the owner can follow.
    fn categories(&self) -> Result<Vec<DemandCategory>, PlatformError>;

    /// The best sellers of `category`, best first.
    fn best_sellers(&self, category: &str) -> Result<Vec<BestSeller>, PlatformError>;

    /// The catalog products that best match `query`, best first.
    fn search(&self, query: &str) -> Result<Vec<CatalogMatch>, PlatformError>;

    /// Who sells the catalog product `product`.
    fn competition(&self, product: &str) -> Result<Competition, PlatformError>;

    /// The top-level category `category` sits under; itself when it is one.
    fn root_category(&self, category: &str) -> Result<DemandCategory, PlatformError>;

    /// What the Sales Channel charges for selling one unit at `price` in
    /// `category` with a listing of type `listing`.
    fn sale_fee(
        &self,
        category: &str,
        price: Money,
        listing: ListingType,
    ) -> Result<Money, PlatformError>;
}
