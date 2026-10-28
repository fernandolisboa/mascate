//! What the app's tests share: a store that sells on Mercado Livre and on
//! Shopee and promotes Hotmart and Shopee products, on a real temp
//! database with an injected clock and ID generator.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use mascate_affiliate::{Affiliate, AffiliatePlatform, CommissionStatus, ManualCommission};
use mascate_commerce::{
    ChannelOrder, ChannelOrderLine, ChannelOrders, FulfillmentMode, LineStock, OnOrderSale,
    OnSalesChannel, Order, OrderLine, OrderStatus, Orders, Shipment, ShipmentStatus, StockShort,
    UnitsBack,
};
use mascate_inventory::Inventory;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{
    Clock, Currency, IdGenerator, ListingType, Money, PlatformError, RecordId, SalesChannelKind,
    Timestamp,
};
use mascate_platform::{Database, migrate};
use rust_decimal::Decimal;
use tempfile::TempDir;

use crate::startup::MODULE_MIGRATIONS;

/// The clock of the store: every sale happened the day before.
pub fn now() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap()
}

pub fn brl(amount: i64) -> Money {
    Money::new(Decimal::from(amount), Currency::Brl)
}

/// What the Mercado Livre Order and the Shopee Order sold for.
pub const MERCADO_LIVRE_SOLD: i64 = 180;
pub const SHOPEE_SOLD: i64 = 100;

/// The approved Hotmart and Shopee Commissions.
pub const HOTMART_EARNED: i64 = 30;
pub const SHOPEE_EARNED: i64 = 10;

/// One Sales Channel reporting one Order, late to dispatch.
struct OneOrder {
    kind: SalesChannelKind,
    order: ChannelOrder,
}

impl OnSalesChannel for OneOrder {
    fn sales_channel(&self) -> SalesChannelKind {
        self.kind
    }
}

impl ChannelOrders for OneOrder {
    fn orders_changed_since(&self, _: Timestamp) -> Result<Vec<ChannelOrder>, PlatformError> {
        Ok(vec![self.order.clone()])
    }
}

/// One unit sold a day ago for R$ `total`, paid and late to dispatch.
pub fn late_order(id: &str, title: &str, total: i64) -> ChannelOrder {
    let sold_at = now() - TimeDelta::days(1);
    ChannelOrder {
        id: id.into(),
        pack: None,
        status: OrderStatus::Paid,
        ordered_at: sold_at,
        updated_at: sold_at,
        lines: vec![ChannelOrderLine {
            item: format!("ITEM-{id}"),
            variation: None,
            title: title.into(),
            variation_name: None,
            quantity: 1,
            unit_price: brl(total),
            sale_fee: None,
            listing_type: Some(ListingType::Classic),
        }],
        total: brl(total),
        paid: Some(brl(total)),
        shipping_paid: None,
        refunded: None,
        buyer: None,
        shipment: Some(Shipment {
            id: format!("SHIP-{id}"),
            status: ShipmentStatus::ReadyToShip,
            dispatch_by: Some(now() - TimeDelta::hours(1)),
            seller_cost: None,
        }),
        returns: Vec::new(),
    }
}

/// The Order `id`, paid and late to dispatch, with one line sold on an
/// On-Order Listing still to buy.
pub fn sold_on_order(id: u128) -> OnOrderSale {
    let sold = late_order(&id.to_string(), "Fone Bluetooth", 90);
    let line = OrderLine {
        id: RecordId::from_u128(id + 1),
        sold: sold.lines[0].clone(),
        stock: LineStock::Short(StockShort::OnOrder),
        back: UnitsBack::default(),
    };
    OnOrderSale {
        order: Order {
            id: RecordId::from_u128(id),
            channel: SalesChannelKind::MercadoLivre,
            lines: vec![line.clone()],
            sold,
            fulfillment_mode: FulfillmentMode::OwnStock,
            buyer_data_erased_at: None,
            synced_at: now(),
        },
        line,
        product: RecordId::from_u128(99),
        missing: 1,
        purchase: None,
    }
}

pub struct Store {
    dir: TempDir,
    pub database: Arc<Database>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGenerator>,
    pub inventory: Arc<Inventory>,
    pub orders: Arc<Orders>,
    pub affiliate: Arc<Affiliate>,
}

impl Store {
    /// A Mercado Livre Order and a Shopee Order, both late to dispatch, and
    /// an approved Commission on Hotmart and one on Shopee.
    pub async fn selling_and_promoting() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        let clock = ManualClock::at(now());
        migrate(&database, &clock, MODULE_MIGRATIONS).await.unwrap();
        let clock: Arc<dyn Clock> = Arc::new(clock);
        let ids: Arc<dyn IdGenerator> = Arc::new(SequentialIds::default());
        let inventory = Arc::new(Inventory::new(database.clone(), clock.clone(), ids.clone()));
        let orders = Arc::new(Orders::new(
            database.clone(),
            inventory.clone(),
            clock.clone(),
            ids.clone(),
        ));
        for channel in [
            OneOrder {
                kind: SalesChannelKind::MercadoLivre,
                order: late_order("2000001", "Fone Bluetooth", MERCADO_LIVRE_SOLD),
            },
            OneOrder {
                kind: SalesChannelKind::Shopee,
                order: late_order("260101ABC", "Capa de celular", SHOPEE_SOLD),
            },
        ] {
            orders.sync(&channel).await.unwrap();
        }
        let affiliate = Arc::new(Affiliate::new(database.clone(), clock.clone(), ids.clone()));
        for (platform, product, amount) in [
            (
                AffiliatePlatform::Hotmart,
                "Curso de Fotografia",
                HOTMART_EARNED,
            ),
            (AffiliatePlatform::Shopee, "Capa de celular", SHOPEE_EARNED),
        ] {
            affiliate
                .record_manual(ManualCommission {
                    platform,
                    program: String::new(),
                    product: product.into(),
                    sold_at: now() - TimeDelta::days(1),
                    amount: brl(amount),
                    status: CommissionStatus::Approved,
                })
                .await
                .unwrap();
        }
        Self {
            dir,
            database,
            clock,
            ids,
            inventory,
            orders,
            affiliate,
        }
    }

    /// Where the Catalog keeps its files.
    pub fn files(&self) -> PathBuf {
        self.dir.path().join("files")
    }
}
