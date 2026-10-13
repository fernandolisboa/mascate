//! Tagging an affiliate link through its query string (#99, ADR 0031
//! decision 4), for the Platforms that read the marking from a URL
//! parameter. Each Platform's adapter declares its parameter and domains;
//! the link the owner pasted changes only in that parameter, since a
//! Platform may refuse a commission on a link altered otherwise.

use mascate_affiliate::{AffiliatePlatform, LinkTagger, TagRefusal};

/// How a Platform reads the marking from its links.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UrlTagger {
    pub platform: AffiliatePlatform,
    /// The query parameter the Platform reports back on the sale.
    pub parameter: &'static str,
    /// The Platform's domains; their subdomains count too.
    pub domains: &'static [&'static str],
    /// The end a marking must have there, like Amazon's `-20`.
    pub tag_ends_with: Option<&'static str>,
}

impl LinkTagger for UrlTagger {
    fn platform(&self) -> AffiliatePlatform {
        self.platform
    }

    fn tag(&self, link: &str, tag: &str) -> Result<String, TagRefusal> {
        if self
            .tag_ends_with
            .is_some_and(|end| !tag.ends_with(end) || tag.len() == end.len())
        {
            return Err(TagRefusal::BadTag(self.platform));
        }
        let link = link.trim();
        on_platform(link, self.platform, self.domains)?;
        Ok(with_parameter(link, self.parameter, tag))
    }
}

/// Whether `link` is a web link on one of `platform`'s `domains` or their
/// subdomains.
pub(crate) fn on_platform(
    link: &str,
    platform: AffiliatePlatform,
    domains: &[&str],
) -> Result<(), TagRefusal> {
    let host = host(link).ok_or(TagRefusal::NotALink)?;
    let on_platform = domains.iter().any(|domain| {
        host == *domain
            || host
                .strip_suffix(domain)
                .is_some_and(|sub| sub.ends_with('.'))
    });
    if !on_platform {
        return Err(TagRefusal::ForeignDomain(platform));
    }
    Ok(())
}

/// The host of an `http` or `https` link, lowercase. A link with spaces,
/// without a host, with a user before the host (which hides the real one,
/// as in `https://hotmart.com@other.example`) or with anything but letters,
/// digits, dots and hyphens in it has none.
fn host(link: &str) -> Option<String> {
    if link.chars().any(char::is_whitespace) {
        return None;
    }
    let lower = link.to_ascii_lowercase();
    let rest = ["https://", "http://"]
        .into_iter()
        .find_map(|scheme| lower.strip_prefix(scheme))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return None;
    }
    let host = authority
        .split(':')
        .next()
        .unwrap_or_default()
        .trim_end_matches('.');
    let plain = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '-';
    (!host.is_empty() && host.chars().all(plain)).then(|| host.to_owned())
}

/// `link` with `parameter` set to `value`: in the place of its first
/// occurrence, any other dropped, or added at the end of the query. Every
/// other parameter and the fragment stay as they were, byte for byte.
fn with_parameter(link: &str, parameter: &str, value: &str) -> String {
    let (before_fragment, fragment) = match link.split_once('#') {
        Some((before, fragment)) => (before, Some(fragment)),
        None => (link, None),
    };
    let (base, query) = match before_fragment.split_once('?') {
        Some((base, query)) if !query.is_empty() => (base, Some(query)),
        Some((base, _)) => (base, None),
        None => (before_fragment, None),
    };
    let tagged = format!("{parameter}={value}");
    let mut pairs: Vec<&str> = Vec::new();
    let mut placed = false;
    for pair in query.into_iter().flat_map(|query| query.split('&')) {
        if pair.split('=').next() == Some(parameter) {
            if !placed {
                pairs.push(&tagged);
                placed = true;
            }
        } else {
            pairs.push(pair);
        }
    }
    if !placed {
        pairs.push(&tagged);
    }
    let mut result = format!("{base}?{}", pairs.join("&"));
    if let Some(fragment) = fragment {
        result.push('#');
        result.push_str(fragment);
    }
    result
}
