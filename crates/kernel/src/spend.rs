//! What a Promotion Channel spent, as the Marketing records it and the
//! Affiliate and the Finance weigh it against the Commissions (ADR 0037).

use crate::{Money, RecordId};

/// What a channel spent in some days, in one currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelSpend {
    pub channel: RecordId,
    pub amount: Money,
}
