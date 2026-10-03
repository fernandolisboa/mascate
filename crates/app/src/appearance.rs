//! Applies an interface theme (#39): maps the palette onto gpui-kit's theme,
//! which every stock component reads, and keeps the tokens gpui-kit lacks
//! (status tints, frames, the accent edge) in the [`Look`] global that
//! Mascate's own elements read through [`look`].

use std::borrow::Cow;
use std::rc::Rc;

use gpui_kit::component::{Theme, ThemeConfig, ThemeConfigColors};
use gpui_kit::{App, Global, Hsla, Pixels, SharedString, Window, WindowAppearance, px, rgb};
use mascate_platform::{ThemeFamily, ThemeMode, UiTheme, UiThemePreference};

use crate::palette::{Palette, Rgb, UiFont, palette};

const MONO_FAMILY: &str = "JetBrains Mono";

const MONO_FONTS: [&[u8]; 4] = [
    include_bytes!("../fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../fonts/JetBrainsMono-Medium.ttf"),
    include_bytes!("../fonts/JetBrainsMono-SemiBold.ttf"),
    include_bytes!("../fonts/JetBrainsMono-Bold.ttf"),
];

pub fn color(value: Rgb) -> Hsla {
    rgb(value.0).into()
}

/// A theme's tokens, ready to paint with.
#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    pub app: Hsla,
    pub surface: Hsla,
    pub raised: Hsla,
    pub sunken: Hsla,
    pub hover: Hsla,
    pub selected: Hsla,
    pub border: Hsla,
    pub frame: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    pub text2: Hsla,
    pub accent: Hsla,
    pub accent_text: Hsla,
    pub accent_edge: Hsla,
    pub success: Hsla,
    pub danger: Hsla,
    /// Corner radius of controls.
    pub radius: Pixels,
    /// Corner radius of cards and panels.
    pub radius_lg: Pixels,
    /// Outline width of cards and controls.
    pub border_width: Pixels,
}

impl Tokens {
    fn new(p: &Palette) -> Self {
        let radius = f32::from(p.radius);
        Self {
            app: color(p.app),
            surface: color(p.surface),
            raised: color(p.raised),
            sunken: color(p.sunken),
            hover: color(p.hover),
            selected: color(p.selected),
            border: color(p.border),
            frame: color(p.frame),
            border_strong: color(p.border_strong),
            text: color(p.text),
            text2: color(p.text2),
            accent: color(p.accent),
            accent_text: color(p.accent_text),
            accent_edge: color(p.accent_edge),
            success: color(p.success),
            danger: color(p.danger),
            radius: px(radius),
            // Cards round a little more than controls; square stays square.
            radius_lg: px(if radius == 0. { 0. } else { radius + 2. }),
            border_width: px(f32::from(p.border_width)),
        }
    }
}

/// The theme on screen and the tokens Mascate paints with besides gpui-kit.
pub struct Look {
    pub theme: UiTheme,
    pub tokens: Tokens,
    /// The platform's interface and monospace families, which base themes
    /// keep (a terminal theme replaces both while it is on).
    system_font: SharedString,
    system_mono: SharedString,
}

impl Global for Look {}

/// The current look. Every element that is not a stock component reads its
/// colors here, so a theme change repaints it too.
pub fn look(cx: &App) -> &Look {
    cx.global::<Look>()
}

/// The system's light/dark setting as the window sees it.
pub fn system_mode(window: &Window) -> ThemeMode {
    mode_of(window.appearance())
}

fn mode_of(appearance: WindowAppearance) -> ThemeMode {
    match appearance {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => ThemeMode::Dark,
        WindowAppearance::Light | WindowAppearance::VibrantLight => ThemeMode::Light,
    }
}

/// Embeds the terminal font and shows the theme `preference` picks for the
/// system's current appearance. Call once, after `gpui_kit::init`.
pub fn init(preference: UiThemePreference, cx: &mut App) {
    let fonts = MONO_FONTS.iter().map(|&font| Cow::Borrowed(font)).collect();
    if let Err(error) = cx.text_system().add_fonts(fonts) {
        // Terminal themes then fall back to the platform's fonts.
        eprintln!("could not load the terminal theme font: {error}");
    }
    let theme = Theme::global(cx);
    let system_font = theme.font_family.clone();
    let system_mono = theme.mono_font_family.clone();
    // A first guess before any window exists; the shell confirms it with its
    // window's appearance once it opens.
    let chosen = preference.resolve(mode_of(cx.window_appearance()));
    cx.set_global(Look {
        theme: chosen,
        tokens: Tokens::new(palette(chosen)),
        system_font,
        system_mono,
    });
    show(chosen, cx);
}

/// Shows the theme `preference` picks while the system is in `mode`.
pub fn follow(preference: UiThemePreference, mode: ThemeMode, cx: &mut App) {
    let theme = preference.resolve(mode);
    if theme != look(cx).theme {
        show(theme, cx);
    }
}

/// Repaints every window in `theme`.
fn show(theme: UiTheme, cx: &mut App) {
    let look = cx.global_mut::<Look>();
    look.theme = theme;
    look.tokens = Tokens::new(palette(theme));
    let config = Rc::new(config(
        theme,
        look.system_font.clone(),
        look.system_mono.clone(),
    ));
    // `update` re-syncs gpui-kit's token copies and refreshes every window.
    Theme::update(cx, |gpui_theme| gpui_theme.apply_config(&config));
}

