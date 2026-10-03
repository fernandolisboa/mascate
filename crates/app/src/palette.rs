//! The interface themes' colors (#39): plain data, so contrast is checked
//! without a window. Same values as the owner's Bardo app, so both look
//! alike. [`crate::appearance`] maps them onto gpui-kit.

use mascate_platform::UiTheme;

/// An sRGB color, `0xRRGGBB`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgb(pub u32);

/// The font a theme draws its interface in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiFont {
    /// The platform's interface font (Segoe UI on Windows).
    System,
    /// Embedded with the app (OFL, `fonts/`), so it renders the same everywhere.
    JetBrainsMono,
}

/// One theme's semantic tokens. Surfaces go from darkest to lightest in
/// dark themes (`sunken` < `app` < `surface` < `raised`) and the other way
/// in light ones, so cards stand off the ground and wells sink into cards.
#[derive(Debug, Clone, PartialEq)]
pub struct Palette {
    /// The window's ground.
    pub app: Rgb,
    /// Cards and panels.
    pub surface: Rgb,
    /// Controls and menus above a surface.
    pub raised: Rgb,
    /// Wells: read-only text, inputs.
    pub sunken: Rgb,
    pub hover: Rgb,
    /// A selected row or option (accent-tinted).
    pub selected: Rgb,
    /// Dividers.
    pub border: Rgb,
    /// The outline of panels and cards (accent-tinted in terminal themes).
    pub frame: Rgb,
    /// The outline of controls.
    pub border_strong: Rgb,
    pub text: Rgb,
    pub text2: Rgb,
    pub text3: Rgb,
    pub accent: Rgb,
    /// Text and icons on an accent fill.
    pub on_accent: Rgb,
    /// Accent-colored text and links.
    pub accent_text: Rgb,
    /// The outline of accent-filled buttons.
    pub accent_edge: Rgb,
    pub focus: Rgb,
    pub success: Rgb,
    pub success_bg: Rgb,
    pub warning: Rgb,
    pub warning_bg: Rgb,
    /// Meters and bars that warn.
    pub warning_fill: Rgb,
    pub danger: Rgb,
    pub danger_bg: Rgb,
    pub info: Rgb,
    pub info_bg: Rgb,
    /// Corner radius of controls, in px (0 in terminal themes).
    pub radius: u8,
    /// Outline width in px (2 in high contrast).
    pub border_width: u8,
    pub font: UiFont,
}

/// The tokens of `theme`.
pub fn palette(theme: UiTheme) -> &'static Palette {
    match theme {
        UiTheme::Paper => &PAPER,
        UiTheme::Sand => &SAND,
        UiTheme::Graphite => &GRAPHITE,
        UiTheme::Slate => &SLATE,
        UiTheme::HighContrastLight => &HIGH_CONTRAST_LIGHT,
        UiTheme::HighContrastDark => &HIGH_CONTRAST_DARK,
        UiTheme::BlackGold => &BLACK_GOLD,
        UiTheme::Brass => &BRASS,
        UiTheme::Phosphor => &PHOSPHOR,
        UiTheme::PhosphorLight => &PHOSPHOR_LIGHT,
    }
}

const PAPER: Palette = Palette {
    app: Rgb(0xF2F2EF),
    surface: Rgb(0xFFFFFF),
    raised: Rgb(0xFFFFFF),
    sunken: Rgb(0xF4F4F1),
    hover: Rgb(0xECECE8),
    selected: Rgb(0xFFF3E0),
    border: Rgb(0xE3E3DF),
    frame: Rgb(0xE3E3DF),
    border_strong: Rgb(0x85888B),
    text: Rgb(0x17181A),
    text2: Rgb(0x4A4E54),
    text3: Rgb(0x5F646B),
    accent: Rgb(0xF2A33A),
    on_accent: Rgb(0x1A1206),
    accent_text: Rgb(0x8A4F00),
    accent_edge: Rgb(0xC9852A),
    focus: Rgb(0x9A5800),
    success: Rgb(0x1C7A3D),
    success_bg: Rgb(0xE7F4EB),
    warning: Rgb(0x7F5B00),
    warning_bg: Rgb(0xFBF1D3),
    warning_fill: Rgb(0xD9A300),
    danger: Rgb(0xBF2F28),
    danger_bg: Rgb(0xFCE9E7),
    info: Rgb(0x1F5FB0),
    info_bg: Rgb(0xE7EFFA),
    radius: 6,
    border_width: 1,
    font: UiFont::System,
};

