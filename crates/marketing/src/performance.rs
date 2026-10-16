//! A Sales Channel's rating of the whole shop (#136): Shopee's shop
//! performance, its overall standing and each metric against the target
//! the channel sets. It is read with the Listing Quality and shown on the
//! same panel; only the channel's own reading is kept, never worked out
//! here.

use std::collections::{BTreeMap, HashSet};

use libsql::{Connection, params};
use mascate_kernel::{IdGenerator, RecordId, SalesChannelKind, Timestamp};
use mascate_platform::{StoredRow, stored};
use rust_decimal::Decimal;

use crate::QualityError;

/// The channel's overall rating of the shop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PerformanceRating {
    Poor,
    ImprovementNeeded,
    Good,
    Excellent,
}

impl PerformanceRating {
    const ALL: [PerformanceRating; 4] = [
        PerformanceRating::Poor,
        PerformanceRating::ImprovementNeeded,
        PerformanceRating::Good,
        PerformanceRating::Excellent,
    ];

    fn code(self) -> &'static str {
        match self {
            PerformanceRating::Poor => "poor",
            PerformanceRating::ImprovementNeeded => "improvement_needed",
            PerformanceRating::Good => "good",
            PerformanceRating::Excellent => "excellent",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|rating| rating.code() == code)
    }

    /// As Shopee Brasil names it to the seller.
    pub fn name(self) -> &'static str {
        match self {
            PerformanceRating::Poor => "Ruim",
            PerformanceRating::ImprovementNeeded => "Precisa melhorar",
            PerformanceRating::Good => "Bom",
            PerformanceRating::Excellent => "Excelente",
        }
    }
}

/// What part of the selling a metric measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MetricGroup {
    Fulfillment,
    Listings,
    CustomerService,
}

impl MetricGroup {
    const ALL: [MetricGroup; 3] = [
        MetricGroup::Fulfillment,
        MetricGroup::Listings,
        MetricGroup::CustomerService,
    ];

    fn code(self) -> &'static str {
        match self {
            MetricGroup::Fulfillment => "fulfillment",
            MetricGroup::Listings => "listings",
            MetricGroup::CustomerService => "customer_service",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|group| group.code() == code)
    }

    pub fn name(self) -> &'static str {
        match self {
            MetricGroup::Fulfillment => "Envio",
            MetricGroup::Listings => "Anúncios",
            MetricGroup::CustomerService => "Atendimento",
        }
    }
}

/// What a metric's number counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MetricUnit {
    Number,
    Percent,
    Seconds,
    Days,
    Hours,
}

impl MetricUnit {
    const ALL: [MetricUnit; 5] = [
        MetricUnit::Number,
        MetricUnit::Percent,
        MetricUnit::Seconds,
        MetricUnit::Days,
        MetricUnit::Hours,
    ];

    fn code(self) -> &'static str {
        match self {
            MetricUnit::Number => "number",
            MetricUnit::Percent => "percent",
            MetricUnit::Seconds => "seconds",
            MetricUnit::Days => "days",
            MetricUnit::Hours => "hours",
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|unit| unit.code() == code)
    }
}

/// How a metric's value compares with its target to meet it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Comparator {
    Below,
    AtMost,
    Above,
    AtLeast,
    Equal,
}

impl Comparator {
    const ALL: [Comparator; 5] = [
        Comparator::Below,
        Comparator::AtMost,
        Comparator::Above,
        Comparator::AtLeast,
        Comparator::Equal,
    ];

    /// The sign the channel writes it with.
    pub fn sign(self) -> &'static str {
        match self {
            Comparator::Below => "<",
            Comparator::AtMost => "<=",
            Comparator::Above => ">",
            Comparator::AtLeast => ">=",
            Comparator::Equal => "=",
        }
    }

    pub fn from_sign(sign: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|comparator| comparator.sign() == sign.trim())
    }

    fn holds(self, value: Decimal, target: Decimal) -> bool {
        match self {
            Comparator::Below => value < target,
            Comparator::AtMost => value <= target,
            Comparator::Above => value > target,
            Comparator::AtLeast => value >= target,
            Comparator::Equal => value == target,
        }
    }
}

