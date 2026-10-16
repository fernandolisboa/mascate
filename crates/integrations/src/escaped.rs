//! Text that goes into a URL as a path segment or a query value.

/// `text` with anything but letters, digits and `-._~` escaped, so it
/// stays one path segment or one query value and reaches no other.
pub(crate) fn escaped(text: &str) -> String {
    let mut escaped = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            escaped.push(char::from(byte));
        } else {
            escaped.push_str(&format!("%{byte:02X}"));
        }
    }
    escaped
}