const SAND: Palette = Palette {
    app: Rgb(0xE6E2DA),
    surface: Rgb(0xF1EEE8),
    raised: Rgb(0xF7F5F1),
    sunken: Rgb(0xE9E5DE),
    hover: Rgb(0xE2DED5),
    selected: Rgb(0xF6E6CC),
    border: Rgb(0xD6D1C7),
    frame: Rgb(0xD6D1C7),
    border_strong: Rgb(0x7F7A70),
    text: Rgb(0x1D1B17),
    text2: Rgb(0x47433C),
    text3: Rgb(0x5A554D),
    accent: Rgb(0xE39A35),
    on_accent: Rgb(0x1A1206),
    accent_text: Rgb(0x7D4700),
    accent_edge: Rgb(0xB9771F),
    focus: Rgb(0x8A4F00),
    success: Rgb(0x1B6E38),
    success_bg: Rgb(0xDDEBDF),
    warning: Rgb(0x715100),
    warning_bg: Rgb(0xF1E4C2),
    warning_fill: Rgb(0xCC9A00),
    danger: Rgb(0xAE2A24),
    danger_bg: Rgb(0xF3DCD7),
    info: Rgb(0x1C5599),
    info_bg: Rgb(0xDCE5F0),
    radius: 6,
    border_width: 1,
    font: UiFont::System,
};

const GRAPHITE: Palette = Palette {
    app: Rgb(0x0F1113),
    surface: Rgb(0x16191C),
    raised: Rgb(0x1E2226),
    sunken: Rgb(0x0C0E10),
    hover: Rgb(0x23282D),
    selected: Rgb(0x2C2416),
    border: Rgb(0x2A2F35),
    frame: Rgb(0x2A2F35),
    border_strong: Rgb(0x5F6873),
    text: Rgb(0xE7E9EC),
    text2: Rgb(0xA3AAB3),
    text3: Rgb(0x8A929C),
    accent: Rgb(0xF2A33A),
    on_accent: Rgb(0x1A1206),
    accent_text: Rgb(0xF2A33A),
    accent_edge: Rgb(0xF2A33A),
    focus: Rgb(0xF2A33A),
    success: Rgb(0x4CC38A),
    success_bg: Rgb(0x13281F),
    warning: Rgb(0xE5C454),
    warning_bg: Rgb(0x2A2512),
    warning_fill: Rgb(0xE5C454),
    danger: Rgb(0xF0716A),
    danger_bg: Rgb(0x2A1416),
    info: Rgb(0x6CB2F5),
    info_bg: Rgb(0x14222F),
    radius: 6,
    border_width: 1,
    font: UiFont::System,
};

const SLATE: Palette = Palette {
    app: Rgb(0x1A1F26),
    surface: Rgb(0x212831),
    raised: Rgb(0x29313B),
    sunken: Rgb(0x161B21),
    hover: Rgb(0x2E3742),
    selected: Rgb(0x3A3020),
    border: Rgb(0x343D48),
    frame: Rgb(0x343D48),
    border_strong: Rgb(0x6D7887),
    text: Rgb(0xE6EAEF),
    text2: Rgb(0xB0B8C3),
    text3: Rgb(0x99A3AF),
    accent: Rgb(0xF4AE4C),
    on_accent: Rgb(0x1A1206),
    accent_text: Rgb(0xF6B860),
    accent_edge: Rgb(0xF4AE4C),
    focus: Rgb(0xF6B860),
    success: Rgb(0x5CCB95),
    success_bg: Rgb(0x1B3329),
    warning: Rgb(0xEACB61),
    warning_bg: Rgb(0x352F1A),
    warning_fill: Rgb(0xEACB61),
    danger: Rgb(0xF57F78),
    danger_bg: Rgb(0x3A2224),
    info: Rgb(0x7DBBF7),
    info_bg: Rgb(0x1D2E3E),
    radius: 6,
    border_width: 1,
    font: UiFont::System,
};

