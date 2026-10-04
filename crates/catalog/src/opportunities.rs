//! Opportunities (#11): each Supplier Offer crossed with the catalog
//! products that match it in the Sales Channel, with the Estimated Margin
//! after the channel's Fees and a score to rank by. A Sync reads the best
//! sellers of the categories the owner follows and the competition of each
//! match; margins are worked out when read, so a new price or tax rate
//! shows at once.

use std::collections::{HashMap, HashSet};

use libsql::{Row, Value, params};
use mascate_kernel::{
    Currency, ListingType, Margin, Money, Percentage, PlatformError, RecordId, Timestamp,
    shipping_paid_by_seller,
};
use mascate_platform::{Migration, StoredRow, load_single_row, save_single_row, stored};
use rust_decimal::Decimal;

use crate::catalog::{OFFER_COLUMNS, offer_from, required};
use crate::{
    BestSeller, Catalog, CatalogError, Competition, DemandCategory, DemandSource, SupplierOffer,
};

pub(crate) const CREATE_DEMAND: Migration = Migration {
    version: 2,
    name: "create discovery settings, demand categories, best sellers and opportunities",
    risky: false,
    sql: "CREATE TABLE catalog_discovery_settings (
        id                    TEXT PRIMARY KEY,
        listing_type          TEXT NOT NULL,
        estimated_fee_percent TEXT NOT NULL,
        estimated_shipping    TEXT NOT NULL,
        currency              TEXT NOT NULL,
        created_at            TEXT NOT NULL,
        updated_at            TEXT NOT NULL,
        deleted_at            TEXT
    );
    CREATE TABLE catalog_demand_categories (
        id             TEXT PRIMARY KEY,
        ml_category_id TEXT NOT NULL,
        name           TEXT NOT NULL,
        created_at     TEXT NOT NULL,
        updated_at     TEXT NOT NULL,
        deleted_at     TEXT
    );
    CREATE UNIQUE INDEX catalog_demand_categories_by_ml_id
        ON catalog_demand_categories (ml_category_id) WHERE deleted_at IS NULL;
    CREATE TABLE catalog_best_sellers (
        id                 TEXT PRIMARY KEY,
        ml_category_id     TEXT NOT NULL,
        ml_id              TEXT NOT NULL,
        position           INTEGER NOT NULL,
        title              TEXT NOT NULL,
        catalog_product_id TEXT,
        price              TEXT,
        currency           TEXT,
        synced_at          TEXT NOT NULL,
        created_at         TEXT NOT NULL,
        updated_at         TEXT NOT NULL,
        deleted_at         TEXT
    );
    CREATE UNIQUE INDEX catalog_best_sellers_by_ml_id
        ON catalog_best_sellers (ml_category_id, ml_id) WHERE deleted_at IS NULL;
    CREATE TABLE catalog_opportunities (
        id                    TEXT PRIMARY KEY,
        link_key              TEXT NOT NULL,
        ml_product_id         TEXT NOT NULL,
        product_name          TEXT NOT NULL,
        category_id           TEXT,
        category_name         TEXT,
        competitors           INTEGER NOT NULL,
        average_price         TEXT NOT NULL,
        lowest_price          TEXT NOT NULL,
        highest_price         TEXT NOT NULL,
        currency              TEXT NOT NULL,
        sale_fee              TEXT,
        sale_fee_listing_type TEXT,
        synced_at             TEXT NOT NULL,
        dismissed_at          TEXT,
        dismissal_reason      TEXT,
        created_at            TEXT NOT NULL,
        updated_at            TEXT NOT NULL,
        deleted_at            TEXT
    );
    CREATE UNIQUE INDEX catalog_opportunities_by_match
        ON catalog_opportunities (link_key, ml_product_id) WHERE deleted_at IS NULL;",
};

const SETTINGS_TABLE: &str = "catalog_discovery_settings";
const SETTINGS_COLUMNS: &[&str] = &[
    "listing_type",
    "estimated_fee_percent",
    "estimated_shipping",
    "currency",
];

/// How many catalog products each Supplier Offer is compared with: the best
/// few search results, so discarding a wrong match leaves the right one.
pub const MATCHES_PER_OFFER: usize = 3;

/// A best seller's place adds this many points minus its position, so the
/// best seller of a category earns 20 and the twentieth earns 1.
const BEST_SELLER_POINTS: u32 = 21;

