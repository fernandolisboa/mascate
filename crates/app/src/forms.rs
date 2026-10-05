//! Form pieces the screens share: text fields, pickers of records and the
//! outcome of the last action.

use chrono::{Datelike as _, NaiveDate};
use gpui_kit::component::input::InputState;
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::SelectState;
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Entity, SharedString, Window};
use mascate_kernel::{Money, Percentage, RecordId};

use crate::kit;

/// One line of a picker of records.
#[derive(Clone)]
pub struct Choice {
    pub id: RecordId,
    pub title: SharedString,
}

impl SearchableListItem for Choice {
    type Value = RecordId;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &RecordId {
        &self.id
    }
}

pub type Picker = Entity<SelectState<SearchableVec<Choice>>>;

pub fn input<V: 'static>(
    placeholder: &'static str,
    window: &mut Window,
    cx: &mut Context<V>,
) -> Entity<InputState> {
    cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
}

pub fn picker<V: 'static>(window: &mut Window, cx: &mut Context<V>) -> Picker {
    cx.new(|cx| {
        SelectState::new(SearchableVec::new(Vec::<Choice>::new()), None, window, cx)
            .searchable(true)
    })
}

/// Puts `choices` in a picker, keeping what was picked if it is still there.
pub fn refill(picker: &Picker, choices: Vec<Choice>, window: &mut Window, cx: &mut App) {
    picker.update(cx, |select, cx| {
        let picked = select.selected_value().copied();
        select.set_items(SearchableVec::new(choices), window, cx);
        match picked {
            Some(picked) => select.set_selected_value(&picked, window, cx),
            None => select.set_selected_index(None, window, cx),
        }
    });
}

/// The outcome of the last action.
pub enum Outcome {
    Done(SharedString),
    Failed(SharedString),
}

pub fn notice(outcome: &Outcome, cx: &App) -> AnyElement {
    match outcome {
        Outcome::Done(text) => kit::success_notice(text.clone(), cx).into_any_element(),
        Outcome::Failed(text) => kit::error_notice(text.clone(), cx).into_any_element(),
    }
}

/// "14", "12,5": a rate as the owner types it.
pub fn percent_text(rate: Percentage) -> String {
    rate.percent().normalize().to_string().replace('.', ",")
}

/// A day as the owner types it: `01/10/2026`, `1/10/26` or `2026-10-01`.
pub fn parse_day(text: &str) -> Option<NaiveDate> {
    let text = text.trim();
    ["%d/%m/%Y", "%Y-%m-%d"]
        .iter()
        .find_map(|format| NaiveDate::parse_from_str(text, format).ok())
        .filter(|date| date.year() >= 2000)
        .or_else(|| NaiveDate::parse_from_str(text, "%d/%m/%y").ok())
}

/// "05/10/2026": a day as the owner types it back.
pub fn day_text(day: NaiveDate) -> String {
    day.format("%d/%m/%Y").to_string()
}

/// "20,00": an amount as the owner types it.
pub fn amount_text(amount: Money) -> String {
    format!("{:.2}", amount.rounded().amount()).replace('.', ",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_days_as_brazilians_type_them() {
        let first_of_october = NaiveDate::from_ymd_opt(2026, 10, 1);
        assert_eq!(parse_day("01/10/2026"), first_of_october);
        assert_eq!(parse_day(" 1/10/2026 "), first_of_october);
        assert_eq!(parse_day("1/10/26"), first_of_october);
        assert_eq!(parse_day("2026-10-01"), first_of_october);
        assert_eq!(parse_day("31/02/2026"), None);
        assert_eq!(parse_day("10/2026"), None);
        assert_eq!(day_text(first_of_october.unwrap()), "01/10/2026");
    }
}
