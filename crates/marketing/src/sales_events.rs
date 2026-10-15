//! The Sales Events calendar (#105, ADR 0031 decision 9): the commercial
//! dates the owner plans Promotions and Offer Posts around, each with the
//! rule its date follows, how many days ahead it starts showing and notes.
//! The common Brazilian dates are recorded once, as rows like any other:
//! the owner edits or deletes them, and a deleted one never comes back.
//! Dates are days on Brasília's clock. The calendar only plans: nothing is
//! created from an event on its own.

use std::cmp::Ordering;
use std::sync::Arc;

use chrono::{Datelike, NaiveDate, TimeDelta, Weekday};
use libsql::{TransactionBehavior, params};
use mascate_kernel::{Clock, IdGenerator, Record, RecordId, Timestamp, brasilia};
use mascate_platform::{Database, Migration, StoredRow, StoredValueError, stored};

pub(crate) const CREATE_SALES_EVENTS: Migration = Migration {
    version: 10,
    name: "create the Sales Events calendar",
    risky: false,
    sql: "CREATE TABLE marketing_sales_events (
        id         TEXT PRIMARY KEY,
        name       TEXT NOT NULL,
        rule       TEXT NOT NULL,
        lead_days  INTEGER NOT NULL,
        notes      TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );
    CREATE TABLE marketing_sales_event_settings (
        id                TEXT PRIMARY KEY,
        defaults_recorded INTEGER NOT NULL,
        created_at        TEXT NOT NULL,
        updated_at        TEXT NOT NULL,
        deleted_at        TEXT
    );",
};

/// The longest name an event can have, in characters.
pub const EVENT_NAME_MAX_CHARS: usize = 60;

/// The longest notes an event can have, in characters.
pub const EVENT_NOTES_MAX_CHARS: usize = 500;

/// The most days ahead an event can start showing.
pub const MAX_LEAD_DAYS: u32 = 180;

/// The most days after the fourth Thursday of November a rule can count.
pub const MAX_DAYS_AFTER_THANKSGIVING: u8 = 30;

/// The years ahead a yearly date is looked for: 29 February skips eight
/// years around a century that is not a leap year.
const YEARS_AHEAD: i32 = 8;

/// Which day a Sales Event falls on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateRule {
    /// Every year on this day of this month; 29 February only in leap years.
    Yearly { day: u32, month: u32 },
    /// Every year on the `nth` (1 to 4) `weekday` of `month`, as Mother's
    /// Day is the second Sunday of May.
    NthWeekday {
        nth: u8,
        weekday: Weekday,
        month: u32,
    },
    /// Every year `days` after the fourth Thursday of November, the US
    /// Thanksgiving: Black Friday is 1, Cyber Monday 4.
    AfterThanksgiving { days: u8 },
    /// Once, on this day.
    Once(NaiveDate),
}

impl DateRule {
    /// The day the rule falls on in `year`, if it falls on one.
    fn date_in(self, year: i32) -> Option<NaiveDate> {
        match self {
            DateRule::Yearly { day, month } => NaiveDate::from_ymd_opt(year, month, day),
            DateRule::NthWeekday {
                nth,
                weekday,
                month,
            } => NaiveDate::from_weekday_of_month_opt(year, month, weekday, nth),
            DateRule::AfterThanksgiving { days } => {
                NaiveDate::from_weekday_of_month_opt(year, 11, Weekday::Thu, 4)
                    .map(|thanksgiving| thanksgiving + TimeDelta::days(i64::from(days)))
            }
            DateRule::Once(date) => (date.year() == year).then_some(date),
        }
    }

    /// Whether the rule falls on `date`.
    pub fn falls_on(self, date: NaiveDate) -> bool {
        self.date_in(date.year()) == Some(date)
    }

    /// The first day on or after `from` the rule falls on; `None` once a
    /// single date has passed.
    pub fn next_date(self, from: NaiveDate) -> Option<NaiveDate> {
        if let DateRule::Once(date) = self {
            return (date >= from).then_some(date);
        }
        // A date after Thanksgiving stays inside its year, so the year of
        // `from` is the first to look in.
        (from.year()..=from.year() + YEARS_AHEAD)
            .filter_map(|year| self.date_in(year))
            .find(|date| *date >= from)
    }

