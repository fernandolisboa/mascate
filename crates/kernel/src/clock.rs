use chrono::{DateTime, Utc};

pub type Timestamp = DateTime<Utc>;

/// Source of the current time, injected so tests control it.
pub trait Clock: Send + Sync {
    fn now(&self) -> Timestamp;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        Utc::now()
    }
}
