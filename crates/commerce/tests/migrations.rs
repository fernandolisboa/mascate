//! A database from before Sales Channels (Commerce v11) moved to this
//! version: every Listing, category, Order, Fee, Promotion and Sync cursor
//! it had stays as it was, on Mercado Livre (ADR 0033).

mod common;

use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{
    ChannelOffer, ChannelPromotion, FeeKind, LineStock, ListingStatus, Listings, OrderStatus,
    Orders, PromotionKind, PromotionStatus, Promotions, StockShort,
};
use mascate_kernel::SalesChannelKind;
use mascate_kernel::Timestamp;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::{Database, ModuleMigrations, migrate};

use common::{brl, percent};

/// The rows a Commerce v11 database held, in that version's columns.
const BEFORE_SALES_CHANNELS: &str = "
    INSERT INTO commerce_listings
        (id, ml_item_id, title, price, currency, available_quantity, status, category_id,
         synced_at, created_at, updated_at)
        VALUES ('00000000-0000-7000-8000-000000000001', 'MLB1', 'Fone Bluetooth', '89.90',
                'BRL', 5, 'active', 'MLB3697', '2026-10-01T12:00:00+00:00',
                '2026-10-01T12:00:00+00:00', '2026-10-01T12:00:00+00:00');
    INSERT INTO commerce_channel_categories (id, ml_category_id, name, created_at, updated_at)
        VALUES ('00000000-0000-7000-8000-000000000002', 'MLB3697', 'Fones de Ouvido',
                '2026-10-01T12:00:00+00:00', '2026-10-01T12:00:00+00:00');
    INSERT INTO commerce_orders
        (id, ml_order_id, status, fulfillment_mode, ordered_at, channel_updated_at, total,
         currency, synced_at, created_at, updated_at)
        VALUES ('00000000-0000-7000-8000-000000000003', '2000001', 'paid', 'own_stock', '2026-10-02T12:00:00+00:00',
                '2026-10-02T12:00:00+00:00', '89.90', 'BRL', '2026-10-02T12:05:00+00:00',
                '2026-10-02T12:05:00+00:00', '2026-10-02T12:05:00+00:00');
    INSERT INTO commerce_order_lines
        (id, order_id, position, ml_item_id, title, quantity, unit_price, currency, stock_short,
         created_at, updated_at)
        VALUES ('00000000-0000-7000-8000-000000000004', '00000000-0000-7000-8000-000000000003', 0, 'MLB1', 'Fone Bluetooth', 1, '89.90', 'BRL', 'no_listing',
                '2026-10-02T12:05:00+00:00', '2026-10-02T12:05:00+00:00');
    INSERT INTO commerce_order_fees
        (id, order_id, ml_charge_id, kind, description, amount, currency, created_at,
         updated_at)
        VALUES ('00000000-0000-7000-8000-000000000005', '00000000-0000-7000-8000-000000000003', 'CH1', 'sale_fee', 'Tarifa de venda', '12.59', 'BRL',
                '2026-10-03T12:00:00+00:00', '2026-10-03T12:00:00+00:00');
    INSERT INTO commerce_order_syncs (id, first_synced_at, synced_through, created_at, updated_at)
        VALUES ('00000000-0000-7000-8000-000000000006', '2026-10-01T12:00:00+00:00', '2026-10-02T12:05:00+00:00',
                '2026-10-01T12:00:00+00:00', '2026-10-02T12:05:00+00:00');
    INSERT INTO commerce_promotions
        (id, ml_promotion_id, kind, name, starts_on, ends_on, status, synced_at, created_at,
         updated_at)
        VALUES ('00000000-0000-7000-8000-000000000007', 'P1', 'seller_campaign', 'Semana do fone', '2026-10-01', '2026-10-31',
                'started', '2026-10-02T13:00:00+00:00', '2026-10-02T13:00:00+00:00',
                '2026-10-02T13:00:00+00:00');
    INSERT INTO commerce_promotion_offers
        (id, ml_item_id, kind, ml_promotion_id, name, status, price, currency, starts_on,
         ends_on, synced_at, created_at, updated_at)
        VALUES ('00000000-0000-7000-8000-000000000008', 'MLB1', 'seller_campaign', 'P1', 'Semana do fone', 'started', '79.90',
                'BRL', '2026-10-01', '2026-10-31', '2026-10-02T13:00:00+00:00',
                '2026-10-02T13:00:00+00:00', '2026-10-02T13:00:00+00:00');
    INSERT INTO commerce_promotion_syncs (id, synced_at, created_at, updated_at)
        VALUES ('00000000-0000-7000-8000-000000000009', '2026-10-02T13:00:00+00:00', '2026-10-02T13:00:00+00:00',
                '2026-10-02T13:00:00+00:00');";

