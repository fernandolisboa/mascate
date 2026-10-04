//! Reading back what every module stores as text: ids, decimals, currencies
//! and times.

use std::str::FromStr;

use libsql::Row;
use mascate_kernel::{Currency, RecordId, Timestamp};
use rust_decimal::Decimal;

use crate::read_stored;

#[derive(Debug, thiserror::Error)]
pub enum StoredValueError {
    #[error("the database holds a value this app cannot read: {0}")]
    Unreadable(String),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

/// Reads one column of a row as a domain value.
pub trait StoredRow {
    fn id_at(&self, at: i32) -> Result<RecordId, StoredValueError>;
    fn optional_id_at(&self, at: i32) -> Result<Option<RecordId>, StoredValueError>;
    fn decimal_at(&self, at: i32) -> Result<Decimal, StoredValueError>;
    fn currency_at(&self, at: i32) -> Result<Currency, StoredValueError>;
    fn time_at(&self, at: i32) -> Result<Timestamp, StoredValueError>;
    fn optional_time_at(&self, at: i32) -> Result<Option<Timestamp>, StoredValueError>;
}

impl StoredRow for Row {
    fn id_at(&self, at: i32) -> Result<RecordId, StoredValueError> {
        parsed(self.get(at)?, |text| RecordId::parse_str(text).ok())
    }

    fn optional_id_at(&self, at: i32) -> Result<Option<RecordId>, StoredValueError> {
        let text: Option<String> = self.get(at)?;
        text.map(|text| parsed(text, |text| RecordId::parse_str(text).ok()))
            .transpose()
    }

    fn decimal_at(&self, at: i32) -> Result<Decimal, StoredValueError> {
        parsed(self.get(at)?, |text| Decimal::from_str(text).ok())
    }

    fn currency_at(&self, at: i32) -> Result<Currency, StoredValueError> {
        parsed(self.get(at)?, Currency::from_code)
    }

    fn time_at(&self, at: i32) -> Result<Timestamp, StoredValueError> {
        parsed(self.get(at)?, read_stored)
    }

    fn optional_time_at(&self, at: i32) -> Result<Option<Timestamp>, StoredValueError> {
        let text: Option<String> = self.get(at)?;
        text.map(|text| parsed(text, read_stored)).transpose()
    }
}

fn parsed<T>(text: String, parse: impl Fn(&str) -> Option<T>) -> Result<T, StoredValueError> {
    parse(&text).ok_or(StoredValueError::Unreadable(text))
}
