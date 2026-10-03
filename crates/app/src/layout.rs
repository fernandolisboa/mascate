//! Arrangements: where a layout places the parts of [`crate::parts`] (#40,
//! #41). They place and frame, and decide nothing. The saved [`LayoutId`]
//! picks one; a new layout is a variant there, a module here and an arm in
//! each switch.

mod studio;
mod workspace;

use gpui_kit::{AnyElement, App, Global};
use mascate_platform::LayoutId;

use crate::parts::{Navigation, ScreenParts};

/// The layout on screen.
struct Current(LayoutId);

impl Global for Current {}

pub fn current(cx: &App) -> LayoutId {
    cx.try_global::<Current>()
        .map_or_else(LayoutId::default, |current| current.0)
}

/// Arranges every window in `layout` from the next frame on. Screens keep
/// their state, so only where things go changes.
pub fn show(layout: LayoutId, cx: &mut App) {
    if cx
        .try_global::<Current>()
        .is_some_and(|current| current.0 == layout)
    {
        return;
    }
    cx.set_global(Current(layout));
    cx.refresh_windows();
}

/// The window: the navigation around the current screen.
pub fn shell(navigation: Navigation, screen: AnyElement, cx: &App) -> AnyElement {
    match current(cx) {
        LayoutId::Workspace => workspace::shell(navigation, screen, cx),
        LayoutId::Studio => studio::shell(navigation, screen, cx),
    }
}

/// A screen from its parts.
pub fn screen(parts: ScreenParts, cx: &App) -> AnyElement {
    match current(cx) {
        LayoutId::Workspace => workspace::screen(parts, cx),
        LayoutId::Studio => studio::screen(parts, cx),
    }
}
