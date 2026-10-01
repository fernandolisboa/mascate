use crate::{Clock, IdGenerator, RecordId, Timestamp};

/// Identity and lifecycle columns every stored record carries (ADR 0004), so
/// two databases can later be merged row by row with last write wins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: RecordId,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    pub deleted_at: Option<Timestamp>,
}

impl Record {
    pub fn new(ids: &dyn IdGenerator, clock: &dyn Clock) -> Self {
        let now = clock.now();
        Self {
            id: ids.next_id(),
            created_at: now,
            updated_at: now,
            deleted_at: None,
        }
    }

    /// Marks the record as changed. Call on every update.
    pub fn touch(&mut self, clock: &dyn Clock) {
        self.updated_at = clock.now();
    }

    /// Records are never physically deleted; deleting is an update too.
    pub fn soft_delete(&mut self, clock: &dyn Clock) {
        let now = clock.now();
        self.deleted_at = Some(now);
        self.updated_at = now;
    }

    pub fn restore(&mut self, clock: &dyn Clock) {
        self.deleted_at = None;
        self.touch(clock);
    }

    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}
