//! Studio (#41): denser, after pro tools. Top tabs, a compact header and a
//! status bar.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, div, px};

use crate::appearance::look;
use crate::parts::{AppState, Navigation, Place, ScreenParts};

pub(super) fn shell(navigation: Navigation, screen: AnyElement, cx: &App) -> AnyElement {
    let t = &look(cx).tokens;
    v_flex()
        .size_full()
        .bg(t.app)
        .text_color(t.text)
        .child(tabs(&navigation, cx))
        .child(div().flex_1().min_h_0().w_full().child(screen))
        .child(status_bar(&navigation.state, cx))
        .into_any_element()
}

fn tabs(nav: &Navigation, cx: &App) -> AnyElement {
    let t = look(cx).tokens;
    let tab = |place: Place| {
        let pick = Rc::clone(&nav.on_pick);
        let selected = place == nav.current;
        h_flex()
            .id(("tab", place as usize))
            .gap_1p5()
            .px_2p5()
            .py_1()
            .rounded(t.radius)
            .cursor_pointer()
            .text_sm()
            .font_weight(gpui_kit::FontWeight::MEDIUM)
            .border(t.border_width)
            .map(|tab| {
                if selected {
                    tab.bg(t.raised)
                        .text_color(t.text)
                        .border_color(t.border_strong)
                } else {
                    tab.text_color(t.text2)
                        .border_color(gpui_kit::transparent_black())
                        .hover(|tab| tab.bg(t.hover))
                }
            })
            .child(Icon::new(place.icon()).size(px(14.)))
            .child(place.name())
            .on_click(move |_, window, cx| pick(place, window, cx))
    };
    h_flex()
        .flex_none()
        .h(px(42.))
        .px_3()
        .gap_1()
        .bg(t.surface)
        .border_b(t.border_width)
        .border_color(t.border)
        .child(
            h_flex()
                .gap_2()
                .mr_3()
                .text_sm()
                .font_weight(gpui_kit::FontWeight::BOLD)
                .child(div().size(px(14.)).rounded(t.radius).bg(t.accent))
                .child("Mascate"),
        )
        .children(Place::MAIN.map(tab))
        .child(div().flex_1())
        .children(Place::PINNED.map(tab))
        .into_any_element()
}

fn status_bar(state: &AppState, cx: &App) -> AnyElement {
    let t = look(cx).tokens;
    let (icon, ink, label) = match state {
        AppState::Ready => (IconName::CircleCheck, t.success, "Pronto"),
        AppState::DatabaseUnavailable => {
            (IconName::CircleX, t.danger, "Banco de dados indisponível")
        }
    };
    h_flex()
        .flex_none()
        .h(px(26.))
        .px_3()
        .gap_4()
        .text_xs()
        .text_color(t.text2)
        .bg(t.surface)
        .border_t(t.border_width)
        .border_color(t.border)
        .child(
            h_flex()
                .gap_1p5()
                .child(Icon::new(icon).size(px(13.)).text_color(ink))
                .child(label),
        )
        .child(div().flex_1())
        .child(concat!("Mascate ", env!("CARGO_PKG_VERSION")))
        .into_any_element()
}

pub(super) fn screen(parts: ScreenParts, cx: &App) -> AnyElement {
    let t = look(cx).tokens;
    v_flex()
        .size_full()
        .child(
            h_flex()
                .flex_none()
                .flex_wrap()
                .gap_2()
                .px_4()
                .py_2()
                .items_center()
                .border_b(t.border_width)
                .border_color(t.border)
                .child(
                    div()
                        .flex_1()
                        .text_base()
                        .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                        .child(parts.title),
                )
                .children(parts.actions),
        )
        .child(
            v_flex()
                .id("studio-screen")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_4()
                .gap_3()
                .children(parts.notices)
                .children(parts.content),
        )
        .into_any_element()
}
