//! The port a Platform's adapter implements to tag an affiliate link with
//! the marking of a Promotion Channel (ADR 0031, decision 4). Each Platform
//! tags its own way, with the same parameter its adapter reads back into a
//! Conversion's tracking, so the Affiliate never learns their vocabulary.

use mascate_kernel::PlatformError;

use crate::source::{AffiliatePlatform, ReportedProgram};

/// Why an adapter will not tag a link.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TagRefusal {
    #[error("this is not a web link")]
    NotALink,
    /// The link leads off the Platform, which tags only its own links.
    #[error("the link is not on {}", .0.name())]
    ForeignDomain(AffiliatePlatform),
    /// The marking does not have the form the Platform takes, like an
    /// Amazon tracking id without its `-20`.
    #[error("the marking does not have the form {} takes", .0.name())]
    BadTag(AffiliatePlatform),
    /// The Platform tags the link through its API, which did not answer
    /// with one, like Shopee's `generateShortLink`.
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

/// How a Platform tags an affiliate link the owner pasted.
pub trait LinkTagger: Send + Sync {
    fn platform(&self) -> AffiliatePlatform;

    /// The one Program every link of the Platform belongs to, when the
    /// Platform has a single one, as Shopee Afiliados: the owner then
    /// creates links before the first Conversion brings the Program.
    fn program(&self) -> Option<ReportedProgram> {
        None
    }

    /// `link` with `tag` where the Platform reads it back, and nothing else
    /// in it changed.
    fn tag(&self, link: &str, tag: &str) -> Result<String, TagRefusal>;
}
