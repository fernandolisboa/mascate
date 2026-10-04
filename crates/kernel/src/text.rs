//! Comparing names the way the owner reads them: accents, case and
//! punctuation aside.

/// An ASCII letter or digit in upper case, accents of Portuguese dropped;
/// `None` for anything else.
pub fn fold(c: char) -> Option<char> {
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

/// The words of `text`, folded: "Fone de Ouvido, Bluetooth" gives `FONE`,
/// `DE`, `OUVIDO`, `BLUETOOTH`.
pub fn folded_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .map(|word| word.chars().filter_map(fold).collect::<String>())
        .filter(|word| !word.is_empty())
        .collect()
}
