//! The window's title bar (#67), drawn by Mascate in the interface theme
//! in place of the native caption, after Bardo's: the app mark and name, a
//! drag area, and the minimize / maximize / close buttons.
//!
//! On Windows every part declares what it is ([`WindowControlArea`]) and
//! the system does the rest, as it does for its own caption: dragging,
//! double click to maximize, snapping, the snap layouts flyout on the
//! maximize button and the system menu. Elsewhere the bar drags and
//! zooms the window itself, and leaves the buttons to the window manager
//! when it draws its own decorations.

use gpui_kit::assets::IconName;
use gpui_kit::component::{Icon, InteractiveElementExt as _, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Decorations, MouseButton, Pixels, Window, WindowControlArea, WindowControls,
    div, px,
};

use crate::appearance::{Tokens, look};

/// Height of the bar.
const HEIGHT: Pixels = px(32.);
/// Width of each window button, as Windows draws its own.
const BUTTON_WIDTH: Pixels = px(46.);

/// The bar over every layout.
pub fn title_bar(window: &mut Window, cx: &mut App) -> AnyElement {
    let t = look(cx).tokens;
    let brand = h_flex()
        .gap_2()
        .pl_3()
        // Room for the traffic lights, which macOS keeps drawing.
        .when(cfg!(target_os = "macos"), |brand| brand.pl(px(80.)))
        .items_center()
        .child(
            div()
                .size(px(12.))
                .rounded(if t.radius == px(0.) { px(0.) } else { px(3.) })
                .bg(t.accent),
        )
        .child(
            div()
                .text_xs()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child("Mascate"),
        );
    let drag = h_flex()
        .id("title-bar-drag")
        .flex_1()
        .h_full()
        .min_w_0()
        .items_center()
        .child(brand);
    let drag = if cfg!(target_os = "windows") {
        drag.window_control_area(WindowControlArea::Drag)
    } else {
        // Without the system's caption the bar moves the window itself,
        // once the pointer moves, so a double click still reaches it.
        let pressed = window.use_keyed_state("title-bar-pressed", cx, |_, _| Pressed(false));
        let down = pressed.clone();
        let up = pressed.clone();
        let out = pressed.clone();
        drag.on_mouse_down(MouseButton::Left, move |_, _, cx| {
            down.update(cx, |pressed, _| pressed.0 = true);
        })
        .on_mouse_up(MouseButton::Left, move |_, _, cx| {
            up.update(cx, |pressed, _| pressed.0 = false);
        })
        .on_mouse_down_out(move |_, _, cx| {
            out.update(cx, |pressed, _| pressed.0 = false);
        })
        .on_mouse_move(move |_, window, cx| {
            if pressed.update(cx, |pressed, _| std::mem::take(&mut pressed.0)) {
                window.start_window_move();
            }
        })
        .on_double_click(|_, window, _| window.zoom_window())
        .on_mouse_down(MouseButton::Right, |event, window, _| {
            window.show_window_menu(event.position);
        })
    };
    let buttons = draws_buttons(window.window_decorations())
        .then(|| buttons(window.window_controls(), window.is_maximized()));
    h_flex()
        .id("title-bar")
        .flex_none()
        .w_full()
        .h(HEIGHT)
        .bg(t.surface)
        .border_b(t.border_width)
        .border_color(t.border)
        .text_color(t.text)
        .child(drag)
        .when_some(buttons, |bar, buttons| {
            bar.child(
                h_flex()
                    .h_full()
                    .flex_none()
                    .children(buttons.into_iter().map(|which| button(which, t))),
            )
        })
        .into_any_element()
}

/// Whether a press on the drag area may still turn into a window move.
struct Pressed(bool);

