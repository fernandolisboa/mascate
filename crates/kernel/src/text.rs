//! Comparing names the way the owner reads them: accents, case and
//! punctuation aside.

use std::ops::Range;

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
    folded_word_spans(text)
        .into_iter()
        .map(|(_, word)| word)
        .collect()
}

/// The words of `text`, folded as in [`folded_words`], each with the bytes
/// of `text` it came from.
pub fn folded_word_spans(text: &str) -> Vec<(Range<usize>, String)> {
    let mut words = Vec::new();
    let mut start = None;
    for (at, c) in text.char_indices().chain([(text.len(), ' ')]) {
        match (c.is_alphanumeric(), start) {
            (true, None) => start = Some(at),
            (false, Some(from)) => {
                let word: String = text[from..at].chars().filter_map(fold).collect();
                if !word.is_empty() {
                    words.push((from..at, word));
                }
                start = None;
            }
            _ => {}
        }
    }
    words
}
