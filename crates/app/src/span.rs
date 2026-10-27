//! The days a screen sums up: the last 7, 30 or 90 days, or the days the
//! owner types (#28, #81). The picker tells its screen when the days change.

use std::ops::{Range, RangeInclusive};

use chrono::{Local, NaiveDate, TimeDelta};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{Entity, EventEmitter, SharedString, Window, div, px};
use mascate_kernel::Timestamp;

use crate::appearance::look;
use crate::forms::{day_text, input, parse_day};
use crate::sales::day_start;

/// The days summed up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Span {
    /// The last few days, today included.
    Days(u16),
    /// The days the owner typed, both included.
    Custom,
}

impl Span {
    const ALL: [Span; 4] = [Span::Days(7), Span::Days(30), Span::Days(90), Span::Custom];

    fn name(self) -> SharedString {
        match self {
            Span::Days(days) => format!("{days} dias").into(),
            Span::Custom => "Personalizado".into(),
        }
    }
}

/// From the start of the first day to the end of the last, by the
/// computer's calendar.
fn time_of(days: RangeInclusive<NaiveDate>) -> Range<Timestamp> {
    day_start(*days.start())..day_start(*days.end() + TimeDelta::days(1))
}

/// The last `count` days, `today` included.
fn last_days(count: u16, today: NaiveDate) -> RangeInclusive<NaiveDate> {
    today - TimeDelta::days(i64::from(count) - 1)..=today
}

/// The days changed.
pub struct SpanChanged;

pub struct SpanPicker {
    span: Span,
    from: Entity<InputState>,
    to: Entity<InputState>,
    /// The custom days last applied.
    custom: (NaiveDate, NaiveDate),
    /// The typed days could not be applied.
    refused: bool,
}

impl EventEmitter<SpanChanged> for SpanPicker {}

/// The days the picker starts on, and the custom days it suggests.
const START_DAYS: u16 = 30;

impl SpanPicker {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let today = Local::now().date_naive();
        let from = input("dd/mm/aaaa", window, cx);
        let to = input("dd/mm/aaaa", window, cx);
        let start = today - TimeDelta::days(i64::from(START_DAYS) - 1);
        from.update(cx, |input, cx| input.set_value(day_text(start), window, cx));
        to.update(cx, |input, cx| input.set_value(day_text(today), window, cx));
        Self {
            span: Span::Days(START_DAYS),
            from,
            to,
            custom: (start, today),
            refused: false,
        }
    }

    /// The time picked.
    pub fn during(&self) -> Range<Timestamp> {
        time_of(self.days())
    }

    /// The days picked, both ends included.
    pub fn days(&self) -> RangeInclusive<NaiveDate> {
        match self.span {
            Span::Days(count) => last_days(count, Local::now().date_naive()),
            Span::Custom => self.custom.0..=self.custom.1,
        }
    }

    fn pick(&mut self, span: Span, cx: &mut Context<Self>) {
        self.span = span;
        self.refused = false;
        cx.emit(SpanChanged);
        cx.notify();
    }

    /// Applies the typed days; days that make no period keep the last ones.
    fn apply_custom(&mut self, cx: &mut Context<Self>) {
        let from = parse_day(&self.from.read(cx).value());
        let to = parse_day(&self.to.read(cx).value());
        match (from, to) {
            (Some(from), Some(to)) if from <= to => {
                self.custom = (from, to);
                self.refused = false;
                cx.emit(SpanChanged);
            }
            _ => self.refused = true,
        }
        cx.notify();
    }
}

impl Render for SpanPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let spans = h_flex()
            .gap_1()
            .children(Span::ALL.into_iter().enumerate().map(|(at, span)| {
                let button = Button::new(("span", at))
                    .label(span.name())
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| this.pick(span, cx)));
                if span == self.span {
                    button.primary()
                } else {
                    button.ghost()
                }
            }));
        let field = |label: &'static str, state: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .w(px(130.))
                .child(div().text_xs().font_medium().child(label))
                .child(Input::new(state).small())
        };
        v_flex()
            .gap_2()
            .child(spans)
            .when(self.span == Span::Custom, |picker| {
                picker.child(
                    h_flex()
                        .flex_wrap()
                        .gap_2()
                        .items_end()
                        .child(field("De", &self.from))
                        .child(field("Até", &self.to))
                        .child(
                            Button::new("apply-days")
                                .label("Aplicar")
                                .outline()
                                .small()
                                .on_click(cx.listener(|this, _, _, cx| this.apply_custom(cx))),
                        ),
                )
            })
            .when(self.refused, |picker| {
                picker.child(div().text_xs().text_color(t.danger).child(
                    "Digite o primeiro e o último dia (ex.: 01/10/2026 e 31/10/2026), o \
                     primeiro antes do último.",
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[test]
    fn the_last_days_end_tomorrow_and_include_today() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let local = |month, day| {
            Local
                .with_ymd_and_hms(2026, month, day, 0, 0, 0)
                .earliest()
                .unwrap()
                .with_timezone(&Utc)
        };

        let week = last_days(7, today);
        assert_eq!(week, NaiveDate::from_ymd_opt(2026, 9, 29).unwrap()..=today);
        assert_eq!(time_of(week), local(9, 29)..local(10, 6));
    }
}