const HIGH_CONTRAST_LIGHT: Palette = Palette {
    app: Rgb(0xFFFFFF),
    surface: Rgb(0xFFFFFF),
    raised: Rgb(0xFFFFFF),
    sunken: Rgb(0xF2F2F2),
    hover: Rgb(0xE6E6E6),
    selected: Rgb(0xFFE7B8),
    border: Rgb(0x1A1A1A),
    frame: Rgb(0x1A1A1A),
    border_strong: Rgb(0x000000),
    text: Rgb(0x000000),
    text2: Rgb(0x1F1F1F),
    text3: Rgb(0x333333),
    accent: Rgb(0xFFB020),
    on_accent: Rgb(0x000000),
    accent_text: Rgb(0x6B3A00),
    accent_edge: Rgb(0x000000),
    focus: Rgb(0x0037DA),
    success: Rgb(0x0B5A26),
    success_bg: Rgb(0xE3F2E7),
    warning: Rgb(0x5C4100),
    warning_bg: Rgb(0xFFF0C2),
    warning_fill: Rgb(0x5C4100),
    danger: Rgb(0x8E0B06),
    danger_bg: Rgb(0xFDE3E1),
    info: Rgb(0x0A3F8C),
    info_bg: Rgb(0xE1EBFA),
    radius: 6,
    border_width: 2,
    font: UiFont::System,
};

const HIGH_CONTRAST_DARK: Palette = Palette {
    app: Rgb(0x000000),
    surface: Rgb(0x000000),
    raised: Rgb(0x0D0D0D),
    sunken: Rgb(0x000000),
    hover: Rgb(0x1F1F1F),
    selected: Rgb(0x3D2A00),
    border: Rgb(0xE6E6E6),
    frame: Rgb(0xE6E6E6),
    border_strong: Rgb(0xFFFFFF),
    text: Rgb(0xFFFFFF),
    text2: Rgb(0xF0F0F0),
    text3: Rgb(0xD9D9D9),
    accent: Rgb(0xFFB54A),
    on_accent: Rgb(0x000000),
    accent_text: Rgb(0xFFC266),
    accent_edge: Rgb(0xFFB54A),
    focus: Rgb(0x4CC2FF),
    success: Rgb(0x5EE89A),
    success_bg: Rgb(0x06200F),
    warning: Rgb(0xFFE066),
    warning_bg: Rgb(0x241D00),
    warning_fill: Rgb(0xFFE066),
    danger: Rgb(0xFF8A80),
    danger_bg: Rgb(0x2A0805),
    info: Rgb(0x8CCBFF),
    info_bg: Rgb(0x051A2E),
    radius: 6,
    border_width: 2,
    font: UiFont::System,
};

const BLACK_GOLD: Palette = Palette {
    app: Rgb(0x0D0D0D),
    surface: Rgb(0x121212),
    raised: Rgb(0x181715),
    sunken: Rgb(0x080808),
    hover: Rgb(0x1D1A12),
    selected: Rgb(0x282003),
    border: Rgb(0x2B2618),
    frame: Rgb(0x7A5C27),
    border_strong: Rgb(0x9C7636),
    text: Rgb(0xE0DFDB),
    text2: Rgb(0xD9BD8B),
    text3: Rgb(0xA89C84),
    accent: Rgb(0xD4AF37),
    on_accent: Rgb(0x0D0D0D),
    accent_text: Rgb(0xDFA650),
    accent_edge: Rgb(0xC18C43),
    focus: Rgb(0xDFA650),
    success: Rgb(0xA3C46B),
    success_bg: Rgb(0x1A2010),
    warning: Rgb(0xF59E0B),
    warning_bg: Rgb(0x2A1E05),
    warning_fill: Rgb(0xF59E0B),
    danger: Rgb(0xE06C6C),
    danger_bg: Rgb(0x2A1212),
    info: Rgb(0x8DB8D0),
    info_bg: Rgb(0x111C22),
    radius: 0,
    border_width: 1,
    font: UiFont::JetBrainsMono,
};

const BRASS: Palette = Palette {
    app: Rgb(0xECE7DA),
    surface: Rgb(0xF7F3E8),
    raised: Rgb(0xFFFCF4),
    sunken: Rgb(0xE7E1D2),
    hover: Rgb(0xE1DAC8),
    selected: Rgb(0xF0DFAF),
    border: Rgb(0xD6CDB6),
    frame: Rgb(0xA9833A),
    border_strong: Rgb(0x7D6533),
    text: Rgb(0x15120B),
    text2: Rgb(0x3F392B),
    text3: Rgb(0x595140),
    accent: Rgb(0xC99A2E),
    on_accent: Rgb(0x15120B),
    accent_text: Rgb(0x7A5600),
    accent_edge: Rgb(0x8E6A1E),
    focus: Rgb(0x7A5600),
    success: Rgb(0x2E6B1F),
    success_bg: Rgb(0xE1EBD2),
    warning: Rgb(0x7A4F00),
    warning_bg: Rgb(0xF6E5C0),
    warning_fill: Rgb(0xC98A00),
    danger: Rgb(0xA8281F),
    danger_bg: Rgb(0xF5DDD8),
    info: Rgb(0x1F5A8A),
    info_bg: Rgb(0xDCE7F0),
    radius: 0,
    border_width: 1,
    font: UiFont::JetBrainsMono,
};

