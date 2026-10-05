//! Supplier Offers as a Platform's catalog reports them: items for sale with
//! their shop, price, sales and commission, and the shops themselves. The
//! catalog defines what it needs here; each Platform's adapter implements
//! it (ADR 0013, ADR 0027), so the catalog never knows which one answers.

use mascate_kernel::{Money, Percentage, PlatformError};
use rust_decimal::Decimal;

use crate::ProductSource;

/// How the found offers come sorted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OfferOrder {
    Relevance,
    #[default]
    Sales,
    LowestPrice,
    HighestCommission,
}

impl OfferOrder {
    pub const ALL: [OfferOrder; 4] = [
        OfferOrder::Sales,
        OfferOrder::Relevance,
        OfferOrder::LowestPrice,
        OfferOrder::HighestCommission,
    ];
}

/// What the owner searches for. Empty fields filter nothing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OfferSearch {
    pub keyword: String,
    /// The Platform's category, as its id or a link to the category page.
    pub category: Option<String>,
    /// Only this shop's items, by the Platform's shop id.
    pub shop: Option<String>,
    pub order: OfferOrder,
    /// 1 for the first page.
    pub page: u32,
}

/// A shop selling on the Platform: a Supplier once one of its offers is
/// kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundShop {
    /// The Platform's id of the shop.
    pub id: String,
    pub name: String,
    pub link: String,
    /// What the Platform pays for a sale through the owner's affiliate link.
    pub commission: Option<Percentage>,
    /// Stars from 0 to 5, when the Platform says.
    pub rating: Option<Decimal>,
}

/// An item for sale on the Platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundOffer {
    /// The Platform's id of the item: its natural key, the same on every
    /// search.
    pub id: String,
    pub title: String,
    /// The item's page.
    pub link: String,
    pub shop: FoundShop,
    /// The lowest price of the item, its cheapest variation.
    pub price: Money,
    /// The highest price, the dearest variation; the same as `price`
    /// without variations.
    pub highest_price: Money,
    /// Units sold, as the Platform counts them.
    pub sales: u32,
    pub commission: Option<Percentage>,
    pub rating: Option<Decimal>,
}

/// One page of what a search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found<T> {
    pub items: Vec<T>,
    /// Whether the next page has more.
    pub more: bool,
}

/// A Platform whose catalog the owner searches for offers. Calls block on
/// the network, so they run off the UI thread.
pub trait OfferSource: Send + Sync {
    /// Which Product Source the kept offers count as.
    fn source(&self) -> ProductSource;

    /// The items that match `search`.
    fn search_offers(&self, search: &OfferSearch) -> Result<Found<FoundOffer>, PlatformError>;

    /// The shops whose name matches `keyword`, on page `page` (1 first).
    fn search_shops(&self, keyword: &str, page: u32) -> Result<Found<FoundShop>, PlatformError>;
}
