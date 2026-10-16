//! Studio (#41): denser, after pro tools. Top tabs, each area folded into a
//! menu, a compact header and a status bar.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{Icon, Side, Sizable as _, h_flex, v_flex};
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
    // Sixteen places as tabs need more than a window's width, so each area
    // folds into a menu of its places; the one holding the current place
    // shows as picked. Only the unnamed first area stays as tabs.
    let area = |ix: usize, name: &'static str, places: &'static [Place]| {
        let pick = Rc::clone(&nav.on_pick);
        let current = nav.current;
        let menu = Button::new(("area", ix))
            .ghost()
            .small()
            .label(name)
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                // On the left each place keeps its icon; the check goes right.
                places
                    .iter()
                    .fold(menu.check_side(Side::Right), |menu, &place| {
                        let pick = Rc::clone(&pick);
                        menu.item(
                            PopupMenuItem::new(place.name())
                                .icon(Icon::new(place.icon()))
                                .checked(place == current)
                                .on_click(move |_, window, cx| pick(place, window, cx)),
                        )
                    })
            });
        // Framed as a picked tab while it holds the current place.
        div()
            .rounded(t.radius)
            .border(t.border_width)
            .map(|frame| {
                if places.contains(&current) {
                    frame.bg(t.raised).border_color(t.border_strong)
                } else {
                    frame.border_color(gpui_kit::transparent_black())
                }
            })
            .child(menu)
            .into_any_element()
    };
    let entries = Place::GROUPS
        .iter()
        .enumerate()
        .flat_map(|(ix, group)| match group.name {
            Some(name) => vec![area(ix, name, group.places)],
            None => group
                .places
                .iter()
                .map(|place| tab(*place).into_any_element())
                .collect(),
        });
    h_flex()
        .flex_none()
        .w_full()
        .min_h(px(42.))
        .px_3()
        .py(px(6.))
        .gap_1()
        .items_start()
        .bg(t.surface)
        .border_b(t.border_width)
        .border_color(t.border)
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .flex_wrap()
                .gap_1()
                .items_center()
                .children(entries),
        )
        .child(
            h_flex()
                .flex_none()
                .gap_1()
                .children(Place::PINNED.map(tab)),
        )
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
