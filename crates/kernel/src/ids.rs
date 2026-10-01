use uuid::Uuid;

/// Every record is identified by a UUIDv7 (ADR 0004): unique across machines
/// and roughly ordered by creation time.
pub type RecordId = Uuid;

/// Source of new record ids, injected so tests control it.
pub trait IdGenerator: Send + Sync {
    fn next_id(&self) -> RecordId;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct UuidV7Generator;

impl IdGenerator for UuidV7Generator {
    fn next_id(&self) -> RecordId {
        Uuid::now_v7()
    }
}
