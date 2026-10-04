/// Where a Supplier Offer is on the web, as the owner pasted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferLink {
    url: String,
    /// The same for every paste of the same item, so new prices join its
    /// history.
    key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidLink {
    #[error("the link is empty")]
    Empty,
    #[error("the link must start with http:// or https://")]
    NotWeb,
    #[error("the link has no site")]
    NoHost,
    #[error("the link has a space in it")]
    Space,
}

/// Marketplaces that name the item in the path and add tracking to the
/// query, which changes on every share of the same item.
const PATH_IDENTIFIES_ITEM: [&str; 10] = [
    "shopee.com.br",
    "mercadolivre.com.br",
    "mercadolibre.com",
    "aliexpress.com",
    "aliexpress.us",
    "amazon.com.br",
    "amazon.com",
    "magazineluiza.com.br",
    "shein.com",
    "temu.com",
];

impl OfferLink {
    pub fn parse(text: &str) -> Result<OfferLink, InvalidLink> {
        let url = text.trim();
        if url.is_empty() {
            return Err(InvalidLink::Empty);
        }
        if url.chars().any(char::is_whitespace) {
            return Err(InvalidLink::Space);
        }
        let scheme_end = url.find("://").ok_or(InvalidLink::NotWeb)?;
        let scheme = url[..scheme_end].to_ascii_lowercase();
        if scheme != "http" && scheme != "https" {
            return Err(InvalidLink::NotWeb);
        }
        let rest = &url[scheme_end + 3..];
        let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let host = rest[..host_end].to_ascii_lowercase();
        if host.is_empty() {
            return Err(InvalidLink::NoHost);
        }
        let rest = &rest[host_end..];
        let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let path = path.trim_end_matches('/');
        let name = host.split(':').next().unwrap_or(&host);
        let query: Vec<&str> = if PATH_IDENTIFIES_ITEM
            .iter()
            .any(|site| name == *site || name.ends_with(&format!(".{site}")))
        {
            Vec::new()
        } else {
            query
                .split('&')
                .filter(|pair| !pair.is_empty() && !pair.starts_with("utm_"))
                .collect()
        };
        let mut key = format!("{host}{path}");
        if !query.is_empty() {
            key.push('?');
            key.push_str(&query.join("&"));
        }
        Ok(OfferLink {
            url: url.to_owned(),
            key,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.url
    }

    pub(crate) fn key(&self) -> &str {
        &self.key
    }
}