fn hex(value: Rgb) -> Option<SharedString> {
    Some(format!("#{:06X}", value.0).into())
}

/// The gpui-kit theme for `theme`: the palette on gpui-kit's keys.
fn config(theme: UiTheme, system_font: SharedString, system_mono: SharedString) -> ThemeConfig {
    let p = palette(theme);
    let terminal = theme.family() == ThemeFamily::Terminal;
    let (font, mono) = match p.font {
        UiFont::JetBrainsMono => (MONO_FAMILY.into(), MONO_FAMILY.into()),
        UiFont::System => (system_font, system_mono),
    };
    let radius = usize::from(p.radius);
    ThemeConfig {
        is_default: false,
        name: SharedString::new_static(theme.code()),
        mode: match theme.mode() {
            ThemeMode::Light => gpui_kit::component::ThemeMode::Light,
            ThemeMode::Dark => gpui_kit::component::ThemeMode::Dark,
        },
        font_size: None,
        font_family: Some(font),
        mono_font_family: Some(mono),
        mono_font_size: None,
        radius: Some(radius),
        radius_lg: Some(if radius == 0 { 0 } else { radius + 2 }),
        // Soft shadows only lift light base cards; terminal and dark themes
        // separate surfaces by tone and frame alone.
        shadow: Some(!terminal && theme.mode() == ThemeMode::Light),
        colors: colors(p),
        highlight: None,
    }
}

fn colors(p: &Palette) -> ThemeConfigColors {
    // Some of its fields are private, so no struct literal.
    let mut c = ThemeConfigColors::default();
    c.background = hex(p.app);
    c.foreground = hex(p.text);
    c.border = hex(p.border);
    c.input = hex(p.border_strong);
    c.ring = hex(p.focus);
    c.caret = hex(p.text);
    c.selection = hex(p.selected);
    c.muted = hex(p.sunken);
    c.muted_foreground = hex(p.text2);
    c.accent = hex(p.hover);
    c.accent_foreground = hex(p.text);
    c.group_box = hex(p.surface);
    c.group_box_foreground = hex(p.text);
    c.group_box_title_foreground = hex(p.text2);
    c.popover = hex(p.raised);
    c.popover_foreground = hex(p.text);
    c.primary = hex(p.accent);
    c.primary_foreground = hex(p.on_accent);
    c.button_primary = hex(p.accent);
    c.button_primary_foreground = hex(p.on_accent);
    c.secondary = hex(p.raised);
    c.secondary_foreground = hex(p.text);
    c.secondary_hover = hex(p.hover);
    c.button = hex(p.raised);
    c.button_foreground = hex(p.text);
    c.button_hover = hex(p.hover);
    // Status colors are inks first (state is written with them), so a fill
    // takes the surface's color for its text.
    c.success = hex(p.success);
    c.success_foreground = hex(p.surface);
    c.warning = hex(p.warning);
    c.warning_foreground = hex(p.surface);
    c.danger = hex(p.danger);
    c.danger_foreground = hex(p.surface);
    c.info = hex(p.info);
    c.info_foreground = hex(p.surface);
    c.link = hex(p.accent_text);
    c.link_hover = hex(p.accent_text);
    c.link_active = hex(p.accent_text);
    c.list = hex(p.surface);
    c.list_hover = hex(p.hover);
    c.list_active = hex(p.selected);
    c.list_active_border = hex(p.accent_edge);
    c.list_even = hex(p.surface);
    c.list_head = hex(p.sunken);
    c.table = hex(p.surface);
    c.table_hover = hex(p.hover);
    c.table_active = hex(p.selected);
    c.table_active_border = hex(p.accent_edge);
    c.table_even = hex(p.surface);
    c.table_head = hex(p.sunken);
    c.table_head_foreground = hex(p.text2);
    c.table_row_border = hex(p.border);
    c.tab_bar = hex(p.app);
    c.tab = hex(p.app);
    c.tab_active = hex(p.surface);
    c.tab_foreground = hex(p.text2);
    c.tab_active_foreground = hex(p.text);
    c.sidebar = hex(p.surface);
    c.sidebar_foreground = hex(p.text);
    c.sidebar_border = hex(p.border);
    c.sidebar_accent = hex(p.selected);
    c.sidebar_accent_foreground = hex(p.text);
    c.sidebar_primary = hex(p.accent);
    c.sidebar_primary_foreground = hex(p.on_accent);
    c.title_bar = hex(p.surface);
    c.title_bar_border = hex(p.border);
    c.description_list_label = hex(p.sunken);
    c.description_list_label_foreground = hex(p.text2);
    c.progress_bar = hex(p.accent);
    c.slider_bar = hex(p.accent);
    c.slider_thumb = hex(p.raised);
    c.switch = hex(p.accent);
    c.switch_thumb = hex(p.raised);
    c.skeleton = hex(p.hover);
    c.drag_border = hex(p.accent);
    c.drop_target = hex(p.selected);
    c.scrollbar_thumb = hex(p.border_strong);
    c.scrollbar_thumb_hover = hex(p.text3);
    c.window_border = hex(p.frame);
    c
}