/// Each competing listing takes a point off the score, up to this many.
const MAX_CROWDING_POINTS: u32 = 20;

/// What the owner assumes where the Sales Channel does not say: the listing
/// type, the sale fee rate when the channel reports none, and the shipping
/// paid on each unit sold with free shipping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoverySettings {
    pub listing_type: ListingType,
    pub estimated_fee: Percentage,
    pub estimated_shipping: Money,
}

impl Default for DiscoverySettings {
    fn default() -> Self {
        Self {
            listing_type: ListingType::Classic,
            estimated_fee: Percentage::new(Decimal::from(14)).expect("14% is a percentage"),
            estimated_shipping: Money::new(Decimal::from(20), Currency::Brl),
        }
    }
}

/// A Fee and whether it is the owner's estimate instead of the channel's
/// own figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Charge {
    pub amount: Money,
    pub estimated: bool,
}

/// A Supplier Offer crossed with a catalog product of the Sales Channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opportunity {
    pub id: RecordId,
    /// The latest offer at the matched link.
    pub offer: SupplierOffer,
    /// The catalog product's id in the Sales Channel.
    pub product_id: String,
    pub product_name: String,
    /// The top-level category it sells in, when known.
    pub category: Option<DemandCategory>,
    pub competitors: u32,
    /// The average price of the competing listings: what it can sell for.
    pub sale_price: Money,
    pub lowest_price: Money,
    pub highest_price: Money,
    pub sale_fee: Charge,
    pub shipping: Charge,
    pub tax: Money,
    /// What one unit costs delivered from the Supplier.
    pub cost: Money,
    pub margin: Margin,
    /// Its place among the best sellers of a followed category.
    pub best_seller: Option<u32>,
    /// Higher ranks first: the margin's percentage, plus points for being a
    /// best seller, minus a point per competing listing (up to 20).
    pub score: Decimal,
    pub synced_at: Timestamp,
}

/// Which Opportunities to list. Empty fields filter nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpportunityFilter {
    /// The lowest Estimated Margin, in percent of the sale price.
    pub min_margin: Option<Decimal>,
    /// A top-level category id.
    pub category: Option<String>,
    pub min_price: Option<Decimal>,
    pub max_price: Option<Decimal>,
}

/// A followed category with its best sellers, best first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryBestSellers {
    pub category: DemandCategory,
    pub best_sellers: Vec<RankedBestSeller>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedBestSeller {
    pub best_seller: BestSeller,
    /// Whether one of the owner's Supplier Offers matches it.
    pub has_opportunity: bool,
}

/// What a Sync of demand read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DemandSync {
    pub best_sellers: usize,
    pub offers: usize,
    pub opportunities: usize,
    /// Offers priced in another currency than the Sales Channel's.
    pub other_currency: usize,
    /// Offers no catalog product with a price matched.
    pub unmatched: usize,
}

/// One catalog product matched to an offer, as the Sync stores it.
struct Matched {
    product: String,
    name: String,
    category: Option<DemandCategory>,
    competitors: u32,
    average: Money,
    lowest: Money,
    highest: Money,
    sale_fee: Option<Money>,
}

impl Catalog {
    pub async fn discovery_settings(&self) -> Result<DiscoverySettings, CatalogError> {
        let Some(row) =
            load_single_row(self.connection(), SETTINGS_TABLE, SETTINGS_COLUMNS).await?
        else {
            return Ok(DiscoverySettings::default());
        };
        let code: String = row.get(0)?;
        let currency = row.currency_at(3)?;
        Ok(DiscoverySettings {
            listing_type: ListingType::from_code(&code).ok_or(CatalogError::Unreadable(code))?,
            estimated_fee: Percentage::new(row.decimal_at(1)?)
                .map_err(|_| CatalogError::Unreadable("estimated fee".into()))?,
            estimated_shipping: Money::new(row.decimal_at(2)?, currency),
        })
    }

    pub async fn save_discovery_settings(
        &self,
        settings: DiscoverySettings,
    ) -> Result<(), CatalogError> {
        if settings.estimated_shipping.is_negative() {
            return Err(CatalogError::Negative);
        }
        save_single_row(
            self.connection(),
            self.clock(),
            self.ids(),
            SETTINGS_TABLE,
            SETTINGS_COLUMNS,
            vec![
                settings.listing_type.code().into(),
                settings.estimated_fee.percent().to_string().into(),
                settings.estimated_shipping.amount().to_string().into(),
                settings.estimated_shipping.currency().code().into(),
            ],
        )
        .await?;
        Ok(())
    }

