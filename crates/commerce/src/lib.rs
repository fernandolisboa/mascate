//! Commerce: Listings and their drafts, On-Order Listings, prices, their
//! history and Automatic Repricing, Promotions, Orders with their Fees and
//! Money Releases, Purchase Orders and shipping on each Sales Channel, and
//! the order the owner ranks the channels in.

mod channels;
mod drafts;
mod fees;
mod freight;
mod listings;
mod merge;
mod on_order;
mod orders;
mod pricing;
mod promotions;
mod purchase_orders;
mod releases;
mod repricing;
mod shipments;
mod stock_mirror;

pub use channels::{ChannelRanking, OnSalesChannel};
pub use drafts::{
    AttributeValue, CatalogItem, CatalogMatch, CategoryAttribute, CategoryPrediction,
    ChannelCategory, ChannelIssue, Check, ChecklistItem, Condition, DraftAttribute, DraftEdit,
    DraftKind, DraftPicture, DraftStart, DraftVariation, Gtin, ListingDraft, ListingPublisher,
    ListingToPublish, MAX_PICTURE_BYTES, MAX_VARIATIONS, MIN_PICTURES, PublishedListing,
    PublishedVariation, Requirement, VariationToPublish, VariationsToPublish, is_blocked,
    is_picture, takes_variations,
};
pub use fees::{
    AdCost, BilledOrder, ChannelBilling, ChannelFee, Fee, FeeImport, FeeKind, LineMargin,
    RealizedMargin, Sale, SalesPeriod, SalesSummary,
};
pub use freight::{freight_for_units, split_by_value};
pub use listings::{
    CatalogProduct, ChannelListing, ChannelStock, LinkSuggestion, Listing, ListingError,
    ListingStatus, ListingSync, ListingToLink, Listings, PRICE_APPLY_WINDOW, SalesChannel,
    SentPrice, SentPriceState, SuggestedBy, Variation,
};
pub use merge::MERGE;
pub use on_order::{
    ListingAvailability, MAX_AVAILABILITY_DAYS, OnOrderListing, OnOrderSale, OnOrderStatusChange,
    OnOrderTerms,
};
pub use orders::{
    Buyer, ChannelOrder, ChannelOrderLine, ChannelOrders, ChannelReturn, DispatchAlert,
    DispatchDue, FulfillmentMode, LabelError, LineStock, Order, OrderError, OrderLine,
    OrderSettings, OrderStatus, OrderSync, Orders, Receiver, ReturnReceipt, ReturnReceived,
    ReturnStatus, ReturnedItem, Shipment, ShipmentStatus, ShippingLabel, ShippingLabels,
    SoldOnOrder, SoldWithoutStock, StockShort, UnitsBack,
};
pub use pricing::{
    CostSource, DraftPrice, FeeBasis, MarginCheck, PriceAssumptions, PriceBreakdown, PriceScenario,
    PriceSuggestion, Pricing, PricingError, SaleFee, TargetMargin,
};
pub use promotions::{
    ChannelOffer, ChannelPromotion, ChannelPromotions, ConfirmedPromotion, CouponDiscount,
    CouponTerms, DiscountPlan, Offer, OfferToJoin, PROMOTIONS_REFRESH_MINUTES, Promotion,
    PromotionCheck, PromotionError, PromotionKind, PromotionPlan, PromotionRequest,
    PromotionStatus, PromotionSync, Promotions, promotion_kinds,
};
pub use purchase_orders::{
    NewPurchaseLine, NewPurchaseOrder, PurchaseLine, PurchaseOrder, PurchaseOrderError,
    PurchaseOrderStatus, PurchaseOrders, Receipt, ReceivedLine, Receiving,
};
pub use releases::{ChannelPayments, PaymentRelease, ReleaseImport};
pub use repricing::{
    AUTOMATIC_REPRICING, ChannelAutomation, ChannelPriceAutomation, ConfirmedRepricing,
    FeaturedOffer, FeaturedOfferPrices, PriceChange, PriceChangeReason, RepricedListing, Repricing,
    RepricingError, RepricingLimits, RepricingPort, RepricingReading, RepricingRequest,
    RepricingRound, RepricingStatus, RepricingWay,
};
pub use shipments::{ChannelShipments, SentShipment};
pub use stock_mirror::{
    MirroredStock, OrderSlots, StockMirror, StockNotSent, StockSend, StockSent,
};

use mascate_platform::{Flag, ModuleMigrations, Reminder, ReminderShows, ReminderTopic};

pub const BUYER_PERSONAL_DATA: Reminder = Reminder {
    key: "commerce.buyer_personal_data",
    topic: ReminderTopic::Legal,
    title: "Dados de compradores (LGPD)",
    text: "Nome, endereço e telefone de quem compra são dados pessoais. Use-os só para envio \
           e suporte; o app guarda apenas o necessário para isso e apaga depois do prazo \
           escolhido em Configurações › Vendas › Pedidos.",
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

/// Raised while a Shopee Order waits for its NF-e (#130): Shopee asks for
/// it before an Order outside the Correios ships, and the app never holds
/// the Order or the stock back for it.
pub const SHOPEE_SALE_INVOICE: Reminder = Reminder {
    key: "commerce.shopee_sale_invoice",
    topic: ReminderTopic::Fiscal,
    title: "NF-e dos pedidos da Shopee",
    text: "A Shopee pede a NF-e antes de liberar o envio de um pedido fora dos Correios, e o \
           pedido aparece em Pedidos como \"Aguardando NF-e\". Quem vende com CNPJ emite a \
           nota e a envia na Central do Vendedor (seller.shopee.com.br); a etiqueta sai em \
           Pedidos logo depois. O app segue com a baixa no estoque e o prazo de envio \
           normalmente.",
    reappears_after_days: 7,
    shows: ReminderShows::WhenRaised,
};

/// Raised while a Magalu Order waits for its NF-e (#134): Magalu ships a
/// delivery only after the store sends its NF-e, and the app never holds
/// the Order or the stock back for it.
pub const MAGALU_SALE_INVOICE: Reminder = Reminder {
    key: "commerce.magalu_sale_invoice",
    topic: ReminderTopic::Fiscal,
    title: "NF-e dos pedidos da Magalu",
    text: "A Magalu só libera o envio de um pedido depois que a loja envia a NF-e da entrega, e \
           o pedido aparece em Pedidos como \"Aguardando NF-e\". Quem vende com CNPJ emite a \
           nota e a informa no Portal do Seller da Magalu (seller.magalu.com). O app segue com \
           a baixa no estoque e o prazo de envio normalmente.",
    reappears_after_days: 7,
    shows: ReminderShows::WhenRaised,
};

/// This module's Reminders.
pub const REMINDERS: &[Reminder] = &[
    BUYER_PERSONAL_DATA,
    AMAZON_SALE_INVOICE,
    SHOPEE_SALE_INVOICE,
    MAGALU_SALE_INVOICE,
];

/// This module's flags.
pub const FLAGS: &[Flag] = &[AUTOMATIC_REPRICING];

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
        drafts::ADD_DRAFT_VARIATIONS,
        listings::ADD_SENT_PRICES,
        repricing::CREATE_PRICE_HISTORY,
        on_order::CREATE_ON_ORDER_LISTINGS,
        purchase_orders::LINK_TO_ORDER_LINES,
    ],
};
