//! How the app's own interface looks: the Interface Theme and how the owner
//! picks it, the Layout that places every screen, and where both are kept.

use std::fmt;
use std::str::FromStr;

use libsql::Value;
use mascate_kernel::{Clock, IdGenerator};

use crate::{Database, Migration, single_row};

/// One of the interface themes Mascate ships. A theme is colors, corner
/// radius, border width and font; it never moves anything on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiTheme {
    Paper,
    Sand,
    Graphite,
    Slate,
    HighContrastLight,
    HighContrastDark,
    BlackGold,
    Brass,
    Phosphor,
    PhosphorLight,
}

/// Light or dark: which slot of "follow the system" a theme fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThemeMode {
    Light,
    Dark,
}

/// `Base` themes share the amber accent, rounded corners and the system
/// font; `Terminal` themes, after Omarchy's desktop themes, bring their own
/// accent, square corners, framed panels and a monospace font. High
/// contrast is a base theme with stronger pairs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThemeFamily {
    Base,
    HighContrast,
    Terminal,
}

impl UiTheme {
    pub const ALL: [UiTheme; 10] = [
        UiTheme::Paper,
        UiTheme::Sand,
        UiTheme::Graphite,
        UiTheme::Slate,
        UiTheme::HighContrastLight,
        UiTheme::HighContrastDark,
        UiTheme::BlackGold,
        UiTheme::Brass,
        UiTheme::Phosphor,
        UiTheme::PhosphorLight,
    ];

    /// Stable code, used in storage.
    pub fn code(self) -> &'static str {
        match self {
            UiTheme::Paper => "paper",
            UiTheme::Sand => "sand",
            UiTheme::Graphite => "graphite",
            UiTheme::Slate => "slate",
            UiTheme::HighContrastLight => "hc-light",
            UiTheme::HighContrastDark => "hc-dark",
            UiTheme::BlackGold => "black-gold",
            UiTheme::Brass => "brass",
            UiTheme::Phosphor => "phosphor",
            UiTheme::PhosphorLight => "phosphor-light",
        }
    }

    pub fn mode(self) -> ThemeMode {
        match self {
            UiTheme::Paper
            | UiTheme::Sand
            | UiTheme::HighContrastLight
            | UiTheme::Brass
            | UiTheme::PhosphorLight => ThemeMode::Light,
            UiTheme::Graphite
            | UiTheme::Slate
            | UiTheme::HighContrastDark
            | UiTheme::BlackGold
            | UiTheme::Phosphor => ThemeMode::Dark,
        }
    }

    pub fn family(self) -> ThemeFamily {
        match self {
            UiTheme::Paper | UiTheme::Sand | UiTheme::Graphite | UiTheme::Slate => {
                ThemeFamily::Base
            }
            UiTheme::HighContrastLight | UiTheme::HighContrastDark => ThemeFamily::HighContrast,
            UiTheme::BlackGold | UiTheme::Brass | UiTheme::Phosphor | UiTheme::PhosphorLight => {
                ThemeFamily::Terminal
            }
        }
    }

    /// The themes that fit one slot of "follow the system".
    pub fn of_mode(mode: ThemeMode) -> impl Iterator<Item = UiTheme> {
        UiTheme::ALL
            .into_iter()
            .filter(move |theme| theme.mode() == mode)
    }
}

impl fmt::Display for UiTheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown interface theme: {0}")]
pub struct UnknownUiTheme(pub String);

impl FromStr for UiTheme {
    type Err = UnknownUiTheme;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        UiTheme::ALL
            .into_iter()
            .find(|theme| theme.code() == s)
            .ok_or_else(|| UnknownUiTheme(s.to_owned()))
    }
}

/// How the owner picks the theme: follow the system's light/dark setting
/// with a theme for each, or always the same theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiThemePreference {
    FollowSystem { light: UiTheme, dark: UiTheme },
    Fixed(UiTheme),
}

impl Default for UiThemePreference {
    fn default() -> Self {
        UiThemePreference::FollowSystem {
            light: UiTheme::Paper,
            dark: UiTheme::Graphite,
        }
    }
}

impl UiThemePreference {
    /// The theme to show while the system appearance is `system`.
    pub fn resolve(self, system: ThemeMode) -> UiTheme {
        match self {
            UiThemePreference::FollowSystem { light, dark } => match system {
                ThemeMode::Light => light,
                ThemeMode::Dark => dark,
            },
            UiThemePreference::Fixed(theme) => theme,
        }
    }

