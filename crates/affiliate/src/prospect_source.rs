//! The port a Platform's reader implements to find Prospects (ADR 0039,
//! decision 2): the public vitrine or the logged-in affiliate market. It
//! only reads; the Affiliate keeps what comes back by the Platform's
//! natural key.

use mascate_kernel::{PlatformError, Timestamp};

use crate::prospect_signals::{SignalOrigin, Signals};
use crate::source::AffiliatePlatform;

/// A category of a Platform's market.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProspectCategory {
    /// As the Platform names it in its addresses.
    pub id: String,
    pub name: String,
}

/// A product as a Platform shows it, or as the owner typed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundProspect {
    pub platform: AffiliatePlatform,
    /// The product's id on the Platform: its natural key.
    pub key: String,
    pub name: String,
    pub signals: Signals,
}

/// How a Platform shows its products to affiliates. Calls block on the
/// network, so they run off the UI thread.
pub trait ProspectSource: Send + Sync {
    fn platform(&self) -> AffiliatePlatform;

    /// Where what it reads comes from.
    fn origin(&self) -> SignalOrigin;

    fn categories(&self) -> Result<Vec<ProspectCategory>, PlatformError>;

    /// The products of `category`, or the best sellers without one.
    fn prospects(
        &self,
        category: Option<&ProspectCategory>,
    ) -> Result<Vec<FoundProspect>, PlatformError>;

    /// One product's page, with what the list does not show. `known` is
    /// what the app keeps of it, such as the address of its page.
    fn details(&self, known: &FoundProspect) -> Result<FoundProspect, PlatformError>;

    /// Until when the Platform is left alone after blocking the reader;
    /// `None` while it may be asked.
    fn blocked_until(&self) -> Option<Timestamp>;
}
