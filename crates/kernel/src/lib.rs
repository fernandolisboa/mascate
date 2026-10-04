//! Building blocks shared by every module. Nothing here knows about the UI
//! or the database.

mod clock;
mod ids;
mod margin;
mod money;
mod platform;
mod record;
mod text;

pub use clock::{Clock, SystemClock, Timestamp};
pub use ids::{IdGenerator, RecordId, UuidV7Generator};
pub use margin::{Margin, OutOfRange, Percentage};
pub use money::{Currency, CurrencyMismatch, Money, parse_amount};
pub use platform::{ListingType, PlatformError, shipping_paid_by_seller};
pub use record::Record;
pub use text::{fold, folded_words};

#[cfg(feature = "test-support")]
pub mod testing;
