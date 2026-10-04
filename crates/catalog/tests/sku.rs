use mascate_catalog::{InvalidSku, Sku};
use proptest::prelude::*;

#[test]
fn reads_what_the_owner_types_as_a_clean_sku() {
    for (typed, sku) in [
        ("fon-blu-001", "FON-BLU-001"),
        ("  fone  tws 1 ", "FONE-TWS-1"),
        ("--a--b--", "A-B"),
        ("Ação-Pá", "ACAO-PA"),
        ("COM10", "COM10"),
        ("con-1", "CON-1"),
    ] {
        assert_eq!(Sku::parse(typed).unwrap().as_str(), sku, "{typed:?}");
    }
}

#[test]
fn refuses_what_cannot_name_a_folder() {
    assert_eq!(Sku::parse(" - "), Err(InvalidSku::Empty));
    assert_eq!(Sku::parse("a/b"), Err(InvalidSku::Character('/')));
    assert_eq!(Sku::parse("..\\x"), Err(InvalidSku::Character('.')));
    assert_eq!(Sku::parse("a:b"), Err(InvalidSku::Character(':')));
    assert_eq!(Sku::parse("nul"), Err(InvalidSku::Reserved("NUL".into())));
    assert_eq!(Sku::parse("lpt9"), Err(InvalidSku::Reserved("LPT9".into())));
    assert_eq!(Sku::parse(&"A".repeat(33)), Err(InvalidSku::TooLong));
    assert!(Sku::parse(&"A".repeat(32)).is_ok());
}

proptest! {
    #[test]
    fn a_parsed_sku_is_a_safe_folder_name_and_parses_to_itself(typed in "\\PC{0,40}") {
        if let Ok(sku) = Sku::parse(&typed) {
            let text = sku.as_str();
            prop_assert!(!text.is_empty() && text.len() <= Sku::MAX_LENGTH);
            prop_assert!(text.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-'));
            prop_assert!(!text.starts_with('-') && !text.ends_with('-') && !text.contains("--"));
            prop_assert_eq!(Sku::parse(text), Ok(sku.clone()));
        }
    }
}
