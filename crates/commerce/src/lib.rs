//! Commerce: Listings and their drafts, prices and Promotions, Orders with their Fees and
//! Money Releases, Purchase Orders and shipping on each Sales Channel, and
//! the order the owner ranks the channels in.

mod channels;
mod drafts;
mod fees;
mod freight;
mod listings;
mod merge;
mod orders;
mod pricing;
mod promotions;
mod purchase_orders;
mod releases;
mod shipments;
mod stock_mirror;

pub use channels::{ChannelRanking, OnSalesChannel};
pub use drafts::{
    AttributeValue, CatalogItem, CatalogMatch, CategoryAttribute, CategoryPrediction,
    ChannelCategory, ChannelIssue, Check, ChecklistItem, Condition, DraftAttribute, DraftEdit,
    DraftKind, DraftPicture, DraftStart, Gtin, ListingDraft, ListingPublisher, ListingToPublish,
    MAX_PICTURE_BYTES, MIN_PICTURES, PublishedListing, Requirement, is_blocked, is_picture,
};
pub use fees::{
    AdCost, BilledOrder, ChannelBilling, ChannelFee, Fee, FeeImport, FeeKind, LineMargin,
    RealizedMargin, Sale, SalesPeriod, SalesSummary,
};
pub use freight::{freight_for_units, split_by_value};
pub use listings::{
    CatalogProduct, ChannelListing, ChannelStock, LinkSuggestion, Listing, ListingError,
    ListingStatus, ListingSync, ListingToLink, Listings, SalesChannel, SuggestedBy, Variation,
};
pub use merge::MERGE;
pub use orders::{
    Buyer, ChannelOrder, ChannelOrderLine, ChannelOrders, ChannelReturn, DispatchAlert,
    DispatchDue, FulfillmentMode, LineStock, Order, OrderError, OrderLine, OrderSettings,
    OrderStatus, OrderSync, Orders, Receiver, ReturnReceipt, ReturnReceived, ReturnStatus,
    ReturnedItem, Shipment, ShipmentStatus, ShippingLabel, ShippingLabels, SoldWithoutStock,
    StockShort, UnitsBack,
};
pub use pricing::{
    CostSource, DraftPrice, FeeBasis, MarginCheck, PriceAssumptions, PriceBreakdown, PriceScenario,
    PriceSuggestion, Pricing, PricingError, SaleFee, TargetMargin,
};
pub use promotions::{
    ChannelOffer, ChannelPromotion, ChannelPromotions, ConfirmedPromotion, CouponDiscount,
    CouponTerms, DiscountPlan, Offer, OfferToJoin, PROMOTIONS_REFRESH_MINUTES, Promotion,
    PromotionCheck, PromotionError, PromotionKind, PromotionPlan, PromotionRequest,
    PromotionStatus, PromotionSync, Promotions,
};
pub use purchase_orders::{
    NewPurchaseLine, NewPurchaseOrder, PurchaseLine, PurchaseOrder, PurchaseOrderError,
    PurchaseOrderStatus, PurchaseOrders, Receipt, ReceivedLine, Receiving,
};
pub use releases::{ChannelPayments, PaymentRelease, ReleaseImport};
pub use shipments::{ChannelShipments, SentShipment};
pub use stock_mirror::{MirroredStock, StockMirror, StockNotSent, StockSend, StockSent};

use mascate_platform::{ModuleMigrations, Reminder, ReminderShows, ReminderTopic};

pub const BUYER_PERSONAL_DATA: Reminder = Reminder {
    key: "commerce.buyer_personal_data",
    topic: ReminderTopic::Legal,
    title: "Dados de compradores (LGPD)",
    text: "Nome, endereço e telefone de quem compra são dados pessoais. Use-os só para envio \
           e suporte; o app guarda apenas o necessário para isso e apaga depois do prazo \
           escolhido em Configurações › Pedidos.",
    reappears_after_days: 90,
    shows: ReminderShows::Always,
};

/// Raised while the owner has a sale on Amazon (#127): the invoice is a
/// fiscal matter that never holds an Order back.
pub const AMAZON_SALE_INVOICE: Reminder = Reminder {
    key: "commerce.amazon_sale_invoice",
    topic: ReminderTopic::Fiscal,
    title: "NF-e das vendas na Amazon",
    text: "Quem vende com CNPJ emite a NF-e de cada pedido da Amazon antes do envio e informa \
           a nota no Seller Central. Sem CNPJ, o app segue com os pedidos, a baixa no estoque e \
           a confirmação do envio normalmente; ao abrir o CNPJ, passe a emitir as notas.",
    reappears_after_days: 30,
    shows: ReminderShows::WhenRaised,
};

/// This module's Reminders.
pub const REMINDERS: &[Reminder] = &[BUYER_PERSONAL_DATA, AMAZON_SALE_INVOICE];

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
        listings::ADD_CATEGORY_NAMES,
        releases::CREATE_MONEY_RELEASES,
        channels::ADD_SALES_CHANNELS,
    ],
};
