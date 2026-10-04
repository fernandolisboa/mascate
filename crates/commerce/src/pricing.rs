//! Prices (#19): the margin one sale leaves at a price, Fee by Fee, as the
//! price simulator shows it; the lowest price that leaves a target margin;
//! each Product's target margin; and the Price Suggestion for a Listing,
//! which only the owner sends to the Sales Channel.

use std::sync::Arc;

use libsql::params;
use mascate_inventory::{Inventory, InventoryError};
use mascate_kernel::{
    Clock, CurrencyMismatch, IdGenerator, ListingType, Margin, Money, Percentage, PlatformError,
    Record, RecordId, shipping_paid_by_seller,
};
use mascate_platform::{
    Database, Migration, StoredRow, StoredValueError, load_single_row, save_single_row, stored,
};
use rust_decimal::{Decimal, RoundingStrategy};

use crate::{Listing, ListingError, ListingStatus, Listings, SalesChannel};

/// The target margin of a Product that has none of its own, until the owner
/// sets another.
const DEFAULT_TARGET_MARGIN: Decimal = Decimal::from_parts(20, 0, 0, false, 0);

/// How many times the Sales Channel is asked for its fee while the price
/// moves into a fee band or across the free shipping price.
const MAX_FEE_LOOKUPS: usize = 6;

/// One cent: prices go to the Sales Channel in whole cents.
const CENT: Decimal = Decimal::from_parts(1, 0, 0, false, 2);

pub(crate) const CREATE_TARGET_MARGINS: Migration = Migration {
    version: 3,
    name: "create target margins",
    risky: false,
    sql: "CREATE TABLE commerce_pricing_settings (
        id                    TEXT PRIMARY KEY,
        default_target_margin TEXT NOT NULL,
        created_at            TEXT NOT NULL,
        updated_at            TEXT NOT NULL,
        deleted_at            TEXT
    );
    CREATE TABLE commerce_target_margins (
        id         TEXT PRIMARY KEY,
        product_id TEXT NOT NULL UNIQUE,
        margin     TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );",
};

const SETTINGS_TABLE: &str = "commerce_pricing_settings";
const SETTINGS_COLUMNS: &[&str] = &["default_target_margin"];

/// What the Sales Channel keeps of a sale: a share of the sale price plus a
/// fixed amount per unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaleFee {
    pub rate: Percentage,
    pub fixed: Money,
}

impl SaleFee {
    /// The fee on a sale at `sale_price`.
    pub fn on(self, sale_price: Money) -> Result<Money, CurrencyMismatch> {
        self.rate.of(sale_price).checked_add(self.fixed)
    }
}

/// One unit sold at a price, with every Fee and the unit's cost: what the
/// price simulator shows and the Price Suggestion solves for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceScenario {
    /// The listed price.
    pub price: Money,
    /// Taken off the listed price; the Fees are charged on what is left.
    pub discount: Percentage,
    pub sale_fee: SaleFee,
    /// What the seller pays to ship the unit.
    pub shipping: Money,
    /// Advertising cost per unit sold.
    pub ads: Money,
    pub tax: Percentage,
    /// The unit's Average Cost.
    pub cost: Money,
}

/// A sale Fee by Fee, and the margin it leaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceBreakdown {
    /// The listed price minus the discount.
    pub sale_price: Money,
    pub sale_fee: Money,
    pub shipping: Money,
    pub ads: Money,
    pub tax: Money,
    pub cost: Money,
    pub margin: Margin,
}

impl PriceScenario {
    /// The same sale at another listed price.
    pub fn at(self, price: Money) -> Self {
        Self { price, ..self }
    }

    /// The listed price minus the discount.
    pub fn sale_price(&self) -> Money {
        self.price
            .times((Decimal::ONE_HUNDRED - self.discount.percent()) / Decimal::ONE_HUNDRED)
    }

