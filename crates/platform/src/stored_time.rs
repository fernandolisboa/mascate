use chrono::{DateTime, SecondsFormat, Utc};
use mascate_kernel::Timestamp;

/// A time as stored in every module's tables: a fixed-width UTC form, so stored times sort as text in time order.
pub fn stored(at: Timestamp) -> String {
    at.to_rfc3339_opts(SecondsFormat::Micros, true)
}

pub fn read_stored(text: &str) -> Option<Timestamp> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}
