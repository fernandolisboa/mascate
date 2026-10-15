//! Sales Channels (#124, ADR 0033): every row of Commerce says which
//! channel it belongs to, every port's adapter says which channel it
//! speaks for, and the owner ranks the channels, which decides who gets
//! the units of the Stock Share first.

use std::sync::Arc;

use libsql::{Connection, Row, TransactionBehavior, Value, params};
use mascate_kernel::{Clock, IdGenerator, Record, SalesChannelKind};
use mascate_platform::{Database, Migration, StoredRow, StoredValueError, stored};

use crate::ListingError;

/// What every port's adapter tells Commerce: the Sales Channel it speaks
/// for. Commerce writes only that channel's rows with it.
pub trait OnSalesChannel {
    fn sales_channel(&self) -> SalesChannelKind;
}

pub(crate) const ADD_SALES_CHANNELS: Migration = Migration {
    version: 12,
    name: "put every row on its Sales Channel",
    risky: false,
    // Adding a column with a default and renaming columns keep every row as
    // it was. The Promotions' keys are table constraints, which only a new
    // table can change, so their rows are copied over as they are.
    sql: "DROP INDEX commerce_listings_by_ml_id;
    ALTER TABLE commerce_listings ADD COLUMN sales_channel TEXT NOT NULL DEFAULT 'mercado_livre';
    ALTER TABLE commerce_listings RENAME COLUMN ml_item_id TO channel_item_id;
    ALTER TABLE commerce_listings RENAME COLUMN ml_variation_id TO channel_variation_id;
    CREATE UNIQUE INDEX commerce_listings_by_channel_id
        ON commerce_listings (sales_channel, channel_item_id, COALESCE(channel_variation_id, ''))
        WHERE channel_item_id IS NOT NULL AND deleted_at IS NULL;

    DROP INDEX commerce_channel_categories_by_ml_id;
    ALTER TABLE commerce_channel_categories
        ADD COLUMN sales_channel TEXT NOT NULL DEFAULT 'mercado_livre';
    ALTER TABLE commerce_channel_categories
        RENAME COLUMN ml_category_id TO channel_category_id;
    CREATE UNIQUE INDEX commerce_channel_categories_by_channel_id
        ON commerce_channel_categories (sales_channel, channel_category_id)
        WHERE deleted_at IS NULL;

    DROP INDEX commerce_orders_by_ml_id;
    ALTER TABLE commerce_orders ADD COLUMN sales_channel TEXT NOT NULL DEFAULT 'mercado_livre';
    ALTER TABLE commerce_orders RENAME COLUMN ml_order_id TO channel_order_id;
    ALTER TABLE commerce_orders RENAME COLUMN ml_pack_id TO channel_pack_id;
    ALTER TABLE commerce_orders RENAME COLUMN ml_shipment_id TO channel_shipment_id;
    CREATE UNIQUE INDEX commerce_orders_by_channel_id
        ON commerce_orders (sales_channel, channel_order_id) WHERE deleted_at IS NULL;

    DROP INDEX commerce_order_lines_by_item;
    ALTER TABLE commerce_order_lines RENAME COLUMN ml_item_id TO channel_item_id;
    ALTER TABLE commerce_order_lines RENAME COLUMN ml_variation_id TO channel_variation_id;
    CREATE UNIQUE INDEX commerce_order_lines_by_item
        ON commerce_order_lines (order_id, channel_item_id, COALESCE(channel_variation_id, ''))
        WHERE deleted_at IS NULL;

    DROP INDEX commerce_order_returns_by_item;
    ALTER TABLE commerce_order_returns RENAME COLUMN ml_return_id TO channel_return_id;
    ALTER TABLE commerce_order_returns RENAME COLUMN ml_item_id TO channel_item_id;
    ALTER TABLE commerce_order_returns RENAME COLUMN ml_variation_id TO channel_variation_id;
    CREATE UNIQUE INDEX commerce_order_returns_by_item
        ON commerce_order_returns
            (order_id, channel_return_id, channel_item_id, COALESCE(channel_variation_id, ''))
        WHERE deleted_at IS NULL;

    DROP INDEX commerce_order_fees_by_charge;
    ALTER TABLE commerce_order_fees RENAME COLUMN ml_charge_id TO channel_charge_id;
    CREATE UNIQUE INDEX commerce_order_fees_by_charge
        ON commerce_order_fees (order_id, channel_charge_id) WHERE deleted_at IS NULL;

    DROP INDEX commerce_money_releases_by_payment;
    ALTER TABLE commerce_money_releases RENAME COLUMN ml_payment_id TO channel_payment_id;
    CREATE UNIQUE INDEX commerce_money_releases_by_payment
        ON commerce_money_releases (order_id, channel_payment_id) WHERE deleted_at IS NULL;

    ALTER TABLE commerce_order_syncs ADD COLUMN sales_channel TEXT NOT NULL DEFAULT 'mercado_livre';
    UPDATE commerce_order_syncs SET deleted_at = updated_at
        WHERE deleted_at IS NULL AND id <> (SELECT id FROM commerce_order_syncs
                                            WHERE deleted_at IS NULL
                                            ORDER BY updated_at DESC LIMIT 1);
    CREATE UNIQUE INDEX commerce_order_syncs_by_channel
        ON commerce_order_syncs (sales_channel) WHERE deleted_at IS NULL;

    ALTER TABLE commerce_promotion_syncs
        ADD COLUMN sales_channel TEXT NOT NULL DEFAULT 'mercado_livre';
    UPDATE commerce_promotion_syncs SET deleted_at = updated_at
        WHERE deleted_at IS NULL AND id <> (SELECT id FROM commerce_promotion_syncs
                                            WHERE deleted_at IS NULL
                                            ORDER BY updated_at DESC LIMIT 1);
    CREATE UNIQUE INDEX commerce_promotion_syncs_by_channel
        ON commerce_promotion_syncs (sales_channel) WHERE deleted_at IS NULL;

    CREATE TABLE commerce_promotions_by_channel (
        id                   TEXT PRIMARY KEY,
        sales_channel        TEXT NOT NULL,
        channel_promotion_id TEXT NOT NULL,
        kind                 TEXT NOT NULL,
        name                 TEXT NOT NULL,
        starts_on            TEXT NOT NULL,
        ends_on              TEXT NOT NULL,
        status               TEXT NOT NULL,
        coupon_kind          TEXT,
        coupon_value         TEXT,
        coupon_minimum       TEXT,
        coupon_maximum       TEXT,
        coupon_budget        TEXT,
        currency             TEXT,
        synced_at            TEXT NOT NULL,
        created_at           TEXT NOT NULL,
        updated_at           TEXT NOT NULL,
        deleted_at           TEXT,
        UNIQUE (sales_channel, channel_promotion_id)
    );
    INSERT INTO commerce_promotions_by_channel
        (id, sales_channel, channel_promotion_id, kind, name, starts_on, ends_on, status,
         coupon_kind, coupon_value, coupon_minimum, coupon_maximum, coupon_budget, currency,
         synced_at, created_at, updated_at, deleted_at)
        SELECT id, 'mercado_livre', ml_promotion_id, kind, name, starts_on, ends_on, status,
               coupon_kind, coupon_value, coupon_minimum, coupon_maximum, coupon_budget,
               currency, synced_at, created_at, updated_at, deleted_at
        FROM commerce_promotions;
    DROP TABLE commerce_promotions;
    ALTER TABLE commerce_promotions_by_channel RENAME TO commerce_promotions;

    CREATE TABLE commerce_promotion_offers_by_channel (
        id                   TEXT PRIMARY KEY,
        sales_channel        TEXT NOT NULL,
        channel_item_id      TEXT NOT NULL,
        kind                 TEXT NOT NULL,
        channel_promotion_id TEXT NOT NULL,
        name                 TEXT NOT NULL,
        status               TEXT NOT NULL,
        price                TEXT,
        currency             TEXT,
        starts_on            TEXT,
        ends_on              TEXT,
        synced_at            TEXT NOT NULL,
        created_at           TEXT NOT NULL,
        updated_at           TEXT NOT NULL,
        deleted_at           TEXT,
        UNIQUE (sales_channel, channel_item_id, kind, channel_promotion_id)
    );
    INSERT INTO commerce_promotion_offers_by_channel
        (id, sales_channel, channel_item_id, kind, channel_promotion_id, name, status, price,
         currency, starts_on, ends_on, synced_at, created_at, updated_at, deleted_at)
        SELECT id, 'mercado_livre', ml_item_id, kind, ml_promotion_id, name, status, price,
               currency, starts_on, ends_on, synced_at, created_at, updated_at, deleted_at
        FROM commerce_promotion_offers;
    DROP TABLE commerce_promotion_offers;
    ALTER TABLE commerce_promotion_offers_by_channel RENAME TO commerce_promotion_offers;

    CREATE TABLE commerce_channel_ranking (
        id            TEXT    PRIMARY KEY,
        sales_channel TEXT    NOT NULL,
        position      INTEGER NOT NULL,
        created_at    TEXT    NOT NULL,
        updated_at    TEXT    NOT NULL,
        deleted_at    TEXT
    );
    CREATE UNIQUE INDEX commerce_channel_ranking_by_channel
        ON commerce_channel_ranking (sales_channel) WHERE deleted_at IS NULL;",
};

