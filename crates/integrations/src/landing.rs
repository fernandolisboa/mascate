//! The address an OAuth login landed on, pasted back by the owner: the app
//! has no address of its own for the Platform to send the browser to.

/// The one-time `code` in the address the browser landed on, or in its
/// query alone; `None` when it is missing or malformed.
pub(crate) fn code_in(address: &str) -> Option<String> {
    let address = address.trim();
    let query = address.split_once('?').map_or(address, |(_, query)| query);
    let query = query.split('#').next().unwrap_or_default();
    let code = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, _)| *key == "code")
        .map(|(_, value)| value)?;
    let well_formed = !code.is_empty()
        && code.len() <= 4096
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte));
    well_formed.then(|| code.to_owned())
}
