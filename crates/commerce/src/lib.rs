//! Commerce: Listings and their drafts, prices and Promotions, Orders, Purchase Orders and
//! shipping on each Sales Channel.

mod drafts;
mod fees;
mod freight;
mod listings;
mod orders;
mod pricing;
mod promotions;
mod purchase_orders;
mod stock_mirror;

pub use drafts::{
    AttributeValue, CategoryAttribute, CategoryPrediction, ChannelCategory, ChannelIssue, Check,
    ChecklistItem, Condition, DraftAttribute, DraftEdit, DraftPicture, DraftStart, ListingDraft,
    ListingPublisher, ListingToPublish, MAX_PICTURE_BYTES, MIN_PICTURES, PublishedListing,
    Requirement, is_blocked, is_picture,
};
pub use fees::{
    BilledOrder, ChannelBilling, ChannelFee, Fee, FeeImport, FeeKind, RealizedMargin, Sale,
    SalesSummary,
};
pub use freight::{freight_for_units, split_by_value};
pub use listings::{
    CatalogProduct, ChannelListing, ChannelStock, LinkSuggestion, Listing, ListingError,
    ListingStatus, ListingSync, ListingToLink, Listings, SalesChannel, SuggestedBy, Variation,
};
pub use orders::{
    Buyer, ChannelOrder, ChannelOrderLine, ChannelOrders, ChannelReturn, DispatchAlert,
    DispatchDue, FulfillmentMode, LineStock, Order, OrderError, OrderLine, OrderSettings,
    OrderStatus, OrderSync, Orders, Receiver, ReturnReceipt, ReturnReceived, ReturnStatus,
    ReturnedItem, Shipment, ShipmentStatus, ShippingLabel, ShippingLabels, StockShort, UnitsBack,
};
pub use pricing::{
    CostSource, DraftPrice, MarginCheck, PriceAssumptions, PriceBreakdown, PriceScenario,
    PriceSuggestion, Pricing, PricingError, SaleFee, TargetMargin,
};
pub use promotions::{
    ChannelOffer, ChannelPromotion, ChannelPromotions, ConfirmedPromotion, CouponDiscount,
    CouponTerms, DiscountPlan, Offer, OfferToJoin, PROMOTIONS_REFRESH_MINUTES, Promotion,
    PromotionCheck, PromotionError, PromotionKind, PromotionPlan, PromotionRequest,
    PromotionStatus, PromotionSync, Promotions, channel_day,
};
pub use purchase_orders::{
    NewPurchaseLine, NewPurchaseOrder, PurchaseLine, PurchaseOrder, PurchaseOrderError,
    PurchaseOrderStatus, PurchaseOrders, Receipt, ReceivedLine, Receiving,
};
pub use stock_mirror::{MirroredStock, StockMirror, StockNotSent, StockSend, StockSent};

use mascate_platform::{ModuleMigrations, Reminder, ReminderTopic};

pub const BUYER_PERSONAL_DATA: Reminder = Reminder {
    key: "commerce.buyer_personal_data",
    topic: ReminderTopic::Legal,
    title: "Dados de compradores (LGPD)",
    text: "Nome, endereço e telefone de quem compra são dados pessoais. Use-os só para envio \
           e suporte; o app guarda apenas o necessário para isso e apaga depois do prazo \
           escolhido em Configurações › Pedidos.",
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
        stock_mirror::ADD_STOCK_SENT,
        orders::CREATE_ORDERS,
        orders::ADD_RETURNS,
        fees::ADD_FEES,
        promotions::CREATE_PROMOTIONS,
    ],
};
