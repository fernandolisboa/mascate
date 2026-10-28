//! Folding words the way the owner reads them, and where each one sits in
//! the text.

use mascate_kernel::{fold, folded_word_spans, folded_words};
use proptest::prelude::*;

#[test]
fn each_folded_word_keeps_the_bytes_it_came_from() {
    let text = "Emagreça 5kg, já!";

    let spans = folded_word_spans(text);

    let read: Vec<(&str, &str)> = spans
        .iter()
        .map(|(at, word)| (&text[at.clone()], word.as_str()))
        .collect();
    assert_eq!(
        read,
        [("Emagreça", "EMAGRECA"), ("5kg", "5KG"), ("já", "JA")]
    );
}

proptest! {
    #[test]
    fn spans_give_the_folded_words_and_fold_back_to_them(text in "\\PC{0,60}") {
        let spans = folded_word_spans(&text);

        let words: Vec<String> = spans.iter().map(|(_, word)| word.clone()).collect();
        prop_assert_eq!(&words, &folded_words(&text));
        for (at, word) in spans {
            let folded: String = text[at].chars().filter_map(fold).collect();
            prop_assert_eq!(folded, word);
        }
    }
}