    /// The light and dark themes "follow the system" uses: the saved pair,
    /// or the default pair with a fixed theme in its own slot, so switching
    /// back to following keeps what was on screen.
    pub fn follow_pair(self) -> (UiTheme, UiTheme) {
        match self {
            UiThemePreference::FollowSystem { light, dark } => (light, dark),
            UiThemePreference::Fixed(theme) => UiThemePreference::default().follow_pair_with(theme),
        }
    }

    /// The pair "follow the system" uses with `theme` in its own slot and
    /// this preference's pair in the other.
    pub fn follow_pair_with(self, theme: UiTheme) -> (UiTheme, UiTheme) {
        let (light, dark) = self.follow_pair();
        match theme.mode() {
            ThemeMode::Light => (theme, dark),
            ThemeMode::Dark => (light, theme),
        }
    }

    /// Stored form: `system:<light>:<dark>` or `fixed:<theme>`.
    pub fn code(self) -> String {
        match self {
            UiThemePreference::FollowSystem { light, dark } => format!("system:{light}:{dark}"),
            UiThemePreference::Fixed(theme) => format!("fixed:{theme}"),
        }
    }

    /// Reads the stored form; anything this version does not know (a theme
    /// removed later, a hand-edited value) gives the default.
    pub fn from_code_or_default(code: &str) -> Self {
        code.parse().unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown interface theme preference: {0}")]
pub struct UnknownUiThemePreference(pub String);

impl FromStr for UiThemePreference {
    type Err = UnknownUiThemePreference;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let unknown = || UnknownUiThemePreference(s.to_owned());
        let parts: Vec<&str> = s.split(':').collect();
        match parts.as_slice() {
            ["system", light, dark] => {
                let light: UiTheme = light.parse().map_err(|_| unknown())?;
                let dark: UiTheme = dark.parse().map_err(|_| unknown())?;
                if light.mode() != ThemeMode::Light || dark.mode() != ThemeMode::Dark {
                    return Err(unknown());
                }
                Ok(UiThemePreference::FollowSystem { light, dark })
            }
            ["fixed", theme] => Ok(UiThemePreference::Fixed(
                theme.parse().map_err(|_| unknown())?,
            )),
            _ => Err(unknown()),
        }
    }
}

/// Where the screens' parts go. A layout moves things on screen and never
/// changes what they do; the theme colors them. A new layout is a variant
/// here and an arrangement in the interface, nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LayoutId {
    /// A sidebar of places grouped by area, pages with room around them.
    #[default]
    Workspace,
    /// Denser: top tabs, compact headers and a status bar.
    Studio,
}

impl LayoutId {
    pub const ALL: [LayoutId; 2] = [LayoutId::Workspace, LayoutId::Studio];

    /// Stable code, used in storage.
    pub fn code(self) -> &'static str {
        match self {
            LayoutId::Workspace => "workspace",
            LayoutId::Studio => "studio",
        }
    }

    /// Reads the stored form; a layout this version does not know gives the
    /// default.
    pub fn from_code_or_default(code: &str) -> Self {
        code.parse().unwrap_or_default()
    }
}

impl fmt::Display for LayoutId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown layout: {0}")]
pub struct UnknownLayout(pub String);

impl FromStr for LayoutId {
    type Err = UnknownLayout;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        LayoutId::ALL
            .into_iter()
            .find(|layout| layout.code() == s)
            .ok_or_else(|| UnknownLayout(s.to_owned()))
    }
}

/// The owner's interface choices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Appearance {
    pub theme: UiThemePreference,
    pub layout: LayoutId,
}

const TABLE: &str = "platform_appearance";
const COLUMNS: &[&str] = &["theme", "layout"];

pub(crate) const CREATE_APPEARANCE: Migration = Migration {
    version: 1,
    name: "create appearance",
    sql: "CREATE TABLE platform_appearance (
        id         TEXT PRIMARY KEY,
        theme      TEXT NOT NULL,
        layout     TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );",
};

/// The saved appearance, or the default before anything was saved.
pub async fn load_appearance(database: &Database) -> Result<Appearance, libsql::Error> {
    let Some(row) = single_row::load(database.connection(), TABLE, COLUMNS).await? else {
        return Ok(Appearance::default());
    };
    Ok(Appearance {
        theme: UiThemePreference::from_code_or_default(&row.get::<String>(0)?),
        layout: LayoutId::from_code_or_default(&row.get::<String>(1)?),
    })
}

