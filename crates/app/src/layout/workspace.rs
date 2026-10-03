//! Workspace (#40): a sidebar of places, pages with room around them.

use std::rc::Rc;

use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, div, px};

use crate::appearance::look;
use crate::parts::{Navigation, Place, ScreenParts};

pub(super) fn shell(navigation: Navigation, screen: AnyElement, cx: &App) -> AnyElement {
    let t = &look(cx).tokens;
    h_flex()
        .size_full()
        .items_start()
        .bg(t.app)
        .text_color(t.text)
        .child(sidebar(navigation, cx))
        .child(div().flex_1().h_full().min_w_0().child(screen))
        .into_any_element()
}

fn sidebar(nav: Navigation, cx: &App) -> AnyElement {
    let t = look(cx).tokens;
    let row = |place: Place| {
        let pick = Rc::clone(&nav.on_pick);
        let selected = place == nav.current;
        h_flex()
            .id(("nav", place as usize))
            .gap_2()
            .px_2()
            .py_1p5()
            .rounded(t.radius)
            .cursor_pointer()
            .text_sm()
            .font_weight(gpui_kit::FontWeight::MEDIUM)
            .map(|row| {
                if selected {
                    row.bg(t.selected).text_color(t.text)
                } else {
                    row.text_color(t.text2).hover(|row| row.bg(t.hover))
                }
            })
            .child(
                Icon::new(place.icon())
                    .size(px(16.))
                    .text_color(if selected { t.accent_text } else { t.text2 }),
            )
            .child(div().flex_1().min_w_0().truncate().child(place.name()))
            .on_click(move |_, window, cx| pick(place, window, cx))
    };

    v_flex()
        .flex_none()
        .w(px(216.))
        .h_full()
        .p_2p5()
        .gap_0p5()
        .bg(t.surface)
        .border_r(t.border_width)
        .border_color(t.border)
        .child(brand(cx))
        .children(Place::MAIN.map(row))
        .child(div().flex_1())
        .child(
            v_flex()
                .gap_0p5()
                .pt_2()
                .border_t(t.border_width)
                .border_color(t.border)
                .children(Place::PINNED.map(row)),
        )
        .into_any_element()
}

fn brand(cx: &App) -> impl IntoElement {
    let t = look(cx).tokens;
    h_flex()
        .gap_2()
        .px_2()
        .pt_1()
        .pb_3()
        .text_base()
        .font_weight(gpui_kit::FontWeight::BOLD)
        .child(div().size(px(18.)).rounded(t.radius).bg(t.accent))
        .child("Mascate")
}

pub(super) fn screen(parts: ScreenParts, cx: &App) -> AnyElement {
    let t = look(cx).tokens;
    v_flex()
        .id("workspace-screen")
        .size_full()
        .overflow_y_scroll()
        .px_8()
        .py_6()
        .gap_5()
        .bg(t.app)
        .text_color(t.text)
        .child(
            h_flex()
                .flex_wrap()
                .gap_3()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .text_2xl()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(parts.title),
                )
                .children(parts.actions),
        )
        .children(parts.notices)
        .children(parts.content)
        .into_any_element()
}
