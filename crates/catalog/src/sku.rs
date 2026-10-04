use std::fmt;

/// A Product's own code, which also names its folder: upper-case letters
/// and digits in groups joined by single hyphens.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sku(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidSku {
    #[error("the SKU is empty")]
    Empty,
    #[error("the SKU has more than {} characters", Sku::MAX_LENGTH)]
    TooLong,
    #[error("the SKU may hold only letters, digits and hyphens, not {0:?}")]
    Character(char),
    #[error("Windows reserves {0} for a device, so no folder can have that name")]
    Reserved(String),
}

/// Names Windows keeps for devices, whatever the folder.
const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

impl Sku {
    pub const MAX_LENGTH: usize = 32;

    /// Reads a SKU the owner typed: accents dropped, letters in upper case,
    /// spaces and repeated hyphens as one hyphen, none at the ends.
    pub fn parse(text: &str) -> Result<Sku, InvalidSku> {
        let mut groups: Vec<String> = Vec::new();
        let mut group = String::new();
        for c in text.chars() {
            if c == '-' || c.is_whitespace() {
                groups.extend((!group.is_empty()).then(|| std::mem::take(&mut group)));
                continue;
            }
            match fold(c) {
                Some(folded) => group.push(folded),
                None => return Err(InvalidSku::Character(c)),
            }
        }
        groups.extend((!group.is_empty()).then_some(group));
        let sku = groups.join("-");
        if sku.is_empty() {
            Err(InvalidSku::Empty)
        } else if sku.len() > Sku::MAX_LENGTH {
            Err(InvalidSku::TooLong)
        } else if RESERVED.contains(&sku.as_str()) {
            Err(InvalidSku::Reserved(sku))
        } else {
            Ok(Sku(sku))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The SKU suggested for a Product named `name` before it is made
    /// unique: the first three letters of its first three words of three or
    /// more characters, as `FON-OUV-BLU` for "Fone de Ouvido Bluetooth".
    pub(crate) fn stem_for(name: &str) -> String {
        let words: Vec<String> = name
            .split(|c: char| !c.is_alphanumeric())
            .map(|word| word.chars().filter_map(fold).collect::<String>())
            .filter(|word| word.len() >= 3)
            .take(3)
            .map(|word| word[..3].to_owned())
            .collect();
        if words.is_empty() {
            "PROD".to_owned()
        } else {
            words.join("-")
        }
    }

    /// `stem` with a sequence number, as `FON-OUV-BLU-001`.
    pub(crate) fn numbered(stem: &str, number: u32) -> Sku {
        Sku(format!("{stem}-{number:03}"))
    }
}

impl fmt::Display for Sku {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An ASCII letter or digit in upper case, accents of Portuguese dropped;
/// `None` for anything else.
fn fold(c: char) -> Option<char> {
    let folded = match c.to_lowercase().next().unwrap_or(c) {
        'á' | 'à' | 'â' | 'ã' | 'ä' => 'A',
        'é' | 'è' | 'ê' | 'ë' => 'E',
        'í' | 'ì' | 'î' | 'ï' => 'I',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'O',
        'ú' | 'ù' | 'û' | 'ü' => 'U',
        'ç' => 'C',
        'ñ' => 'N',
        c if c.is_ascii_alphanumeric() => c.to_ascii_uppercase(),
        _ => return None,
    };
    Some(folded)
}
