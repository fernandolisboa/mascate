//! Demand in a Sales Channel, as a Product Source reports it: the best
//! sellers of each category and the competitors of a catalog product. The
//! catalog defines what it needs here; each Platform's adapter implements it
//! (ADR 0013), so the catalog never knows which Platform answers.

use mascate_kernel::{Currency, Money};

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

/// How a listing is exposed in the Sales Channel; its sale fee depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ListingType {
    #[default]
    Classic,
    Premium,
}

impl ListingType {
    pub const ALL: [ListingType; 2] = [ListingType::Classic, ListingType::Premium];

    /// Mercado Livre's id for it.
    pub fn code(self) -> &'static str {
        match self {
            ListingType::Classic => "gold_special",
            ListingType::Premium => "gold_pro",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.code() == code)
    }

    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            ListingType::Classic => "Clássico",
            ListingType::Premium => "Premium",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DemandError {
    #[error("the Connection to the Platform is not set up")]
    NotConnected,
    #[error("the Platform no longer accepts the login")]
    Expired,
    #[error("the Platform asked to wait before asking again")]
    RateLimited,
    #[error("the Platform refused the request: {0}")]
    Refused(String),
    #[error("the Platform does not know it")]
    NotFound,
    #[error("could not talk to the Platform: {0}")]
    Failed(String),
}

/// A Platform that reports demand. Calls block on the network, so they run
/// off the UI thread.
pub trait DemandSource: Send + Sync {
    /// The currency the Sales Channel sells in.
    fn currency(&self) -> Currency;

    /// The categories the owner can follow.
    fn categories(&self) -> Result<Vec<DemandCategory>, DemandError>;

    /// The best sellers of `category`, best first.
    fn best_sellers(&self, category: &str) -> Result<Vec<BestSeller>, DemandError>;

    /// The catalog products that best match `query`, best first.
    fn search(&self, query: &str) -> Result<Vec<CatalogMatch>, DemandError>;

    /// Who sells the catalog product `product`.
    fn competition(&self, product: &str) -> Result<Competition, DemandError>;

    /// The top-level category `category` sits under; itself when it is one.
    fn root_category(&self, category: &str) -> Result<DemandCategory, DemandError>;

    /// What the Sales Channel charges for selling one unit at `price` in
    /// `category` with a listing of type `listing`.
    fn sale_fee(
        &self,
        category: &str,
        price: Money,
        listing: ListingType,
    ) -> Result<Money, DemandError>;
}