const PHOSPHOR: Palette = Palette {
    app: Rgb(0x040A06),
    surface: Rgb(0x08110B),
    raised: Rgb(0x0D1A11),
    sunken: Rgb(0x020604),
    hover: Rgb(0x0F2216),
    selected: Rgb(0x0B2C16),
    border: Rgb(0x163220),
    frame: Rgb(0x1F7A42),
    border_strong: Rgb(0x2E9A57),
    text: Rgb(0xCBF7D5),
    text2: Rgb(0x86DBA0),
    text3: Rgb(0x62B57F),
    accent: Rgb(0x3DF57A),
    on_accent: Rgb(0x02100A),
    accent_text: Rgb(0x3DF57A),
    accent_edge: Rgb(0x3DF57A),
    focus: Rgb(0x3DF57A),
    success: Rgb(0x5FE0D0),
    success_bg: Rgb(0x082222),
    warning: Rgb(0xE8D35A),
    warning_bg: Rgb(0x211E08),
    warning_fill: Rgb(0xE8D35A),
    danger: Rgb(0xFF6B78),
    danger_bg: Rgb(0x2A0B10),
    info: Rgb(0x6CC8FF),
    info_bg: Rgb(0x08192A),
    radius: 0,
    border_width: 1,
    font: UiFont::JetBrainsMono,
};

const PHOSPHOR_LIGHT: Palette = Palette {
    app: Rgb(0xE4ECE3),
    surface: Rgb(0xF1F6F0),
    raised: Rgb(0xFAFDF9),
    sunken: Rgb(0xDEE7DD),
    hover: Rgb(0xD5E1D4),
    selected: Rgb(0xC8E8D0),
    border: Rgb(0xC2D2C1),
    frame: Rgb(0x2F8050),
    border_strong: Rgb(0x4C7858),
    text: Rgb(0x06140A),
    text2: Rgb(0x21402B),
    text3: Rgb(0x385A43),
    accent: Rgb(0x2BC45E),
    on_accent: Rgb(0x02100A),
    accent_text: Rgb(0x0A6A2C),
    accent_edge: Rgb(0x0A6A2C),
    focus: Rgb(0x0A6A2C),
    success: Rgb(0x0D5F5A),
    success_bg: Rgb(0xD3EAE7),
    warning: Rgb(0x6B5300),
    warning_bg: Rgb(0xEFE6BE),
    warning_fill: Rgb(0xB89A00),
    danger: Rgb(0xA3202B),
    danger_bg: Rgb(0xF4DADC),
    info: Rgb(0x16507F),
    info_bg: Rgb(0xD8E5F0),
    radius: 0,
    border_width: 1,
    font: UiFont::JetBrainsMono,
};

#[cfg(test)]
mod tests {
    use mascate_platform::{ThemeFamily, ThemeMode};

    use super::*;

    impl Rgb {
        fn channel(self, shift: u32) -> f64 {
            let value = f64::from((self.0 >> shift) & 0xFF) / 255.0;
            if value <= 0.039_28 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }

        /// WCAG 2.x relative luminance.
        fn luminance(self) -> f64 {
            0.2126 * self.channel(16) + 0.7152 * self.channel(8) + 0.0722 * self.channel(0)
        }

        /// This color at `alpha` over `ground`, blended in sRGB.
        fn over(self, ground: Rgb, alpha: f64) -> Rgb {
            let mix = |shift: u32| {
                let top = f64::from((self.0 >> shift) & 0xFF);
                let bottom = f64::from((ground.0 >> shift) & 0xFF);
                ((top * alpha + bottom * (1.0 - alpha)).round() as u32) << shift
            };
            Rgb(mix(16) | mix(8) | mix(0))
        }

