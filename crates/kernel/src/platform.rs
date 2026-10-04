//! What every Platform has in common, whichever role it plays: how a call
//! to it fails, how a Sales Channel exposes a listing and when the seller
//! pays for shipping.

use rust_decimal::Decimal;

use crate::Money;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    #[error("the Connection to the Platform is not set up")]
    NotConnected,
    #[error("the Platform no longer accepts the login")]
    Expired,
    #[error("the Platform asked to wait before asking again")]
    RateLimited,
    #[error("the Platform refused the request: {0}")]
    Refused(String),
    #[error("the Platform does not know it")]
    NotFound,
    #[error("could not talk to the Platform: {0}")]
    Failed(String),
}

/// How a listing is exposed in the Sales Channel; its sale fee depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ListingType {
    #[default]
    Classic,
    Premium,
}

impl ListingType {
    pub const ALL: [ListingType; 2] = [ListingType::Classic, ListingType::Premium];

    /// How it is stored. These are Mercado Livre's own ids, the first Sales
    /// Channel; stored values never change, so a new channel maps its own.
    pub fn code(self) -> &'static str {
        match self {
            ListingType::Classic => "gold_special",
            ListingType::Premium => "gold_pro",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.code() == code)
    }

    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            ListingType::Classic => "Clássico",
            ListingType::Premium => "Premium",
        }
    }
}

/// Mercado Livre makes the seller offer free shipping, and pay part of it,
/// from this sale price on; below it the buyer pays for shipping.
const FREE_SHIPPING_FROM: Decimal = Decimal::from_parts(79, 0, 0, false, 0);

/// What the seller pays to ship one unit sold at `sale_price`: `estimate`
/// once free shipping is required, nothing below that or when `estimate` is
/// in another currency than the price.
pub fn shipping_paid_by_seller(sale_price: Money, estimate: Money) -> Money {
    if sale_price.amount() >= FREE_SHIPPING_FROM && estimate.currency() == sale_price.currency() {
        estimate
    } else {
        Money::zero(sale_price.currency())
    }
}
