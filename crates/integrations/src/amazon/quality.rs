//! Listing Quality for marketing's port (ADR 0021, ADR 0033 decision 8):
//! the `issues` `getListingsItem` reports for each SKU, in Portuguese.
//! Amazon gives no score and no visits by API; the panel shows what is
//! wrong and links to the listing in Seller Central.

use mascate_kernel::{PlatformError, SalesChannelKind};
use mascate_marketing::{ActionKind, ChannelQuality, QualityAction, QualitySource};

use super::answers::{ItemIssues, ListingIssue};
use super::{Amazon, MARKETPLACE};
use crate::escaped::escaped;

const FIX_LABEL: &str = "Corrigir no Seller Central";

impl QualitySource for Amazon {
    fn rated_channel(&self) -> SalesChannelKind {
        SalesChannelKind::Amazon
    }

    /// A SKU Amazon no longer has is not rated.
    fn quality(&self, listing: &str) -> Result<Option<ChannelQuality>, PlatformError> {
        let path = format!("{}/{}", self.items_path()?, escaped(listing));
        let answer: ItemIssues = match self.get(
            &path,
            &[
                ("marketplaceIds", MARKETPLACE),
                ("includedData", "issues"),
                ("issueLocale", "pt_BR"),
            ],
        ) {
            Err(PlatformError::NotFound) => return Ok(None),
            other => other?,
        };
        let pending = answer
            .issues
            .iter()
            .filter(|issue| {
                issue.marketplace_ids.is_empty()
                    || issue.marketplace_ids.iter().any(|id| id == MARKETPLACE)
            })
            .map(|issue| action(issue, listing))
            .collect();
        Ok(Some(ChannelQuality {
            score: None,
            pending,
        }))
    }

    fn visits(&self, _: &str, _: u32) -> Result<Option<u32>, PlatformError> {
        Ok(None)
    }
}

/// An issue as marketing keeps it. An error, or anything Amazon already
/// enforces, is a problem; a warning or a note is a suggestion.
fn action(issue: &ListingIssue, sku: &str) -> QualityAction {
    let enforced: Vec<&str> = issue
        .enforcements
        .iter()
        .filter(|enforcements| {
            enforcements
                .exemption
                .as_ref()
                .is_none_or(|exemption| !exemption.status.starts_with("EXEMPT"))
        })
        .flat_map(|enforcements| &enforcements.actions)
        .map(|action| enforcement(&action.action))
        .collect();
    let mut text = issue
        .message
        .as_deref()
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .unwrap_or("A Amazon apontou um problema neste anúncio.")
        .to_owned();
    if !enforced.is_empty() {
        text.push_str(&format!(" Enquanto isso: {}.", enforced.join(", ")));
    }
    QualityAction {
        key: key(issue),
        kind: if issue.severity.as_deref() == Some("ERROR") || !enforced.is_empty() {
            ActionKind::Problem
        } else {
            ActionKind::Opportunity
        },
        text,
        label: FIX_LABEL.into(),
        link: Some(format!(
            "https://sellercentral.amazon.com.br/abis/listing/edit?marketplaceID={MARKETPLACE}&sku={}",
            escaped(sku)
        )),
    }
}

/// What keeps an issue from showing twice: Amazon's code, and the
/// attributes it is about, since one code can name several.
fn key(issue: &ListingIssue) -> String {
    let code = issue.code.as_deref().unwrap_or("");
    let attributes = issue.attribute_names.join(",");
    match (code.is_empty(), attributes.is_empty()) {
        (false, true) => code.to_owned(),
        (true, true) => issue.message.clone().unwrap_or_default(),
        _ => format!("{code}:{attributes}"),
    }
}

/// What an enforcement does to the listing, as the owner reads it.
fn enforcement(action: &str) -> &'static str {
    match action {
        "LISTING_SUPPRESSED" => "o anúncio está suspenso",
        "ATTRIBUTE_SUPPRESSED" => "um dado do anúncio foi suprimido",
        "CATALOG_ITEM_REMOVED" => "o produto saiu do catálogo",
        "SEARCH_SUPPRESSED" => "o anúncio não aparece na busca",
        _ => "a Amazon restringiu o anúncio",
    }
}