fn at(text: &str) -> Timestamp {
    Timestamp::from(chrono::DateTime::parse_from_rfc3339(text).unwrap())
}

fn day(month_day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, month_day).unwrap()
}

#[test]
fn every_row_from_before_sales_channels_stays_as_it_was_on_mercado_livre() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap(),
        ));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        let migrations = mascate_commerce::MIGRATIONS.migrations;
        let before = migrations
            .iter()
            .position(|migration| migration.version == 12)
            .unwrap();
        let v11 = ModuleMigrations {
            module: "commerce",
            migrations: &migrations[..before],
        };
        migrate(
            &database,
            clock.as_ref(),
            &[mascate_inventory::MIGRATIONS, v11],
        )
        .await
        .unwrap();
        database
            .connection()
            .execute_batch(BEFORE_SALES_CHANNELS)
            .await
            .unwrap();

        migrate(
            &database,
            clock.as_ref(),
            &[mascate_inventory::MIGRATIONS, mascate_commerce::MIGRATIONS],
        )
        .await
        .unwrap();

        let ids = Arc::new(SequentialIds::default());
        let inventory = Arc::new(mascate_inventory::Inventory::new(
            database.clone(),
            clock.clone(),
            ids.clone(),
        ));
        let listings = Listings::new(database.clone(), clock.clone(), ids.clone());
        let kept = listings.listings().await.unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].channel, SalesChannelKind::MercadoLivre);
        assert_eq!(kept[0].listed.id, "MLB1");
        assert_eq!(kept[0].listed.price, brl("89.90"));
        assert_eq!(kept[0].listed.status, ListingStatus::Active);
        assert_eq!(
            listings
                .category_names(SalesChannelKind::MercadoLivre)
                .await
                .unwrap()["MLB3697"],
            "Fones de Ouvido"
        );
        assert!(
            listings
                .category_names(SalesChannelKind::Amazon)
                .await
                .unwrap()
                .is_empty()
        );

        let orders = Orders::new(
            database.clone(),
            inventory.clone(),
            clock.clone(),
            ids.clone(),
        );
        let sales = orders
            .sales(percent("0"), Timestamp::MIN_UTC..Timestamp::MAX_UTC, &[])
            .await
            .unwrap()
            .sales;
        assert_eq!(sales.len(), 1);
        let sale = &sales[0];
        assert_eq!(sale.order.channel, SalesChannelKind::MercadoLivre);
        assert_eq!(sale.order.sold.id, "2000001");
        assert_eq!(sale.order.sold.status, OrderStatus::Paid);
        assert_eq!(
            sale.order.lines[0].stock,
            LineStock::Short(StockShort::NoListing)
        );
        let fees: Vec<(FeeKind, Option<&str>, _)> = sale
            .margin
            .fees
            .iter()
            .map(|fee| (fee.kind, fee.description.as_deref(), fee.amount))
            .collect();
        assert_eq!(
            fees,
            [(FeeKind::SaleFee, Some("Tarifa de venda"), brl("12.59"))]
        );
        // The Order Sync cursor became Mercado Livre's.
        assert_eq!(
            orders.last_sync().await.unwrap(),
            Some(at("2026-10-02T12:05:00+00:00"))
        );

        let promotions = Promotions::new(database, inventory, clock.clone(), ids);
        let kept = promotions.promotions().await.unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].channel, SalesChannelKind::MercadoLivre);
        assert_eq!(
            kept[0].listed,
            ChannelPromotion {
                id: "P1".into(),
                kind: PromotionKind::SellerCampaign,
                name: "Semana do fone".into(),
                starts: day(1),
                ends: day(31),
                status: PromotionStatus::Started,
                coupon: None,
            }
        );
        let offers = promotions.offers().await.unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].channel, SalesChannelKind::MercadoLivre);
        assert_eq!(
            offers[0].listed,
            ChannelOffer {
                listing: "MLB1".into(),
                kind: PromotionKind::SellerCampaign,
                promotion: Some("P1".into()),
                name: "Semana do fone".into(),
                status: PromotionStatus::Started,
                price: Some(brl("79.90")),
                starts: Some(day(1)),
                ends: Some(day(31)),
            }
        );
        assert_eq!(
            promotions
                .last_sync(SalesChannelKind::MercadoLivre)
                .await
                .unwrap(),
            Some(at("2026-10-02T13:00:00+00:00"))
        );
        // A channel connected later starts with its own Sync due.
        assert_eq!(
            promotions
                .last_sync(SalesChannelKind::Amazon)
                .await
                .unwrap(),
            None
        );
    });
}