    pub fn breakdown(&self) -> Result<PriceBreakdown, CurrencyMismatch> {
        let sale_price = self.sale_price();
        let sale_fee = self.sale_fee.on(sale_price)?;
        let tax = self.tax.of(sale_price);
        let margin = Margin::of(
            sale_price,
            &[sale_fee, self.shipping, self.ads, tax, self.cost],
        )?;
        Ok(PriceBreakdown {
            sale_price,
            sale_fee,
            shipping: self.shipping,
            ads: self.ads,
            tax,
            cost: self.cost,
            margin,
        })
    }

    /// Whether the margin is at least `target` of the sale price.
    pub fn reaches(&self, target: Percentage) -> Result<bool, CurrencyMismatch> {
        let breakdown = self.breakdown()?;
        Ok(breakdown.margin.amount.amount() >= target.of(breakdown.sale_price).amount())
    }

    /// The lowest listed price, in whole cents, whose margin reaches
    /// `target` with this sale's discount, Fees and cost. `None` when no
    /// price does: the shares of the price that go to the discount, the sale
    /// fee, the tax and the target leave nothing to pay the rest.
    pub fn lowest_price_for(&self, target: Percentage) -> Result<Option<Money>, CurrencyMismatch> {
        let currency = self.price.currency();
        let fixed = Money::sum(
            currency,
            [self.sale_fee.fixed, self.shipping, self.ads, self.cost],
        )?;
        // Margin = sale price × (1 − fee rate − tax − target) − fixed costs;
        // the sale price is the listed price × (1 − discount).
        let kept = (Decimal::ONE_HUNDRED
            - self.sale_fee.rate.percent()
            - self.tax.percent()
            - target.percent())
            / Decimal::ONE_HUNDRED;
        let undiscounted = (Decimal::ONE_HUNDRED - self.discount.percent()) / Decimal::ONE_HUNDRED;
        if kept <= Decimal::ZERO || undiscounted <= Decimal::ZERO {
            return Ok(None);
        }
        let Some(exact) = fixed
            .amount()
            .max(Decimal::ZERO)
            .checked_div(kept * undiscounted)
        else {
            return Ok(None);
        };
        let mut cents = exact
            .round_dp_with_strategy(2, RoundingStrategy::ToPositiveInfinity)
            .max(CENT);
        // The division rounds past 28 digits; step to the exact cent.
        while !self.at(Money::new(cents, currency)).reaches(target)? {
            cents += CENT;
        }
        while cents > CENT
            && self
                .at(Money::new(cents - CENT, currency))
                .reaches(target)?
        {
            cents -= CENT;
        }
        Ok(Some(Money::new(cents, currency)))
    }
}

/// A Product's target margin and whether it is its own or the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetMargin {
    pub margin: Percentage,
    /// Set for this Product, instead of the default for all.
    pub own: bool,
}

/// What the caller knows of the sale that commerce does not: the tax rate
/// (Finance) and the shipping estimate (the catalog's discovery settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceAssumptions {
    pub tax: Percentage,
    /// What the seller pays to ship a unit once free shipping is required.
    pub estimated_shipping: Money,
}

/// The price the app suggests for a Listing and every variation that shares
/// it. It only reaches the Sales Channel when the owner approves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceSuggestion {
    /// The Listings whose price changes together: the listing's variations.
    pub listings: Vec<RecordId>,
    /// The lowest price in whole cents at which every one of them reaches
    /// its Product's target margin.
    pub price: Money,
    /// The Product whose margin set the price: the one that needs it highest.
    pub product: RecordId,
    pub target: TargetMargin,
    /// That Product's sale at the suggested price.
    pub suggested: PriceScenario,
    /// That Product's sale at today's price.
    pub current: PriceScenario,
}

/// Where the unit cost of a draft's price came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostSource {
    AverageCost,
    /// The Product never came into stock; the caller's Supplier Offer.
    SupplierOffer,
}

/// The price the app suggests for a draft, and the sale at that price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftPrice {
    pub suggested: PriceScenario,
    pub target: TargetMargin,
    pub cost_from: CostSource,
}

