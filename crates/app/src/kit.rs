//! Shared pieces that carry the presentation rules (#39). Colors come from
//! the current [`look`], so every piece repaints with the theme.

use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{App, Div, SharedString, div, px};

use crate::appearance::look;

/// A section heading.
pub fn section_heading(text: impl Into<SharedString>) -> Div {
    div()
        .text_lg()
        .font_weight(gpui_kit::FontWeight::MEDIUM)
        .child(text.into())
}

/// Something that went wrong: icon and text in the danger ink.
pub fn error_notice(text: impl Into<SharedString>, cx: &App) -> Div {
    let ink = look(cx).tokens.danger;
    h_flex()
        .min_w_0()
        .gap_1p5()
        .items_start()
        .text_sm()
        .text_color(ink)
        .child(
            div()
                .flex_none()
                .pt(px(2.))
                .child(Icon::new(IconName::CircleX).size(px(14.)).text_color(ink)),
        )
        .child(div().min_w_0().child(text.into()))
}

/// A radio mark, filled when `on`.
pub fn radio(on: bool, cx: &App) -> Div {
    let t = look(cx).tokens;
    div()
        .flex_none()
        .size(px(16.))
        .rounded_full()
        .bg(t.raised)
        .border_color(if on { t.accent } else { t.border_strong })
        .map(|dot| {
            if on {
                dot.border(px(5.))
            } else {
                dot.border_1()
            }
        })
}