/// Saves `appearance` over the one already saved, if any.
pub async fn save_appearance(
    database: &Database,
    clock: &dyn Clock,
    ids: &dyn IdGenerator,
    appearance: Appearance,
) -> Result<(), libsql::Error> {
    single_row::save(
        database.connection(),
        clock,
        ids,
        TABLE,
        COLUMNS,
        vec![
            Value::Text(appearance.theme.code()),
            Value::Text(appearance.layout.code().into()),
        ],
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_codes_round_trip_and_are_distinct() {
        for theme in UiTheme::ALL {
            assert_eq!(theme.code().parse::<UiTheme>(), Ok(theme));
        }
        let mut codes: Vec<_> = UiTheme::ALL.iter().map(|t| t.code()).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), UiTheme::ALL.len());
    }

    #[test]
    fn five_light_and_five_dark_themes_with_two_of_each_terminal() {
        assert_eq!(UiTheme::of_mode(ThemeMode::Light).count(), 5);
        assert_eq!(UiTheme::of_mode(ThemeMode::Dark).count(), 5);
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let terminal = UiTheme::of_mode(mode)
                .filter(|theme| theme.family() == ThemeFamily::Terminal)
                .count();
            assert_eq!(terminal, 2, "{mode:?}");
        }
    }

    #[test]
    fn default_follows_the_system_with_paper_and_graphite() {
        let preference = UiThemePreference::default();
        assert_eq!(preference.resolve(ThemeMode::Light), UiTheme::Paper);
        assert_eq!(preference.resolve(ThemeMode::Dark), UiTheme::Graphite);
    }

    #[test]
    fn a_fixed_theme_ignores_the_system() {
        let preference = UiThemePreference::Fixed(UiTheme::BlackGold);
        assert_eq!(preference.resolve(ThemeMode::Light), UiTheme::BlackGold);
        assert_eq!(preference.resolve(ThemeMode::Dark), UiTheme::BlackGold);
    }

    #[test]
    fn following_again_keeps_the_fixed_theme_in_its_slot() {
        assert_eq!(
            UiThemePreference::Fixed(UiTheme::Phosphor).follow_pair(),
            (UiTheme::Paper, UiTheme::Phosphor)
        );
        assert_eq!(
            UiThemePreference::Fixed(UiTheme::Brass).follow_pair(),
            (UiTheme::Brass, UiTheme::Graphite)
        );
        let saved = UiThemePreference::FollowSystem {
            light: UiTheme::Sand,
            dark: UiTheme::Slate,
        };
        assert_eq!(saved.follow_pair(), (UiTheme::Sand, UiTheme::Slate));
    }

    #[test]
    fn a_new_theme_in_one_slot_keeps_the_other_slot() {
        let saved = UiThemePreference::FollowSystem {
            light: UiTheme::Sand,
            dark: UiTheme::Slate,
        };
        assert_eq!(
            saved.follow_pair_with(UiTheme::Brass),
            (UiTheme::Brass, UiTheme::Slate)
        );
        assert_eq!(
            saved.follow_pair_with(UiTheme::BlackGold),
            (UiTheme::Sand, UiTheme::BlackGold)
        );
    }

    #[test]
    fn preferences_round_trip_through_their_code() {
        let preferences = [
            UiThemePreference::default(),
            UiThemePreference::FollowSystem {
                light: UiTheme::Brass,
                dark: UiTheme::BlackGold,
            },
            UiThemePreference::Fixed(UiTheme::PhosphorLight),
        ];
        for preference in preferences {
            assert_eq!(preference.code().parse(), Ok(preference));
        }
        assert_eq!(UiThemePreference::default().code(), "system:paper:graphite");
        assert_eq!(
            UiThemePreference::Fixed(UiTheme::BlackGold).code(),
            "fixed:black-gold"
        );
    }

    #[test]
    fn unknown_preferences_fall_back_to_the_default() {
        for code in [
            "",
            "fixed:neon",
            "system:paper",
            "system:graphite:paper",
            "auto",
            "fixed:paper:extra",
        ] {
            assert_eq!(
                UiThemePreference::from_code_or_default(code),
                UiThemePreference::default(),
                "{code}"
            );
        }
    }

    #[test]
    fn unknown_layouts_fall_back_to_workspace() {
        for layout in LayoutId::ALL {
            assert_eq!(LayoutId::from_code_or_default(layout.code()), layout);
        }
        for code in ["", "Studio", "dashboard", "studio "] {
            assert_eq!(
                LayoutId::from_code_or_default(code),
                LayoutId::Workspace,
                "{code:?}"
            );
        }
    }
}