#[derive(Debug, thiserror::Error)]
pub enum PricingError {
    #[error(transparent)]
    Listing(#[from] ListingError),
    #[error("the listing is closed in the Sales Channel")]
    Closed,
    #[error("Listing {0} is not linked to a Product")]
    NotLinked(RecordId),
    #[error("Product {0} never came into stock, so it has no Average Cost")]
    NoCost(RecordId),
    #[error("the Sales Channel did not say the listing's category or type")]
    NoFeeBasis,
    #[error("no price reaches the target margin: the sale fee, tax and target take it all")]
    Unreachable,
    #[error("the sale fee kept changing with the price")]
    Unsettled,
    #[error("the target margin goes from 0 up to, not including, 100%")]
    InvalidTarget,
    #[error(transparent)]
    Currencies(#[from] CurrencyMismatch),
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error(transparent)]
    Stock(#[from] InventoryError),
    #[error("the prices hold a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for PricingError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => PricingError::Unreadable(text),
            StoredValueError::Sql(error) => PricingError::Sql(error),
        }
    }
}

/// Target margins and Price Suggestions. The cost of each unit is its
/// Average Cost in Inventory.
pub struct Pricing {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
    listings: Listings,
    inventory: Arc<Inventory>,
}

impl Pricing {
    pub fn new(
        database: Arc<Database>,
        inventory: Arc<Inventory>,
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            listings: Listings::new(database.clone(), clock.clone(), ids.clone()),
            database,
            clock,
            ids,
            inventory,
        }
    }

    /// The target margin of every Product without one of its own: 20%
    /// until the owner sets another.
    pub async fn default_target_margin(&self) -> Result<Percentage, PricingError> {
        let row =
            load_single_row(self.database.connection(), SETTINGS_TABLE, SETTINGS_COLUMNS).await?;
        match row {
            Some(row) => stored_target(row.decimal_at(0)?),
            None => Ok(Percentage::new(DEFAULT_TARGET_MARGIN).expect("20% is a percentage")),
        }
    }

    pub async fn save_default_target_margin(&self, margin: Percentage) -> Result<(), PricingError> {
        check_target(margin)?;
        save_single_row(
            self.database.connection(),
            self.clock.as_ref(),
            self.ids.as_ref(),
            SETTINGS_TABLE,
            SETTINGS_COLUMNS,
            vec![margin.percent().to_string().into()],
        )
        .await?;
        Ok(())
    }

    /// The Product's own target margin or, without one, the default.
    pub async fn target_margin(&self, product: RecordId) -> Result<TargetMargin, PricingError> {
        let mut rows = self
            .database
            .connection()
            .query(
                "SELECT margin FROM commerce_target_margins
                 WHERE product_id = ?1 AND deleted_at IS NULL",
                params![product.to_string()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => Ok(TargetMargin {
                margin: stored_target(row.decimal_at(0)?)?,
                own: true,
            }),
            None => Ok(TargetMargin {
                margin: self.default_target_margin().await?,
                own: false,
            }),
        }
    }

    /// Sets the Product's own target margin or, with `None`, goes back to
    /// the default.
    pub async fn set_target_margin(
        &self,
        product: RecordId,
        margin: Option<Percentage>,
    ) -> Result<(), PricingError> {
        let record = Record::new(self.ids.as_ref(), self.clock.as_ref());
        let at = stored(record.created_at);
        match margin {
            Some(margin) => {
                check_target(margin)?;
                self.database
                    .connection()
                    .execute(
                        "INSERT INTO commerce_target_margins
                             (id, product_id, margin, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?4)
                         ON CONFLICT (product_id) DO UPDATE SET
                             margin = excluded.margin,
                             updated_at = excluded.updated_at,
                             deleted_at = NULL",
                        params![
                            record.id.to_string(),
                            product.to_string(),
                            margin.percent().to_string(),
                            at
                        ],
                    )
                    .await?;
            }
            None => {
                self.database
                    .connection()
                    .execute(
                        "UPDATE commerce_target_margins SET deleted_at = ?1, updated_at = ?1
                         WHERE product_id = ?2 AND deleted_at IS NULL",
                        params![at, product.to_string()],
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// The Price Suggestion for `listing`: the lowest price at which it and
    /// every other open variation of the same listing reach the target
    /// margin of the Product each sells, after the channel's sale fee at
    /// that price, shipping once free shipping is required, the tax and the
    /// unit's Average Cost. The channel is asked for its fee at today's
    /// price and again at each new price, until the fee and shipping it
    /// solved with are the ones that apply. Nothing is sent to the channel.
    pub async fn suggest(
        &self,
        listing: RecordId,
        channel: &dyn SalesChannel,
        assumptions: PriceAssumptions,
    ) -> Result<PriceSuggestion, PricingError> {
        let asked = self.listings.listing(listing).await?;
        let (category, listing_type) = fee_basis(&asked)?;
        let mut products = Vec::new();
        for sibling in self.open_variations(&asked).await? {
            let product = sibling.product.ok_or(PricingError::NotLinked(sibling.id))?;
            let cost = self.cost(product).await?;
            let target = self.target_margin(product).await?;
            products.push((sibling.id, product, cost, target));
        }

        let today = asked.listed.price;
        let today_fee = channel.sale_fee(&category, today, listing_type)?;
        let costs: Vec<(RecordId, Money, TargetMargin)> = products
            .iter()
            .map(|(_, product, cost, target)| (*product, *cost, *target))
            .collect();
        let settled = settle(
            channel,
            (&category, listing_type),
            (today, today_fee),
            &costs,
            assumptions,
        )?;
        let (product, cost, target) = costs[settled.highest];
        Ok(PriceSuggestion {
            listings: products.iter().map(|(id, ..)| *id).collect(),
            price: settled.price,
            product,
            target,
            suggested: sale(settled.price, settled.fee, cost, assumptions),
            current: sale(today, today_fee, cost, assumptions),
        })
    }

    /// The first price of a draft: the lowest, in cents, at which its
    /// Product reaches its target margin in the draft's category and
    /// Listing Type, with the unit's Average Cost or, before the Product
    /// ever came into stock, `offer_cost` (what a Supplier Offer charges).
    /// Nothing is sent to the channel.
    pub async fn draft_price(
        &self,
        draft: RecordId,
        channel: &dyn SalesChannel,
        assumptions: PriceAssumptions,
        offer_cost: Option<Money>,
    ) -> Result<DraftPrice, PricingError> {
        let draft = self.listings.draft(draft).await?;
        let category = draft.category.ok_or(PricingError::NoFeeBasis)?.id;
        let (cost, cost_from) = match self.inventory.average_cost(draft.product).await? {
            Some(cost) => (cost, CostSource::AverageCost),
            None => (
                offer_cost.ok_or(PricingError::NoCost(draft.product))?,
                CostSource::SupplierOffer,
            ),
        };
        let target = self.target_margin(draft.product).await?;
        // Any price tells the channel's fee band to start from; the cost is
        // a near one.
        let probe = Money::new(cost.amount().max(CENT), cost.currency());
        let fee = channel.sale_fee(&category, probe, draft.listing_type)?;
        let settled = settle(
            channel,
            (&category, draft.listing_type),
            (probe, fee),
            &[(draft.product, cost, target)],
            assumptions,
        )?;
        Ok(DraftPrice {
            suggested: sale(settled.price, settled.fee, cost, assumptions),
            target,
            cost_from,
        })
    }

    /// The sale of one unit of the Product `listing` sells at today's price,
    /// with the channel's fee at that price: where the price simulator
    /// starts. Nothing is sent to the channel.
    pub async fn today(
        &self,
        listing: RecordId,
        channel: &dyn SalesChannel,
        assumptions: PriceAssumptions,
    ) -> Result<PriceScenario, PricingError> {
        let asked = self.listings.listing(listing).await?;
        let (category, listing_type) = fee_basis(&asked)?;
        let product = asked.product.ok_or(PricingError::NotLinked(asked.id))?;
        let cost = self.cost(product).await?;
        let price = asked.listed.price;
        let fee = channel.sale_fee(&category, price, listing_type)?;
        Ok(sale(price, fee, cost, assumptions))
    }

    /// The Average Cost of a unit of `product`.
    async fn cost(&self, product: RecordId) -> Result<Money, PricingError> {
        self.inventory
            .average_cost(product)
            .await?
            .ok_or(PricingError::NoCost(product))
    }

    /// The Listings of the same channel listing that are still open:
    /// `listing` itself, or every open variation of it.
    async fn open_variations(&self, listing: &Listing) -> Result<Vec<Listing>, PricingError> {
        Ok(self
            .listings
            .sharing_price(&listing.listed.id)
            .await?
            .into_iter()
            .filter(|sibling| {
                sibling.id == listing.id || sibling.listed.status != ListingStatus::Closed
            })
            .collect())
    }
}

/// What the channel needs to tell its fee: the listing's category and type.
/// A closed listing sells no more, so it has no price to work out.
fn fee_basis(listing: &Listing) -> Result<(String, ListingType), PricingError> {
    if listing.listed.status == ListingStatus::Closed {
        return Err(PricingError::Closed);
    }
    match (&listing.listed.category, listing.listed.listing_type) {
        (Some(category), Some(listing_type)) => Ok((category.clone(), listing_type)),
        _ => Err(PricingError::NoFeeBasis),
    }
}

/// Where the search for a price ended: the price, the one of the costs
/// that needs it highest, and the channel's fee there.
struct Settled {
    price: Money,
    highest: usize,
    fee: SaleFee,
}

/// The lowest price at which each of `costs` (a Product, its unit cost and
/// target margin) reaches its target, starting from the channel's fee at
/// `start`. The channel is asked for its fee at each new price, until the
/// fee and shipping solved with are the ones that apply there.
fn settle(
    channel: &dyn SalesChannel,
    (category, listing_type): (&str, ListingType),
    start: (Money, SaleFee),
    costs: &[(RecordId, Money, TargetMargin)],
    assumptions: PriceAssumptions,
) -> Result<Settled, PricingError> {
    let (mut probe, mut fee) = start;
    for _ in 0..MAX_FEE_LOOKUPS {
        let mut highest: Option<(Money, usize)> = None;
        for (index, (_, cost, target)) in costs.iter().enumerate() {
            let price = sale(probe, fee, *cost, assumptions)
                .lowest_price_for(target.margin)?
                .ok_or(PricingError::Unreachable)?;
            if highest.is_none_or(|(most, _)| price.amount() > most.amount()) {
                highest = Some((price, index));
            }
        }
        let (price, index) = highest.ok_or(PricingError::Unreachable)?;
        let cost = costs[index].1;
        let fee_there = channel.sale_fee(category, price, listing_type)?;
        if fee_there == fee
            && sale(price, fee, cost, assumptions).shipping
                == sale(probe, fee, cost, assumptions).shipping
        {
            return Ok(Settled {
                price,
                highest: index,
                fee,
            });
        }
        (probe, fee) = (price, fee_there);
    }
    Err(PricingError::Unsettled)
}

/// One unit sold at `price` with no discount and no Ads: shipping once free
/// shipping is required, and the tax.
fn sale(
    price: Money,
    sale_fee: SaleFee,
    cost: Money,
    assumptions: PriceAssumptions,
) -> PriceScenario {
    PriceScenario {
        price,
        discount: Percentage::ZERO,
        sale_fee,
        shipping: shipping_paid_by_seller(price, assumptions.estimated_shipping),
        ads: Money::zero(price.currency()),
        tax: assumptions.tax,
        cost,
    }
}

/// A target margin leaves room for every Fee and the cost: under 100%.
fn check_target(margin: Percentage) -> Result<(), PricingError> {
    if margin.percent() < Decimal::ONE_HUNDRED {
        Ok(())
    } else {
        Err(PricingError::InvalidTarget)
    }
}

fn stored_target(margin: Decimal) -> Result<Percentage, PricingError> {
    Percentage::new(margin).map_err(|_| PricingError::Unreadable(margin.to_string()))
}
