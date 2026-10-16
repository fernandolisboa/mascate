//! What every crawler that searches a site for offers shares: the words a
//! search goes out with, and the amounts, counts and stars read from what a
//! page carries for people and for machines.

use std::str::FromStr;

use mascate_catalog::OfferSearch;
use mascate_kernel::{Currency, Money, PlatformError, folded_words};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::Value;

use crate::answers::decimal;

/// The words of `search` without accents and in lower case, for `site`'s
/// search address. The crawlers search only by words: a category or a shop
/// is refused rather than ignored.
pub(crate) fn search_words(search: &OfferSearch, site: &str) -> Result<Vec<String>, PlatformError> {
    if search
        .category
        .as_deref()
        .is_some_and(|category| !category.trim().is_empty())
    {
        return Err(PlatformError::Refused(format!(
            "o crawler {site} não filtra por categoria"
        )));
    }
    if search.shop.is_some() {
        return Err(PlatformError::Refused(format!(
            "o crawler {site} não busca por loja"
        )));
    }
    let words: Vec<String> = folded_words(&search.keyword)
        .into_iter()
        .map(|word| word.to_ascii_lowercase())
        .collect();
    if words.is_empty() {
        return Err(PlatformError::Refused("digite o que procurar".into()));
    }
    Ok(words)
}

/// Why `site`'s crawler finds no shops.
pub(crate) fn no_shops(site: &str) -> PlatformError {
    PlatformError::Refused(format!("o crawler {site} não busca lojas"))
}

/// Calls `visit` on `value` and on every object and array inside it.
pub(crate) fn walk(value: &Value, visit: &mut impl FnMut(&Value)) {
    visit(value);
    match value {
        Value::Object(fields) => fields.values().for_each(|inner| walk(inner, visit)),
        Value::Array(values) => values.iter().for_each(|inner| walk(inner, visit)),
        _ => {}
    }
}

/// Whether a JSON-LD node is a schema.org Product.
pub(crate) fn is_product(value: &Value) -> bool {
    match value.get("@type") {
        Some(Value::String(kind)) => kind == "Product",
        Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == "Product"),
        _ => false,
    }
}

/// An amount written as a JSON number or text, in `currency` (reais when
/// the page does not say); never negative.
pub(crate) fn money(amount: &Value, currency: Option<&Value>) -> Option<Money> {
    let currency = match currency.and_then(Value::as_str) {
        Some(code) => Currency::from_code(code)?,
        None => Currency::Brl,
    };
    number_in(amount)
        .filter(|amount| !amount.is_sign_negative())
        .map(|amount| Money::new(amount, currency))
}

/// A number written as a JSON number or as text with a decimal point.
pub(crate) fn number_in(value: &Value) -> Option<Decimal> {
    match value {
        Value::Number(number) => decimal(number),
        Value::String(text) => Decimal::from_str(text.trim()).ok(),
        _ => None,
    }
}

/// Stars from 0 to 5 at the start of a line such as `"4.8 | +10mil
/// vendidos"` or `"4,5 de 5 estrelas"`.
pub(crate) fn stars(text: &str) -> Option<Decimal> {
    let first = text.split(['|', ' ', '(']).find(|word| !word.is_empty())?;
    Decimal::from_str(&first.replace(',', "."))
        .ok()
        .filter(|stars| (Decimal::ZERO..=Decimal::from(5)).contains(stars))
}

/// The count just before `word` in a line such as `"+10mil vendidos"`,
/// `"1.234 vendidos"` or `"Mais de 1 mil compras no mês passado"`; 0 when
/// the line does not say.
pub(crate) fn count_before(text: &str, word: &str) -> u32 {
    let lower = text.to_lowercase();
    let Some(before) = lower.find(word).map(|at| &lower[..at]) else {
        return 0;
    };
    let count = before.rsplit('|').next().unwrap_or_default().trim();
    let count = count.strip_prefix("mais de").unwrap_or(count);
    let count = count.trim_start_matches(|c: char| c == '+' || c.is_whitespace());
    let digits: String = count
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .collect();
    let unit = count[digits.len()..].trim();
    let factor = if unit.starts_with("mil") && !unit.starts_with("milh") {
        1_000
    } else if unit.starts_with("mi") {
        1_000_000
    } else {
        1
    };
    let number = if factor == 1 {
        digits.replace(['.', ','], "")
    } else {
        digits.replace('.', "").replace(',', ".")
    };
    Decimal::from_str(&number)
        .ok()
        .map(|number| number * Decimal::from(factor))
        .and_then(|units| units.trunc().to_u32())
        .unwrap_or(0)
}