/// The Sales Channels in the order the owner ranks them (`CONTEXT.md`):
/// Mercado Livre first until the owner says otherwise, and each channel
/// connected later after the ones before it.
pub struct ChannelRanking {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl ChannelRanking {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            database,
            clock,
            ids,
        }
    }

    /// The channels, first to last: the ones the owner ranked or that were
    /// connected, and Mercado Livre first when none was.
    pub async fn ranking(&self) -> Result<Vec<SalesChannelKind>, ListingError> {
        let ranked = read_ranking(self.database.connection()).await?;
        Ok(if ranked.is_empty() {
            vec![SalesChannelKind::MercadoLivre]
        } else {
            ranked
        })
    }

    /// Puts a channel just connected after the ones already ranked; one
    /// already there keeps its place. Returns the ranking.
    pub async fn connected(
        &self,
        channel: SalesChannelKind,
    ) -> Result<Vec<SalesChannelKind>, ListingError> {
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let mut ranking = read_ranking(&transaction).await?;
        if ranking.is_empty() {
            ranking.push(SalesChannelKind::MercadoLivre);
        }
        if !ranking.contains(&channel) {
            ranking.push(channel);
        }
        self.write_ranking(&transaction, &ranking).await?;
        transaction.commit().await?;
        Ok(ranking)
    }

    /// Saves the owner's ranking: the same channels as now, in a new order.
    pub async fn rank(&self, ranking: &[SalesChannelKind]) -> Result<(), ListingError> {
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let mut now = read_ranking(&transaction).await?;
        if now.is_empty() {
            now.push(SalesChannelKind::MercadoLivre);
        }
        let mut asked = ranking.to_vec();
        asked.sort_unstable();
        asked.dedup();
        now.sort_unstable();
        if asked.len() != ranking.len() || asked != now {
            return Err(ListingError::NotTheRanking);
        }
        self.write_ranking(&transaction, ranking).await?;
        transaction.commit().await?;
        Ok(())
    }

    async fn write_ranking(
        &self,
        on: &Connection,
        ranking: &[SalesChannelKind],
    ) -> Result<(), ListingError> {
        for (position, channel) in ranking.iter().enumerate() {
            let record = Record::new(self.ids.as_ref(), self.clock.as_ref());
            on.execute(
                "INSERT INTO commerce_channel_ranking
                     (id, sales_channel, position, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT (sales_channel) WHERE deleted_at IS NULL
                 DO UPDATE SET position = excluded.position, updated_at = excluded.updated_at
                     WHERE position <> excluded.position",
                params![
                    record.id.to_string(),
                    channel.code(),
                    i64::try_from(position).unwrap_or(i64::MAX),
                    stored(record.created_at)
                ],
            )
            .await?;
        }
        Ok(())
    }
}

