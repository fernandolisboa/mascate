//! The data a page carries for machines, read without an HTML parser: the
//! JSON-LD blocks of schema.org, a script element holding JSON (such as
//! `__NEXT_DATA__`) and an object a script assigns to a name (such as
//! `window.__PRELOADED_STATE__ = {...};`).

use serde_json::Value;

/// Every JSON-LD block on the page that reads as JSON.
pub(crate) fn json_ld(html: &str) -> Vec<Value> {
    scripts(html)
        .filter(|script| script.attributes.contains("application/ld+json"))
        .filter_map(|script| serde_json::from_str(script.content.trim()).ok())
        .collect()
}

/// The JSON inside the script element whose id is `id`.
pub(crate) fn script_json(html: &str, id: &str) -> Option<Value> {
    let quoted = [format!("id=\"{id}\""), format!("id='{id}'")];
    scripts(html)
        .find(|script| {
            quoted
                .iter()
                .any(|id| script.attributes_as_written.contains(id))
        })
        .and_then(|script| serde_json::from_str(script.content.trim()).ok())
}

/// The JSON object or array a script assigns to `name`.
pub(crate) fn assigned_json(html: &str, name: &str) -> Option<Value> {
    scripts(html).find_map(|script| {
        let after = &script.content[script.content.find(name)? + name.len()..];
        let after = after.trim_start().strip_prefix('=')?.trim_start();
        serde_json::from_str(balanced(after)?).ok()
    })
}

struct Script<'a> {
    /// In lower case, for matching types.
    attributes: String,
    attributes_as_written: &'a str,
    content: &'a str,
}

fn scripts(html: &str) -> impl Iterator<Item = Script<'_>> {
    // ASCII lower case keeps every byte where it was.
    let lower = html.to_ascii_lowercase();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(start) = lower[from..].find("<script").map(|at| from + at) {
        let Some(open_end) = lower[start..].find('>').map(|at| start + at) else {
            break;
        };
        let Some(close) = lower[open_end..].find("</script").map(|at| open_end + at) else {
            break;
        };
        let attributes_as_written = &html[start + "<script".len()..open_end];
        found.push(Script {
            attributes: attributes_as_written.to_ascii_lowercase(),
            attributes_as_written,
            content: &html[open_end + 1..close],
        });
        from = close;
    }
    found.into_iter()
}

/// The JSON object or array `text` starts with, up to its closing bracket.
fn balanced(text: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (at, byte) in text.bytes().enumerate() {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'{' | b'[' => depth += 1,
            b'}' | b']' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&text[..=at]);
                }
            }
            _ if depth == 0 => return None,
            _ => {}
        }
    }
    None
}
