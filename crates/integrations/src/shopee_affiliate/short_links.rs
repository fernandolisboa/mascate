//! Shopee Afiliados as a `LinkTagger` (#100, ADR 0031 decision 4): the
//! owner's affiliate link for a Shopee product or shop is the short link
//! `generateShortLink` gives, with the Promotion Channel's marking as the
//! first subId, which comes back as the Conversion's tracking. Shopee's
//! terms (2.2) forbid editing an affiliate link, so the app never tags one
//! by hand: the link sent is the product's or the shop's, as pasted.

use mascate_affiliate::{AffiliatePlatform, LinkTagger, ReportedProgram, TagRefusal};
use mascate_kernel::PlatformError;
use serde::Deserialize;

use super::{ShopeeAffiliates, conversions, string};
use crate::link_tags::on_platform;

/// Where a Shopee Brasil product or shop page lives.
const PAGES: &[&str] = &["shopee.com.br"];

/// Where Shopee's short links live.
const SHORT_LINKS: &[&str] = &["shope.ee", "shp.ee", "shopee.com.br"];

/// The longest marking sent as a subId; the API publishes no limit, and a
/// Promotion Channel's marking is at most 20 characters.
const SUB_ID_MAX_CHARS: usize = 20;

impl LinkTagger for ShopeeAffiliates {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Shopee
    }

    fn program(&self) -> Option<ReportedProgram> {
        Some(conversions::program())
    }

    /// Asks Shopee for the short link of `link` with `tag` as the first
    /// subId. Shopee's help allows only letters and digits in a subId, and
    /// the report joins the subIds with dashes, so any other marking is
    /// refused before asking.
    fn tag(&self, link: &str, tag: &str) -> Result<String, TagRefusal> {
        if !is_sub_id(tag) {
            return Err(TagRefusal::BadTag(AffiliatePlatform::Shopee));
        }
        let link = link.trim();
        if !link.to_ascii_lowercase().starts_with("https://") {
            return Err(TagRefusal::NotALink);
        }
        on_platform(link, AffiliatePlatform::Shopee, PAGES)?;
        let answer: ShortLink = self.query(
            "generateShortLink",
            &format!(
                "mutation{{generateShortLink(input: {{originUrl: {}, subIds: [{}]}}){{shortLink}}}}",
                string(link),
                string(tag)
            ),
        )?;
        let short = answer.short_link.trim();
        if !short.to_ascii_lowercase().starts_with("https://")
            || on_platform(short, AffiliatePlatform::Shopee, SHORT_LINKS).is_err()
        {
            return Err(PlatformError::Failed(format!(
                "Shopee answered a short link off Shopee: {short}"
            ))
            .into());
        }
        Ok(short.to_owned())
    }
}

/// Letters and digits, up to the longest marking.
fn is_sub_id(tag: &str) -> bool {
    !tag.is_empty()
        && tag.chars().count() <= SUB_ID_MAX_CHARS
        && tag.chars().all(|c| c.is_ascii_alphanumeric())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShortLink {
    short_link: String,
}
