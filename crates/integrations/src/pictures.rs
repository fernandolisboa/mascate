//! A picture file as a `multipart/form-data` body, the way Mercado Livre
//! and Shopee take uploads.

/// The body and its `Content-Type`, which names the boundary.
pub(crate) struct PictureForm {
    pub content_type: String,
    pub body: Vec<u8>,
}

impl PictureForm {
    /// The file `file_name` with `bytes` in the form field `field`. The
    /// name goes in quotes, so quotes and line breaks are taken out of it.
    pub fn new(field: &str, file_name: &str, bytes: &[u8]) -> Self {
        let boundary = boundary_for(bytes);
        let name: String = file_name
            .chars()
            .filter(|c| !matches!(c, '"' | '\r' | '\n' | '\\'))
            .collect();
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; \
             filename=\"{name}\"\r\nContent-Type: {}\r\n\r\n",
            content_type(file_name)
        )
        .into_bytes();
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        Self {
            content_type: format!("multipart/form-data; boundary={boundary}"),
            body,
        }
    }
}

fn content_type(file_name: &str) -> &'static str {
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else {
        "image/jpeg"
    }
}

/// A multipart boundary must not occur in the file: one derived from its
/// bytes, checked against them, does not.
fn boundary_for(bytes: &[u8]) -> String {
    let mut seed = bytes.len() as u64;
    loop {
        // FNV-1a over the bytes and the seed.
        let hash = bytes
            .iter()
            .chain(seed.to_le_bytes().iter())
            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3)
            });
        let boundary = format!("mascate-{hash:016x}");
        if !contains(bytes, boundary.as_bytes()) {
            return boundary;
        }
        seed = seed.wrapping_add(1);
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