    /// The categories whose best sellers the owner follows, by name.
    pub async fn demand_categories(&self) -> Result<Vec<DemandCategory>, CatalogError> {
        let mut rows = self
            .connection()
            .query(
                "SELECT ml_category_id, name FROM catalog_demand_categories
                 WHERE deleted_at IS NULL ORDER BY name COLLATE NOCASE",
                (),
            )
            .await?;
        let mut categories = Vec::new();
        while let Some(row) = rows.next().await? {
            categories.push(DemandCategory {
                id: row.get(0)?,
                name: row.get(1)?,
            });
        }
        Ok(categories)
    }

    /// Follows exactly `categories`. A category left out stops being synced
    /// and its best sellers stop showing.
    pub async fn follow_categories(
        &self,
        categories: &[DemandCategory],
    ) -> Result<(), CatalogError> {
        let connection = self.database().connect_for_transaction().await?;
        let transaction = connection.transaction().await?;
        let now = self.now();
        let mut followed = HashSet::new();
        for category in categories {
            if !followed.insert(category.id.as_str()) {
                continue;
            }
            let renamed = transaction
                .execute(
                    "UPDATE catalog_demand_categories SET name = ?1, updated_at = ?2
                     WHERE ml_category_id = ?3 AND deleted_at IS NULL",
                    params![category.name.clone(), now.clone(), category.id.clone()],
                )
                .await?;
            if renamed == 0 {
                let record = self.new_record();
                transaction
                    .execute(
                        "INSERT INTO catalog_demand_categories
                             (id, ml_category_id, name, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?4)",
                        params![
                            record.id.to_string(),
                            category.id.clone(),
                            category.name.clone(),
                            stored(record.created_at)
                        ],
                    )
                    .await?;
            }
        }
        let mut rows = transaction
            .query(
                "SELECT ml_category_id FROM catalog_demand_categories WHERE deleted_at IS NULL",
                (),
            )
            .await?;
        let mut dropped = Vec::new();
        while let Some(row) = rows.next().await? {
            let id: String = row.get(0)?;
            if !followed.contains(id.as_str()) {
                dropped.push(id);
            }
        }
        for id in dropped {
            transaction
                .execute(
                    "UPDATE catalog_demand_categories SET deleted_at = ?1, updated_at = ?1
                     WHERE ml_category_id = ?2 AND deleted_at IS NULL",
                    params![now.clone(), id],
                )
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    /// Reads the best sellers of every followed category and compares each
    /// link's latest Supplier Offer with the catalog products that match its
    /// title. Running it again with the same answers leaves the same state;
    /// a discarded Opportunity stays discarded.
    pub async fn sync_demand(&self, source: &dyn DemandSource) -> Result<DemandSync, CatalogError> {
        let settings = self.discovery_settings().await?;
        let currency = source.currency();
        let mut report = DemandSync::default();
        for category in self.demand_categories().await? {
            let best_sellers = source.best_sellers(&category.id)?;
            report.best_sellers += best_sellers.len();
            self.store_best_sellers(&category.id, &best_sellers).await?;
        }
        let mut roots: HashMap<String, Option<DemandCategory>> = HashMap::new();
        for (key, offer) in self.latest_offers().await? {
            report.offers += 1;
            if offer.price.currency() != currency {
                report.other_currency += 1;
                continue;
            }
            let matches = optional(source.search(&offer.title))?.unwrap_or_default();
            let mut kept = Vec::new();
            for found in matches.into_iter().take(MATCHES_PER_OFFER) {
                let Some(competition) = optional(source.competition(&found.id))? else {
                    continue;
                };
                let Some(prices) = PriceRange::of(&competition, currency) else {
                    continue;
                };
                let category = match &competition.category {
                    Some(id) => match roots.get(id) {
                        Some(root) => root.clone(),
                        None => {
                            let root = optional(source.root_category(id))?;
                            roots.insert(id.clone(), root.clone());
                            root
                        }
                    },
                    None => None,
                };
                let sale_fee = match &competition.category {
                    Some(id) => optional(source.sale_fee(
                        id,
                        prices.average.rounded(),
                        settings.listing_type,
                    ))?
                    .filter(|fee| fee.currency() == currency && !fee.is_negative()),
                    None => None,
                };
                self.store_match(
                    &key,
                    Matched {
                        product: found.id.clone(),
                        name: found.name,
                        category,
                        competitors: competition.sellers,
                        average: prices.average,
                        lowest: prices.lowest,
                        highest: prices.highest,
                        sale_fee,
                    },
                    settings.listing_type,
                )
                .await?;
                kept.push(found.id);
            }
            self.forget_stale_matches(&key, &kept).await?;
            if kept.is_empty() {
                report.unmatched += 1;
            }
            report.opportunities += kept.len();
        }
        Ok(report)
    }

    /// When demand was last synced; `None` before the first Sync.
    pub async fn last_demand_sync(&self) -> Result<Option<Timestamp>, CatalogError> {
        let mut rows = self
            .connection()
            .query(
                "SELECT MAX(synced_at) FROM (
                     SELECT synced_at FROM catalog_best_sellers WHERE deleted_at IS NULL
                     UNION ALL
                     SELECT synced_at FROM catalog_opportunities WHERE deleted_at IS NULL)",
                (),
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(row.optional_time_at(0)?),
            None => Ok(None),
        }
    }

    /// The best sellers of every followed category, as last synced.
    pub async fn best_sellers(&self) -> Result<Vec<CategoryBestSellers>, CatalogError> {
        let matched: HashSet<String> = {
            let mut rows = self
                .connection()
                .query(
                    "SELECT ml_product_id FROM catalog_opportunities
                     WHERE deleted_at IS NULL AND dismissed_at IS NULL",
                    (),
                )
                .await?;
            let mut matched = HashSet::new();
            while let Some(row) = rows.next().await? {
                matched.insert(row.get::<String>(0)?);
            }
            matched
        };
        let mut listed = Vec::new();
        for category in self.demand_categories().await? {
            let mut rows = self
                .connection()
                .query(
                    "SELECT ml_id, position, title, catalog_product_id, price, currency
                     FROM catalog_best_sellers
                     WHERE ml_category_id = ?1 AND deleted_at IS NULL ORDER BY position",
                    params![category.id.clone()],
                )
                .await?;
            let mut best_sellers = Vec::new();
            while let Some(row) = rows.next().await? {
                let best_seller = best_seller_from(&row)?;
                let has_opportunity = best_seller
                    .catalog_product
                    .as_ref()
                    .is_some_and(|product| matched.contains(product));
                best_sellers.push(RankedBestSeller {
                    best_seller,
                    has_opportunity,
                });
            }
            listed.push(CategoryBestSellers {
                category,
                best_sellers,
            });
        }
        Ok(listed)
    }

    /// The Opportunities that pass `filter`, highest score first, with
    /// margins worked out with the current settings and `tax`, the rate of
    /// tax on each sale. Discarded ones never show.
    pub async fn opportunities(
        &self,
        filter: &OpportunityFilter,
        tax: Percentage,
    ) -> Result<Vec<Opportunity>, CatalogError> {
        let settings = self.discovery_settings().await?;
        let offers: HashMap<String, SupplierOffer> =
            self.latest_offers().await?.into_iter().collect();
        let positions = self.best_seller_positions().await?;
        let mut rows = self
            .connection()
            .query(
                "SELECT id, link_key, ml_product_id, product_name, category_id, category_name,
                        competitors, average_price, lowest_price, highest_price, currency,
                        sale_fee, sale_fee_listing_type, synced_at
                 FROM catalog_opportunities
                 WHERE deleted_at IS NULL AND dismissed_at IS NULL",
                (),
            )
            .await?;
        let mut listed = Vec::new();
        while let Some(row) = rows.next().await? {
            let key: String = row.get(1)?;
            let Some(offer) = offers.get(&key) else {
                continue;
            };
            let Some(opportunity) = opportunity_from(&row, offer, &settings, tax, &positions)?
            else {
                continue;
            };
            if filter.passes(&opportunity) {
                listed.push(opportunity);
            }
        }
        listed.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| b.margin.amount.amount().cmp(&a.margin.amount.amount()))
                .then_with(|| a.product_name.cmp(&b.product_name))
        });
        Ok(listed)
    }

    /// Discards an Opportunity for good, saying why. Later Syncs that find
    /// the same match keep it discarded.
    pub async fn dismiss_opportunity(
        &self,
        opportunity: RecordId,
        reason: &str,
    ) -> Result<(), CatalogError> {
        let reason = required(reason, CatalogError::MissingReason)?;
        let now = self.now();
        let dismissed = self
            .connection()
            .execute(
                "UPDATE catalog_opportunities
                 SET dismissed_at = ?1, dismissal_reason = ?2, updated_at = ?1
                 WHERE id = ?3 AND deleted_at IS NULL AND dismissed_at IS NULL",
                params![now, reason, opportunity.to_string()],
            )
            .await?;
        if dismissed == 0 {
            return Err(CatalogError::UnknownOpportunity(opportunity));
        }
        Ok(())
    }

    /// How many Opportunities the owner discarded.
    pub async fn dismissed_opportunities(&self) -> Result<u64, CatalogError> {
        let mut rows = self
            .connection()
            .query(
                "SELECT COUNT(*) FROM catalog_opportunities
                 WHERE deleted_at IS NULL AND dismissed_at IS NOT NULL",
                (),
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(row.get::<u64>(0)?),
            None => Ok(0),
        }
    }

    /// Each link's latest Supplier Offer, by the link's key.
    async fn latest_offers(&self) -> Result<Vec<(String, SupplierOffer)>, CatalogError> {
        let mut rows = self
            .connection()
            .query(
                &format!("SELECT {OFFER_COLUMNS} ORDER BY o.observed_at DESC, o.id DESC"),
                (),
            )
            .await?;
        let mut seen = HashSet::new();
        let mut latest = Vec::new();
        while let Some(row) = rows.next().await? {
            let key: String = row.get(11)?;
            if seen.insert(key.clone()) {
                latest.push((key, offer_from(&row)?));
            }
        }
        Ok(latest)
    }

    /// The best place each catalog product holds among the best sellers of
    /// the followed categories.
    async fn best_seller_positions(&self) -> Result<HashMap<String, u32>, CatalogError> {
        let mut rows = self
            .connection()
            .query(
                "SELECT b.catalog_product_id, MIN(b.position) FROM catalog_best_sellers b
                 JOIN catalog_demand_categories c
                   ON c.ml_category_id = b.ml_category_id AND c.deleted_at IS NULL
                 WHERE b.deleted_at IS NULL AND b.catalog_product_id IS NOT NULL
                 GROUP BY b.catalog_product_id",
                (),
            )
            .await?;
        let mut positions = HashMap::new();
        while let Some(row) = rows.next().await? {
            positions.insert(row.get::<String>(0)?, row.get::<u32>(1)?);
        }
        Ok(positions)
    }

    /// Replaces the stored best sellers of `category` with `best_sellers`.
    async fn store_best_sellers(
        &self,
        category: &str,
        best_sellers: &[BestSeller],
    ) -> Result<(), CatalogError> {
        let connection = self.database().connect_for_transaction().await?;
        let transaction = connection.transaction().await?;
        let now = self.now();
        let mut kept = HashSet::new();
        for best_seller in best_sellers {
            if !kept.insert(best_seller.id.as_str()) {
                continue;
            }
            let price = best_seller.price.map(|price| price.amount().to_string());
            let currency = best_seller.price.map(|price| price.currency().code());
            let updated = transaction
                .execute(
                    "UPDATE catalog_best_sellers
                     SET position = ?1, title = ?2, catalog_product_id = ?3, price = ?4,
                         currency = ?5, synced_at = ?6, updated_at = ?6
                     WHERE ml_category_id = ?7 AND ml_id = ?8 AND deleted_at IS NULL",
                    params![
                        best_seller.position,
                        best_seller.title.clone(),
                        best_seller.catalog_product.clone(),
                        price.clone(),
                        currency,
                        now.clone(),
                        category,
                        best_seller.id.clone()
                    ],
                )
                .await?;
            if updated == 0 {
                transaction
                    .execute(
                        "INSERT INTO catalog_best_sellers
                             (id, ml_category_id, ml_id, position, title, catalog_product_id,
                              price, currency, synced_at, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9, ?9)",
                        params![
                            self.next_id().to_string(),
                            category,
                            best_seller.id.clone(),
                            best_seller.position,
                            best_seller.title.clone(),
                            best_seller.catalog_product.clone(),
                            price,
                            currency,
                            now.clone()
                        ],
                    )
                    .await?;
            }
        }
        let mut rows = transaction
            .query(
                "SELECT ml_id FROM catalog_best_sellers
                 WHERE ml_category_id = ?1 AND deleted_at IS NULL",
                params![category],
            )
            .await?;
        let mut gone = Vec::new();
        while let Some(row) = rows.next().await? {
            let id: String = row.get(0)?;
            if !kept.contains(id.as_str()) {
                gone.push(id);
            }
        }
        for id in gone {
            transaction
                .execute(
                    "UPDATE catalog_best_sellers SET deleted_at = ?1, updated_at = ?1
                     WHERE ml_category_id = ?2 AND ml_id = ?3 AND deleted_at IS NULL",
                    params![now.clone(), category, id],
                )
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    async fn store_match(
        &self,
        key: &str,
        matched: Matched,
        listing_type: ListingType,
    ) -> Result<(), CatalogError> {
        let now = self.now();
        let (category_id, category_name) = match matched.category {
            Some(category) => (Some(category.id), Some(category.name)),
            None => (None, None),
        };
        let sale_fee = matched.sale_fee.map(|fee| fee.amount().to_string());
        let fee_listing = matched.sale_fee.map(|_| listing_type.code());
        let values: Vec<Value> = vec![
            matched.name.into(),
            category_id.into(),
            category_name.into(),
            matched.competitors.into(),
            matched.average.amount().to_string().into(),
            matched.lowest.amount().to_string().into(),
            matched.highest.amount().to_string().into(),
            matched.average.currency().code().into(),
            sale_fee.into(),
            fee_listing.into(),
            now.into(),
            key.into(),
            matched.product.into(),
        ];
        let updated = self
            .connection()
            .execute(
                "UPDATE catalog_opportunities
                 SET product_name = ?1, category_id = ?2, category_name = ?3, competitors = ?4,
                     average_price = ?5, lowest_price = ?6, highest_price = ?7, currency = ?8,
                     sale_fee = ?9, sale_fee_listing_type = ?10, synced_at = ?11,
                     updated_at = ?11
                 WHERE link_key = ?12 AND ml_product_id = ?13 AND deleted_at IS NULL",
                values.clone(),
            )
            .await?;
        if updated == 0 {
            let mut values = values;
            values.push(self.next_id().to_string().into());
            self.connection()
                .execute(
                    "INSERT INTO catalog_opportunities
                         (product_name, category_id, category_name, competitors, average_price,
                          lowest_price, highest_price, currency, sale_fee,
                          sale_fee_listing_type, synced_at, link_key, ml_product_id, id,
                          created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                             ?11, ?11)",
                    values,
                )
                .await?;
        }
        Ok(())
    }

    /// Drops the matches of the link `key` that this Sync no longer found,
    /// except discarded ones, which stay so they never come back.
    async fn forget_stale_matches(&self, key: &str, kept: &[String]) -> Result<(), CatalogError> {
        let mut rows = self
            .connection()
            .query(
                "SELECT ml_product_id FROM catalog_opportunities
                 WHERE link_key = ?1 AND deleted_at IS NULL AND dismissed_at IS NULL",
                params![key],
            )
            .await?;
        let mut stale = Vec::new();
        while let Some(row) = rows.next().await? {
            let product: String = row.get(0)?;
            if !kept.contains(&product) {
                stale.push(product);
            }
        }
        let now = self.now();
        for product in stale {
            self.connection()
                .execute(
                    "UPDATE catalog_opportunities SET deleted_at = ?1, updated_at = ?1
                     WHERE link_key = ?2 AND ml_product_id = ?3 AND deleted_at IS NULL",
                    params![now.clone(), key, product],
                )
                .await?;
        }
        Ok(())
    }
}

/// `answer`, with `NotFound` as nothing and a refusal of that one request
/// as nothing too; whatever stops the whole Sync stays an error.
fn optional<T>(answer: Result<T, PlatformError>) -> Result<Option<T>, CatalogError> {
    match answer {
        Ok(value) => Ok(Some(value)),
        Err(PlatformError::NotFound | PlatformError::Refused(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// The lowest, highest and average price of a competition.
struct PriceRange {
    average: Money,
    lowest: Money,
    highest: Money,
}

impl PriceRange {
    /// `None` when no listing has a positive price in `currency`.
    fn of(competition: &Competition, currency: Currency) -> Option<Self> {
        let prices: Vec<Money> = competition
            .prices
            .iter()
            .copied()
            .filter(|price| price.currency() == currency && price.amount() > Decimal::ZERO)
            .collect();
        let lowest = *prices.iter().min_by_key(|price| price.amount())?;
        let highest = *prices.iter().max_by_key(|price| price.amount())?;
        let total = Money::sum(currency, prices.iter().copied()).ok()?;
        let average = Money::new(total.amount() / Decimal::from(prices.len()), currency);
        Some(Self {
            average,
            lowest,
            highest,
        })
    }
}

impl OpportunityFilter {
    fn passes(&self, opportunity: &Opportunity) -> bool {
        let price = opportunity.sale_price.amount();
        self.min_margin.is_none_or(|min| {
            opportunity
                .margin
                .percent
                .is_some_and(|percent| percent >= min)
        }) && self.category.as_ref().is_none_or(|wanted| {
            opportunity
                .category
                .as_ref()
                .is_some_and(|category| &category.id == wanted)
        }) && self.min_price.is_none_or(|min| price >= min)
            && self.max_price.is_none_or(|max| price <= max)
    }
}

/// The Opportunity a stored match makes with `offer`; `None` when the offer
/// is now priced in another currency than the match.
fn opportunity_from(
    row: &Row,
    offer: &SupplierOffer,
    settings: &DiscoverySettings,
    tax: Percentage,
    positions: &HashMap<String, u32>,
) -> Result<Option<Opportunity>, CatalogError> {
    let currency = row.currency_at(10)?;
    let money =
        |at| -> Result<Money, CatalogError> { Ok(Money::new(row.decimal_at(at)?, currency)) };
    let sale_price = money(7)?;
    let cost = offer.total();
    if cost.currency() != currency {
        return Ok(None);
    }
    let fee_listing: Option<String> = row.get(12)?;
    let reported_fee = match row.get::<Option<String>>(11)? {
        Some(_) if fee_listing.as_deref() == Some(settings.listing_type.code()) => Some(money(11)?),
        _ => None,
    };
    let sale_fee = match reported_fee {
        Some(amount) => Charge {
            amount,
            estimated: false,
        },
        None => Charge {
            amount: settings.estimated_fee.of(sale_price),
            estimated: true,
        },
    };
    let shipping = Charge {
        amount: shipping_paid_by_seller(sale_price, settings.estimated_shipping),
        estimated: true,
    };
    let tax_amount = tax.of(sale_price);
    let margin = Margin::of(
        sale_price,
        &[sale_fee.amount, shipping.amount, tax_amount, cost],
    )?;
    let product_id: String = row.get(2)?;
    let best_seller = positions.get(&product_id).copied();
    let competitors: u32 = row.get(6)?;
    let category = match (row.get::<Option<String>>(4)?, row.get::<Option<String>>(5)?) {
        (Some(id), Some(name)) => Some(DemandCategory { id, name }),
        _ => None,
    };
    Ok(Some(Opportunity {
        id: row.id_at(0)?,
        offer: offer.clone(),
        product_name: row.get(3)?,
        category,
        competitors,
        sale_price,
        lowest_price: money(8)?,
        highest_price: money(9)?,
        sale_fee,
        shipping,
        tax: tax_amount,
        cost,
        score: score(&margin, best_seller, competitors),
        margin,
        best_seller,
        product_id,
        synced_at: row.time_at(13)?,
    }))
}

/// The margin's percentage, plus points for a best seller's place, minus a
/// point per competing listing.
pub fn score(margin: &Margin, best_seller: Option<u32>, competitors: u32) -> Decimal {
    let bonus = best_seller.map_or(0, |position| BEST_SELLER_POINTS.saturating_sub(position));
    let crowding = competitors.min(MAX_CROWDING_POINTS);
    margin.percent.unwrap_or_default() + Decimal::from(bonus) - Decimal::from(crowding)
}

fn best_seller_from(row: &Row) -> Result<BestSeller, CatalogError> {
    let price = match row.get::<Option<String>>(4)? {
        Some(_) => Some(Money::new(row.decimal_at(4)?, row.currency_at(5)?)),
        None => None,
    };
    Ok(BestSeller {
        id: row.get(0)?,
        position: row.get(1)?,
        title: row.get(2)?,
        catalog_product: row.get(3)?,
        price,
    })
}