async fn read_ranking(on: &Connection) -> Result<Vec<SalesChannelKind>, ListingError> {
    let mut rows = on
        .query(
            "SELECT sales_channel FROM commerce_channel_ranking WHERE deleted_at IS NULL
             ORDER BY position, created_at",
            (),
        )
        .await?;
    let mut ranking = Vec::new();
    while let Some(row) = rows.next().await? {
        ranking.push(row.sales_channel_at(0)?);
    }
    Ok(ranking)
}

/// The Sync cursor of each channel in `table`, as `read` makes it of the
/// live row's `columns`, which it finds from position 1 on.
pub(crate) async fn load_cursors<T>(
    on: &Connection,
    table: &'static str,
    columns: &[&'static str],
    read: impl Fn(&Row) -> Result<T, StoredValueError>,
) -> Result<Vec<(SalesChannelKind, T)>, StoredValueError> {
    let mut rows = on
        .query(
            &format!(
                "SELECT sales_channel, {} FROM {table} WHERE deleted_at IS NULL
                 ORDER BY sales_channel",
                columns.join(", ")
            ),
            (),
        )
        .await?;
    let mut cursors = Vec::new();
    while let Some(row) = rows.next().await? {
        cursors.push((row.sales_channel_at(0)?, read(&row)?));
    }
    Ok(cursors)
}

/// Saves `values` into `columns` of `channel`'s Sync cursor in `table`,
/// creating it the first time.
pub(crate) async fn save_cursor(
    on: &Connection,
    record: Record,
    table: &'static str,
    columns: &[&'static str],
    channel: SalesChannelKind,
    values: Vec<Value>,
) -> Result<(), libsql::Error> {
    let placeholders: Vec<String> = (5..5 + columns.len()).map(|at| format!("?{at}")).collect();
    let updates: Vec<String> = columns
        .iter()
        .map(|column| format!("{column} = excluded.{column}"))
        .collect();
    let mut params = vec![
        Value::Text(record.id.to_string()),
        Value::Text(channel.code().into()),
        Value::Text(stored(record.created_at)),
        Value::Text(stored(record.updated_at)),
    ];
    params.extend(values);
    on.execute(
        &format!(
            "INSERT INTO {table} (id, sales_channel, created_at, updated_at, {})
             VALUES (?1, ?2, ?3, ?4, {})
             ON CONFLICT (sales_channel) WHERE deleted_at IS NULL
             DO UPDATE SET {}, updated_at = excluded.updated_at",
            columns.join(", "),
            placeholders.join(", "),
            updates.join(", ")
        ),
        params,
    )
    .await?;
    Ok(())
}