/// Whether the window's buttons are Mascate's to draw: always on Windows,
/// on Linux only when the window manager leaves decorations to the app,
/// never on macOS.
fn draws_buttons(decorations: Decorations) -> bool {
    if cfg!(target_os = "macos") {
        return false;
    }
    !cfg!(target_os = "linux") || matches!(decorations, Decorations::Client { .. })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Button {
    Minimize,
    Maximize,
    Restore,
    Close,
}

impl Button {
    fn id(self) -> &'static str {
        match self {
            Button::Minimize => "window-minimize",
            Button::Maximize => "window-maximize",
            Button::Restore => "window-restore",
            Button::Close => "window-close",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Button::Minimize => IconName::WindowMinimize,
            Button::Maximize => IconName::WindowMaximize,
            Button::Restore => IconName::WindowRestore,
            Button::Close => IconName::WindowClose,
        }
    }

    fn area(self) -> WindowControlArea {
        match self {
            Button::Minimize => WindowControlArea::Min,
            Button::Maximize | Button::Restore => WindowControlArea::Max,
            Button::Close => WindowControlArea::Close,
        }
    }
}

/// The buttons the platform supports, left to right; a maximized window
/// offers to restore.
fn buttons(supported: WindowControls, maximized: bool) -> Vec<Button> {
    let size = if maximized {
        Button::Restore
    } else {
        Button::Maximize
    };
    [
        supported.minimize.then_some(Button::Minimize),
        supported.maximize.then_some(size),
        Some(Button::Close),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn button(which: Button, t: Tokens) -> AnyElement {
    let (hover, ink) = if which == Button::Close {
        (t.danger, t.surface)
    } else {
        (t.hover, t.text)
    };
    div()
        .id(which.id())
        .flex()
        .flex_none()
        .w(BUTTON_WIDTH)
        .h_full()
        .items_center()
        .justify_center()
        .hover(move |style| style.bg(hover).text_color(ink))
        .child(Icon::new(which.icon()).size(px(14.)))
        // Windows presses the button itself, from the area it reports.
        .when(cfg!(target_os = "windows"), |button| {
            button.window_control_area(which.area())
        })
        .when(!cfg!(target_os = "windows"), |button| {
            button
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(move |_, window, _| match which {
                    Button::Minimize => window.minimize_window(),
                    Button::Maximize | Button::Restore => window.zoom_window(),
                    Button::Close => window.remove_window(),
                })
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui_kit::assets::Assets;
    use gpui_kit::{AssetSource, Tiling};

    use super::*;

    /// The app ships only gpui-kit's default icons: any other draws nothing.
    #[test]
    fn every_window_button_has_an_icon_the_app_ships() {
        for which in [
            Button::Minimize,
            Button::Maximize,
            Button::Restore,
            Button::Close,
        ] {
            assert!(
                matches!(Assets.load(&which.icon().path()), Ok(Some(_))),
                "{which:?} has no shipped icon"
            );
        }
    }

    #[test]
    fn a_window_offers_minimize_maximize_and_close() {
        assert_eq!(
            buttons(WindowControls::default(), false),
            [Button::Minimize, Button::Maximize, Button::Close]
        );
    }

    #[test]
    fn a_maximized_window_offers_to_restore() {
        assert_eq!(
            buttons(WindowControls::default(), true),
            [Button::Minimize, Button::Restore, Button::Close]
        );
    }

    #[test]
    fn only_the_buttons_the_platform_supports_are_drawn_and_close_always_is() {
        let none = WindowControls {
            fullscreen: false,
            maximize: false,
            minimize: false,
            window_menu: false,
        };
        assert_eq!(buttons(none, false), [Button::Close]);
        assert_eq!(
            buttons(
                WindowControls {
                    minimize: true,
                    ..none
                },
                true
            ),
            [Button::Minimize, Button::Close]
        );
    }

    #[test]
    fn the_app_draws_the_buttons_when_decorations_are_left_to_it() {
        let client = Decorations::Client {
            tiling: Tiling::default(),
        };
        assert_eq!(draws_buttons(client), !cfg!(target_os = "macos"));
    }

    #[test]
    fn the_window_manager_keeps_the_buttons_it_draws_on_linux() {
        assert_eq!(
            draws_buttons(Decorations::Server),
            cfg!(target_os = "windows")
        );
    }
}