    /// The rule, if it names a day the calendar has.
    fn checked(self) -> Result<Self, EventError> {
        let valid = match self {
            // 2000 is a leap year, so 29 February passes.
            DateRule::Yearly { day, month } => NaiveDate::from_ymd_opt(2000, month, day).is_some(),
            DateRule::NthWeekday { nth, month, .. } => {
                (1..=4).contains(&nth) && (1..=12).contains(&month)
            }
            DateRule::AfterThanksgiving { days } => days <= MAX_DAYS_AFTER_THANKSGIVING,
            DateRule::Once(_) => true,
        };
        valid.then_some(self).ok_or(EventError::BadRule)
    }

    /// As stored: `yearly 03-15`, `weekday 2 Sun 05`, `thanksgiving 1`,
    /// `once 2026-07-08`.
    fn code(self) -> String {
        match self {
            DateRule::Yearly { day, month } => format!("yearly {month:02}-{day:02}"),
            DateRule::NthWeekday {
                nth,
                weekday,
                month,
            } => format!("weekday {nth} {weekday} {month:02}"),
            DateRule::AfterThanksgiving { days } => format!("thanksgiving {days}"),
            DateRule::Once(date) => format!("once {}", date.format("%Y-%m-%d")),
        }
    }

    fn from_code(code: &str) -> Option<Self> {
        let parts: Vec<&str> = code.split_whitespace().collect();
        let rule = match parts.as_slice() {
            ["yearly", day] => {
                let (month, day) = day.split_once('-')?;
                DateRule::Yearly {
                    day: day.parse().ok()?,
                    month: month.parse().ok()?,
                }
            }
            ["weekday", nth, weekday, month] => DateRule::NthWeekday {
                nth: nth.parse().ok()?,
                weekday: weekday.parse().ok()?,
                month: month.parse().ok()?,
            },
            ["thanksgiving", days] => DateRule::AfterThanksgiving {
                days: days.parse().ok()?,
            },
            ["once", date] => DateRule::Once(NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?),
            _ => return None,
        };
        rule.checked().ok()
    }
}

/// A commercial date on the owner's calendar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SalesEvent {
    pub id: RecordId,
    pub name: String,
    pub rule: DateRule,
    /// How many days before its date the event starts showing on Hoje.
    pub lead_days: u32,
    pub notes: String,
}

impl SalesEvent {
    /// The event's first date on or after `from`.
    pub fn next_date(&self, from: NaiveDate) -> Option<NaiveDate> {
        self.rule.next_date(from)
    }
}

/// An event as the owner writes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSalesEvent {
    pub name: String,
    pub rule: DateRule,
    pub lead_days: u32,
    pub notes: String,
}

impl NewSalesEvent {
    /// The event trimmed, if every field has a form the calendar takes.
    fn checked(&self) -> Result<NewSalesEvent, EventError> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > EVENT_NAME_MAX_CHARS {
            return Err(EventError::BadName);
        }
        if self.lead_days > MAX_LEAD_DAYS {
            return Err(EventError::BadLeadDays);
        }
        let notes = self.notes.trim();
        if notes.chars().count() > EVENT_NOTES_MAX_CHARS {
            return Err(EventError::LongNotes);
        }
        Ok(NewSalesEvent {
            name: name.to_owned(),
            rule: self.rule.checked()?,
            lead_days: self.lead_days,
            notes: notes.to_owned(),
        })
    }
}

/// An event's next date, as the owner plans for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpcomingEvent {
    pub event: SalesEvent,
    pub date: NaiveDate,
    /// Days from the day asked about to `date`; 0 on the day itself.
    pub days_away: u32,
}

impl UpcomingEvent {
    /// Whether the event is close enough to show on Hoje.
    pub fn in_lead_time(&self) -> bool {
        self.days_away <= self.event.lead_days
    }
}

