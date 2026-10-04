//! Building blocks shared by every module. Nothing here knows about the UI,
//! the database or any Platform.

mod clock;
mod ids;
mod money;
mod record;

pub use clock::{Clock, SystemClock, Timestamp};
pub use ids::{IdGenerator, RecordId, UuidV7Generator};
pub use money::{Currency, CurrencyMismatch, Money, parse_amount};
pub use record::Record;

#[cfg(feature = "test-support")]
pub mod testing;