/// The value a metric is held to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetricTarget {
    pub value: Decimal,
    pub comparator: Comparator,
}

/// One of the channel's metrics of the shop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerformanceMetric {
    /// The channel's id of the metric: what keeps it from showing twice.
    pub key: String,
    pub group: MetricGroup,
    /// In the channel's words.
    pub name: String,
    /// In the period running now; `None` while there is nothing to measure.
    pub current: Option<Decimal>,
    /// In the period before.
    pub previous: Option<Decimal>,
    pub unit: MetricUnit,
    pub target: Option<MetricTarget>,
}

impl PerformanceMetric {
    /// Whether the current value meets the target; `None` without either.
    pub fn meets_target(&self) -> Option<bool> {
        let target = self.target?;
        Some(target.comparator.holds(self.current?, target.value))
    }
}

/// The shop as the channel last rated it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelPerformance {
    /// `None` while the channel has no overall rating for the shop.
    pub rating: Option<PerformanceRating>,
    /// In the channel's order.
    pub metrics: Vec<PerformanceMetric>,
}

impl ChannelPerformance {
    /// The metrics that miss their target.
    pub fn below_target(&self) -> usize {
        self.metrics
            .iter()
            .filter(|metric| metric.meets_target() == Some(false))
            .count()
    }
}

/// A channel's rating of the shop, as last read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShopPerformance {
    pub channel: SalesChannelKind,
    pub performance: ChannelPerformance,
    pub checked_at: Timestamp,
}

