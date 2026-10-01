//! Deterministic doubles for [`Clock`] and [`IdGenerator`].

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::TimeDelta;
use uuid::Uuid;

use crate::{Clock, IdGenerator, RecordId, Timestamp};

/// A clock that only moves when told to.
#[derive(Debug)]
pub struct ManualClock(Mutex<Timestamp>);

impl ManualClock {
    pub fn at(start: Timestamp) -> Self {
        Self(Mutex::new(start))
    }

    pub fn advance(&self, by: TimeDelta) {
        *self.0.lock().expect("clock lock poisoned") += by;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Timestamp {
        *self.0.lock().expect("clock lock poisoned")
    }
}

/// Hands out 1, 2, 3, ... as UUIDs, so assertions can name expected ids.
#[derive(Debug, Default)]
pub struct SequentialIds(AtomicU64);

impl IdGenerator for SequentialIds {
    fn next_id(&self) -> RecordId {
        Uuid::from_u128(u128::from(self.0.fetch_add(1, Ordering::Relaxed)) + 1)
    }
}