/// The common Brazilian commercial dates, as the calendar starts.
pub fn default_sales_events() -> Vec<NewSalesEvent> {
    let event = |name: &str, rule: DateRule, lead_days: u32| NewSalesEvent {
        name: name.to_owned(),
        rule,
        lead_days,
        notes: String::new(),
    };
    let mut events = vec![event(
        "Dia do Consumidor",
        DateRule::Yearly { day: 15, month: 3 },
        7,
    )];
    events.extend((4..=12).map(|month| {
        event(
            &format!("Shopee {month}.{month}"),
            DateRule::Yearly { day: month, month },
            3,
        )
    }));
    events.extend([
        event(
            "Dia das Mães",
            DateRule::NthWeekday {
                nth: 2,
                weekday: Weekday::Sun,
                month: 5,
            },
            14,
        ),
        event(
            "Dia dos Pais",
            DateRule::NthWeekday {
                nth: 2,
                weekday: Weekday::Sun,
                month: 8,
            },
            14,
        ),
        event(
            "Dia das Crianças",
            DateRule::Yearly { day: 12, month: 10 },
            14,
        ),
        event("Black Friday", DateRule::AfterThanksgiving { days: 1 }, 21),
        event("Cyber Monday", DateRule::AfterThanksgiving { days: 4 }, 7),
        event("Natal", DateRule::Yearly { day: 25, month: 12 }, 21),
    ]);
    events
}

#[derive(Debug, thiserror::Error)]
pub enum EventError {
    #[error("an event needs a name of up to {EVENT_NAME_MAX_CHARS} characters")]
    BadName,
    #[error("the date rule names a day the calendar does not have")]
    BadRule,
    #[error("an event shows at most {MAX_LEAD_DAYS} days ahead")]
    BadLeadDays,
    #[error("an event's notes have at most {EVENT_NOTES_MAX_CHARS} characters")]
    LongNotes,
    #[error("no such Sales Event")]
    NotFound,
    #[error("the calendar holds a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

impl From<StoredValueError> for EventError {
    fn from(error: StoredValueError) -> Self {
        match error {
            StoredValueError::Unreadable(text) => EventError::Unreadable(text),
            StoredValueError::Sql(error) => EventError::Sql(error),
        }
    }
}

const EVENT_QUERY: &str = "SELECT id, name, rule, lead_days, notes
     FROM marketing_sales_events WHERE deleted_at IS NULL";

/// The day it is at `at` on Brasília's clock.
fn brasilia_day(at: Timestamp) -> NaiveDate {
    at.with_timezone(&brasilia()).date_naive()
}

/// The owner's Sales Events.
pub struct SalesEvents {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl SalesEvents {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            database,
            clock,
            ids,
        }
    }

