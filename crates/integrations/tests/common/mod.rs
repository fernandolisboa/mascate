//! What the tests of several adapters share.

/// The value of the first `name` parameter in `link`'s query, as written.
pub fn query_value(link: &str, name: &str) -> Option<String> {
    let before_fragment = link.split('#').next()?;
    let (_, query) = before_fragment.split_once('?')?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        (key == name).then(|| value.to_owned())
    })
}