        /// WCAG 2.x contrast ratio between two colors (1 to 21).
        fn contrast(self, other: Rgb) -> f64 {
            let (a, b) = (self.luminance(), other.luminance());
            (a.max(b) + 0.05) / (a.min(b) + 0.05)
        }
    }

    /// Text must reach this against every ground it sits on.
    fn text_minimum(theme: UiTheme) -> f64 {
        match theme.family() {
            ThemeFamily::HighContrast => 7.0,
            _ => 4.5,
        }
    }

    fn assert_contrast(theme: UiTheme, what: &str, fg: Rgb, bg: Rgb, minimum: f64) {
        let ratio = fg.contrast(bg);
        assert!(
            ratio >= minimum,
            "{theme}: {what} is {ratio:.2}:1, needs {minimum}:1"
        );
    }

    #[test]
    fn contrast_of_known_pairs() {
        assert!((Rgb(0x000000).contrast(Rgb(0xFFFFFF)) - 21.0).abs() < 1e-9);
        assert!((Rgb(0x777777).contrast(Rgb(0x777777)) - 1.0).abs() < 1e-9);
        // #767676 on white is the classic 4.54:1.
        assert!((Rgb(0x767676).contrast(Rgb(0xFFFFFF)) - 4.54).abs() < 0.01);
        assert_eq!(Rgb(0xFFFFFF).over(Rgb(0x000000), 0.5), Rgb(0x808080));
    }

    #[test]
    fn every_palette_matches_its_theme() {
        for theme in UiTheme::ALL {
            let p = palette(theme);
            let dark = p.text.luminance() > p.surface.luminance();
            assert_eq!(dark, theme.mode() == ThemeMode::Dark, "{theme}");
            let terminal = theme.family() == ThemeFamily::Terminal;
            assert_eq!(p.radius == 0, terminal, "{theme}");
            assert_eq!(p.font == UiFont::JetBrainsMono, terminal, "{theme}");
            let outline = if theme.family() == ThemeFamily::HighContrast {
                2
            } else {
                1
            };
            assert_eq!(p.border_width, outline, "{theme}");
        }
    }

    #[test]
    fn every_theme_meets_wcag_contrast() {
        for theme in UiTheme::ALL {
            let p = palette(theme);
            let text = text_minimum(theme);
            let grounds = [("surface", p.surface), ("app", p.app), ("sunken", p.sunken)];
            let inks = [
                ("text", p.text),
                ("text2", p.text2),
                ("text3", p.text3),
                ("accent text", p.accent_text),
            ];
            for (ink_name, ink) in inks {
                for (ground_name, ground) in grounds {
                    let what = format!("{ink_name} on {ground_name}");
                    assert_contrast(theme, &what, ink, ground, text);
                }
            }
            for (name, ink) in [("text", p.text), ("text2", p.text2)] {
                assert_contrast(theme, &format!("{name} on selected"), ink, p.selected, text);
                assert_contrast(theme, &format!("{name} on hover"), ink, p.hover, text);
            }
            let statuses = [
                ("success", p.success, p.success_bg),
                ("warning", p.warning, p.warning_bg),
                ("danger", p.danger, p.danger_bg),
                ("info", p.info, p.info_bg),
            ];
            for (name, ink, tint) in statuses {
                // Notices sit on the app ground, chips on their tint.
                assert_contrast(theme, &format!("{name} on surface"), ink, p.surface, text);
                assert_contrast(theme, &format!("{name} on app"), ink, p.app, text);
                assert_contrast(theme, &format!("{name} on its tint"), ink, tint, text);
            }
            let chip = "accent text on selected";
            assert_contrast(theme, chip, p.accent_text, p.selected, text);
            // gpui-kit fills a dark theme's inputs with 30% of the control
            // outline over the card.
            if theme.mode() == ThemeMode::Dark {
                let fill = p.border_strong.over(p.surface, 0.3);
                for (name, ink) in [("text", p.text), ("text2", p.text2)] {
                    assert_contrast(theme, &format!("{name} in an input"), ink, fill, text);
                }
            }
            assert_contrast(theme, "accent ink on accent", p.on_accent, p.accent, text);
            for (ground_name, ground) in [("surface", p.surface), ("app", p.app)] {
                let control = format!("control outline on {ground_name}");
                assert_contrast(theme, &control, p.border_strong, ground, 3.0);
                let ring = format!("focus ring on {ground_name}");
                assert_contrast(theme, &ring, p.focus, ground, 3.0);
            }
        }
    }
}