/// Writes `channel`'s rating of the shop over the last one: each metric by
/// the channel's id, and those it no longer reports gone.
pub(crate) async fn write_performance(
    on: &Connection,
    ids: &dyn IdGenerator,
    channel: SalesChannelKind,
    performance: &ChannelPerformance,
    now: Timestamp,
) -> Result<(), QualityError> {
    on.execute(
        "INSERT INTO marketing_shop_performance
             (id, sales_channel, rating, checked_at, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4, ?4)
         ON CONFLICT (sales_channel) WHERE deleted_at IS NULL
         DO UPDATE SET rating = excluded.rating, checked_at = excluded.checked_at,
             updated_at = excluded.updated_at",
        params![
            ids.next_id().to_string(),
            channel.code(),
            performance.rating.map(PerformanceRating::code),
            stored(now)
        ],
    )
    .await?;
    let mut rows = on
        .query(
            "SELECT id FROM marketing_shop_performance
             WHERE sales_channel = ?1 AND deleted_at IS NULL",
            params![channel.code()],
        )
        .await?;
    let performance_id = rows
        .next()
        .await?
        .ok_or_else(|| QualityError::Unreadable(channel.code().to_owned()))?
        .id_at(0)?;

    let mut kept = HashSet::new();
    for (position, metric) in performance.metrics.iter().enumerate() {
        if !kept.insert(metric.key.as_str()) {
            continue;
        }
        on.execute(
            "INSERT INTO marketing_performance_metrics
                 (id, performance_id, channel_metric_id, metric_group, name, current_value,
                  previous_value, unit, target_value, comparator, position, created_at,
                  updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
             ON CONFLICT (performance_id, channel_metric_id) WHERE deleted_at IS NULL
             DO UPDATE SET metric_group = excluded.metric_group, name = excluded.name,
                 current_value = excluded.current_value,
                 previous_value = excluded.previous_value, unit = excluded.unit,
                 target_value = excluded.target_value, comparator = excluded.comparator,
                 position = excluded.position, updated_at = excluded.updated_at",
            params![
                ids.next_id().to_string(),
                performance_id.to_string(),
                metric.key.clone(),
                metric.group.code(),
                metric.name.clone(),
                metric.current.map(|value| value.to_string()),
                metric.previous.map(|value| value.to_string()),
                metric.unit.code(),
                metric.target.map(|target| target.value.to_string()),
                metric.target.map(|target| target.comparator.sign()),
                position as i64,
                stored(now)
            ],
        )
        .await?;
    }
    let mut rows = on
        .query(
            "SELECT id, channel_metric_id FROM marketing_performance_metrics
             WHERE performance_id = ?1 AND deleted_at IS NULL",
            params![performance_id.to_string()],
        )
        .await?;
    let mut gone = Vec::new();
    while let Some(row) = rows.next().await? {
        if !kept.contains(row.get::<String>(1)?.as_str()) {
            gone.push(row.id_at(0)?);
        }
    }
    for id in gone {
        on.execute(
            "UPDATE marketing_performance_metrics SET deleted_at = ?1, updated_at = ?1
             WHERE id = ?2",
            params![stored(now), id.to_string()],
        )
        .await?;
    }
    Ok(())
}

/// Every channel's rating of the shop, in the channels' order.
pub(crate) async fn read_performances(
    on: &Connection,
) -> Result<Vec<ShopPerformance>, QualityError> {
    let mut metrics: BTreeMap<RecordId, Vec<(i64, PerformanceMetric)>> = BTreeMap::new();
    let mut rows = on
        .query(
            "SELECT performance_id, channel_metric_id, metric_group, name, current_value,
                 previous_value, unit, target_value, comparator, position
             FROM marketing_performance_metrics WHERE deleted_at IS NULL",
            (),
        )
        .await?;
    while let Some(row) = rows.next().await? {
        let target = match (
            optional_decimal(row.get(7)?)?,
            row.get::<Option<String>>(8)?,
        ) {
            (Some(value), Some(sign)) => Some(MetricTarget {
                value,
                comparator: Comparator::from_sign(&sign).ok_or(QualityError::Unreadable(sign))?,
            }),
            _ => None,
        };
        let group = row.get::<String>(2)?;
        let unit = row.get::<String>(6)?;
        metrics.entry(row.id_at(0)?).or_default().push((
            row.get(9)?,
            PerformanceMetric {
                key: row.get(1)?,
                group: MetricGroup::from_code(&group).ok_or(QualityError::Unreadable(group))?,
                name: row.get(3)?,
                current: optional_decimal(row.get(4)?)?,
                previous: optional_decimal(row.get(5)?)?,
                unit: MetricUnit::from_code(&unit).ok_or(QualityError::Unreadable(unit))?,
                target,
            },
        ));
    }

    let mut performances = Vec::new();
    let mut rows = on
        .query(
            "SELECT id, sales_channel, rating, checked_at
             FROM marketing_shop_performance WHERE deleted_at IS NULL",
            (),
        )
        .await?;
    while let Some(row) = rows.next().await? {
        let rating = match row.get::<Option<String>>(2)? {
            Some(code) => {
                Some(PerformanceRating::from_code(&code).ok_or(QualityError::Unreadable(code))?)
            }
            None => None,
        };
        let mut kept = metrics.remove(&row.id_at(0)?).unwrap_or_default();
        kept.sort_by_key(|(position, _)| *position);
        performances.push(ShopPerformance {
            channel: row.sales_channel_at(1)?,
            performance: ChannelPerformance {
                rating,
                metrics: kept.into_iter().map(|(_, metric)| metric).collect(),
            },
            checked_at: row.time_at(3)?,
        });
    }
    performances.sort_by_key(|shop| shop.channel);
    Ok(performances)
}

fn optional_decimal(text: Option<String>) -> Result<Option<Decimal>, QualityError> {
    text.map(|text| text.parse().map_err(|_| QualityError::Unreadable(text)))
        .transpose()
}