    /// Records the common Brazilian dates the first time the calendar
    /// opens, and never again, so a deleted one stays deleted. Whether it
    /// recorded them now.
    pub async fn record_defaults(&self) -> Result<bool, EventError> {
        let connection = self.database.connect_for_transaction().await?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await?;
        let mut recorded = transaction
            .query(
                "SELECT 1 FROM marketing_sales_event_settings
                 WHERE deleted_at IS NULL AND defaults_recorded = 1",
                (),
            )
            .await?;
        if recorded.next().await?.is_some() {
            return Ok(false);
        }
        drop(recorded);
        for event in default_sales_events() {
            let record = Record::new(self.ids.as_ref(), self.clock.as_ref());
            transaction
                .execute(
                    "INSERT INTO marketing_sales_events
                         (id, name, rule, lead_days, notes, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                    params![
                        record.id.to_string(),
                        event.name,
                        event.rule.code(),
                        event.lead_days,
                        event.notes,
                        stored(record.created_at)
                    ],
                )
                .await?;
        }
        let record = Record::new(self.ids.as_ref(), self.clock.as_ref());
        transaction
            .execute(
                "INSERT INTO marketing_sales_event_settings
                     (id, defaults_recorded, created_at, updated_at)
                 VALUES (?1, 1, ?2, ?2)",
                params![record.id.to_string(), stored(record.created_at)],
            )
            .await?;
        transaction.commit().await?;
        Ok(true)
    }

    /// Every event, the next to come first from the day it is at `at` on
    /// Brasília's clock; single dates already past go last.
    pub async fn events(&self, at: Timestamp) -> Result<Vec<SalesEvent>, EventError> {
        let today = brasilia_day(at);
        let mut events = self.all().await?;
        events.sort_by(|a, b| {
            match (a.next_date(today), b.next_date(today)) {
                (Some(a), Some(b)) => a.cmp(&b),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            }
            .then_with(|| by_name(a, b))
        });
        Ok(events)
    }

    /// The events whose next date, from the day it is at `from` on
    /// Brasília's clock, is at most `days` away, the soonest first.
    pub async fn upcoming(
        &self,
        from: Timestamp,
        days: u32,
    ) -> Result<Vec<UpcomingEvent>, EventError> {
        let today = brasilia_day(from);
        let mut upcoming: Vec<UpcomingEvent> = self
            .all()
            .await?
            .into_iter()
            .filter_map(|event| {
                let date = event.next_date(today)?;
                let days_away = u32::try_from((date - today).num_days()).ok()?;
                (days_away <= days).then_some(UpcomingEvent {
                    event,
                    date,
                    days_away,
                })
            })
            .collect();
        upcoming.sort_by(|a, b| {
            a.date
                .cmp(&b.date)
                .then_with(|| by_name(&a.event, &b.event))
        });
        Ok(upcoming)
    }

    /// The events inside their lead time at `at`, the soonest first: what
    /// Hoje shows.
    pub async fn in_lead_time(&self, at: Timestamp) -> Result<Vec<UpcomingEvent>, EventError> {
        Ok(self
            .upcoming(at, MAX_LEAD_DAYS)
            .await?
            .into_iter()
            .filter(UpcomingEvent::in_lead_time)
            .collect())
    }

    pub async fn event(&self, id: RecordId) -> Result<SalesEvent, EventError> {
        let mut rows = self
            .database
            .connection()
            .query(
                &format!("{EVENT_QUERY} AND id = ?1"),
                params![id.to_string()],
            )
            .await?;
        match rows.next().await? {
            Some(row) => event_from(&row),
            None => Err(EventError::NotFound),
        }
    }

    pub async fn create(&self, event: &NewSalesEvent) -> Result<SalesEvent, EventError> {
        let event = event.checked()?;
        let record = Record::new(self.ids.as_ref(), self.clock.as_ref());
        self.database
            .connection()
            .execute(
                "INSERT INTO marketing_sales_events
                     (id, name, rule, lead_days, notes, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![
                    record.id.to_string(),
                    event.name,
                    event.rule.code(),
                    event.lead_days,
                    event.notes,
                    stored(record.created_at)
                ],
            )
            .await?;
        self.event(record.id).await
    }

    pub async fn update(
        &self,
        id: RecordId,
        event: &NewSalesEvent,
    ) -> Result<SalesEvent, EventError> {
        let event = event.checked()?;
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE marketing_sales_events
                 SET name = ?1, rule = ?2, lead_days = ?3, notes = ?4, updated_at = ?5
                 WHERE id = ?6 AND deleted_at IS NULL",
                params![
                    event.name,
                    event.rule.code(),
                    event.lead_days,
                    event.notes,
                    stored(self.clock.now()),
                    id.to_string()
                ],
            )
            .await?;
        if changed == 0 {
            return Err(EventError::NotFound);
        }
        self.event(id).await
    }

    pub async fn delete(&self, id: RecordId) -> Result<(), EventError> {
        let changed = self
            .database
            .connection()
            .execute(
                "UPDATE marketing_sales_events SET deleted_at = ?1, updated_at = ?1
                 WHERE id = ?2 AND deleted_at IS NULL",
                params![stored(self.clock.now()), id.to_string()],
            )
            .await?;
        if changed == 0 {
            return Err(EventError::NotFound);
        }
        Ok(())
    }

    async fn all(&self) -> Result<Vec<SalesEvent>, EventError> {
        let mut rows = self.database.connection().query(EVENT_QUERY, ()).await?;
        let mut events = Vec::new();
        while let Some(row) = rows.next().await? {
            events.push(event_from(&row)?);
        }
        Ok(events)
    }
}

fn by_name(a: &SalesEvent, b: &SalesEvent) -> Ordering {
    a.name
        .to_lowercase()
        .cmp(&b.name.to_lowercase())
        .then_with(|| a.id.cmp(&b.id))
}

fn event_from(row: &libsql::Row) -> Result<SalesEvent, EventError> {
    let rule: String = row.get(2)?;
    let lead_days: i64 = row.get(3)?;
    Ok(SalesEvent {
        id: row.id_at(0)?,
        name: row.get(1)?,
        rule: DateRule::from_code(&rule).ok_or(EventError::Unreadable(rule))?,
        lead_days: u32::try_from(lead_days)
            .map_err(|_| EventError::Unreadable(lead_days.to_string()))?,
        notes: row.get(4)?,
    })
}
