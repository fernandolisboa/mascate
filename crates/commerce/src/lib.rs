//! Commerce: Listings and their drafts, Orders, Purchase Orders and shipping on each Sales Channel.

mod drafts;
mod freight;
mod listings;
mod pricing;
mod purchase_orders;

pub use drafts::{
    AttributeValue, CategoryAttribute, CategoryPrediction, ChannelCategory, ChannelIssue, Check,
    ChecklistItem, Condition, DraftAttribute, DraftEdit, DraftPicture, DraftStart, ListingDraft,
    ListingPublisher, ListingToPublish, MAX_PICTURE_BYTES, MIN_PICTURES, PublishedListing,
    Requirement, is_blocked, is_picture,
};
pub use freight::{freight_for_units, split_by_value};
pub use listings::{
    CatalogProduct, ChannelListing, LinkSuggestion, Listing, ListingError, ListingStatus,
    ListingSync, ListingToLink, Listings, SalesChannel, SuggestedBy, Variation,
};
pub use pricing::{
    CostSource, DraftPrice, PriceAssumptions, PriceBreakdown, PriceScenario, PriceSuggestion,
    Pricing, PricingError, SaleFee, TargetMargin,
};
pub use purchase_orders::{
    NewPurchaseLine, NewPurchaseOrder, PurchaseLine, PurchaseOrder, PurchaseOrderError,
    PurchaseOrderStatus, PurchaseOrders, Receipt, ReceivedLine, Receiving,
};

use mascate_platform::{ModuleMigrations, Reminder, ReminderTopic};

pub const BUYER_PERSONAL_DATA: Reminder = Reminder {
    key: "commerce.buyer_personal_data",
    topic: ReminderTopic::Legal,
    title: "Dados de compradores (LGPD)",
    text: "Nome, endereço e telefone de quem compra são dados pessoais. Use-os só para envio \
           e suporte; o app guarda apenas o necessário para isso.",
    reappears_after_days: 90,
};

/// This module's Reminders.
pub const REMINDERS: &[Reminder] = &[BUYER_PERSONAL_DATA];

/// This module's own tables.
pub const MIGRATIONS: ModuleMigrations = ModuleMigrations {
    module: "commerce",
    migrations: &[
        purchase_orders::CREATE_PURCHASE_ORDERS,
        listings::CREATE_LISTINGS,
        pricing::CREATE_TARGET_MARGINS,
        drafts::CREATE_DRAFTS,
    ],
};
