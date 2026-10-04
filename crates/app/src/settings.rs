//! Settings screen: Connections (#5), the Restricted Features (#7), Backup
//! (#6), Updates (#8), then Appearance, which picks the layout (#41) and
//! the interface theme (#39).
//! Later settings join it.

use gpui_kit::assets::IconName;
use gpui_kit::component::searchable_list::{SearchableListItem, SearchableVec};
use gpui_kit::component::select::{Select, SelectEvent, SelectState};
use gpui_kit::component::{Icon, IndexPath, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, ClickEvent, Entity, Hsla, MouseButton, SharedString, Subscription, Window, div, px,
};
use mascate_platform::{Appearance, LayoutId, ThemeFamily, ThemeMode, UiTheme, UiThemePreference};

use crate::appearance::{self, color, look};
use crate::backups::BackupSection;
use crate::connections::ConnectionsSection;
use crate::kit;
use crate::layout;
use crate::palette::palette;
use crate::parts::ScreenParts;
use crate::preferences;
use crate::restricted::RestrictedFeaturesSection;
use crate::updates::UpdatesSection;

fn theme_name(theme: UiTheme) -> &'static str {
    match theme {
        UiTheme::Paper => "Papel",
        UiTheme::Sand => "Areia",
        UiTheme::Graphite => "Grafite",
        UiTheme::Slate => "Ardósia",
        UiTheme::HighContrastLight => "Alto contraste claro",
        UiTheme::HighContrastDark => "Alto contraste escuro",
        UiTheme::BlackGold => "Ouro Negro",
        UiTheme::Brass => "Latão",
        UiTheme::Phosphor => "Fósforo",
        UiTheme::PhosphorLight => "Fósforo Claro",
    }
}

fn theme_kind(theme: UiTheme) -> &'static str {
    match (theme.family(), theme.mode()) {
        (ThemeFamily::Terminal, ThemeMode::Light) => "Terminal, claro",
        (ThemeFamily::Terminal, ThemeMode::Dark) => "Terminal, escuro",
        (_, ThemeMode::Light) => "Claro",
        (_, ThemeMode::Dark) => "Escuro",
    }
}

fn layout_name(layout: LayoutId) -> &'static str {
    match layout {
        LayoutId::Workspace => "Workspace",
        LayoutId::Studio => "Studio",
    }
}

fn layout_description(layout: LayoutId) -> &'static str {
    match layout {
        LayoutId::Workspace => "Barra lateral com os lugares e páginas com espaço.",
        LayoutId::Studio => "Abas no topo, telas compactas e barra de status.",
    }
}

/// One theme in a light or dark picker.
#[derive(Clone)]
struct ThemeChoice {
    value: UiTheme,
    title: SharedString,
}

impl SearchableListItem for ThemeChoice {
    type Value = UiTheme;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &UiTheme {
        &self.value
    }
}

type ThemeSelect = Entity<SelectState<SearchableVec<ThemeChoice>>>;

fn theme_choices(mode: ThemeMode) -> SearchableVec<ThemeChoice> {
    SearchableVec::new(
        UiTheme::of_mode(mode)
            .map(|value| ThemeChoice {
                value,
                title: theme_name(value).into(),
            })
            .collect::<Vec<_>>(),
    )
}

pub struct SettingsScreen {
    connections: Entity<ConnectionsSection>,
    restricted: Entity<RestrictedFeaturesSection>,
    backup: Entity<BackupSection>,
    updates: Entity<UpdatesSection>,
    /// The light and dark slots of "follow the system", kept while a fixed
    /// theme is on so following again restores them.
    follow_pair: (UiTheme, UiTheme),
    light_theme: ThemeSelect,
    dark_theme: ThemeSelect,
    /// Numbers the appearance saves, so only the newest one's outcome shows.
    saves: u64,
    /// Why the last appearance change was not saved.
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl SettingsScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let follow_pair = preferences::appearance(cx).theme.follow_pair();
        let (light, dark) = follow_pair;
        let mut select = |mode, theme: UiTheme| {
            let at = UiTheme::of_mode(mode).position(|t| t == theme).unwrap_or(0);
            cx.new(|cx| SelectState::new(theme_choices(mode), Some(IndexPath::new(at)), window, cx))
        };
        let light_theme = select(ThemeMode::Light, light);
        let dark_theme = select(ThemeMode::Dark, dark);
        let subscriptions = [&light_theme, &dark_theme]
            .map(|select| {
                cx.subscribe_in(
                    select,
                    window,
                    move |this, _, event: &SelectEvent<SearchableVec<ThemeChoice>>, window, cx| {
                        let SelectEvent::Confirm(Some(theme)) = event else {
                            return;
                        };
                        let (light, dark) = this.following().follow_pair_with(*theme);
                        this.set_theme(UiThemePreference::FollowSystem { light, dark }, window, cx);
                    },
                )
            })
            .into();
        Self {
            connections: cx.new(|cx| ConnectionsSection::new(window, cx)),
            restricted: cx.new(RestrictedFeaturesSection::new),
            backup: cx.new(|cx| BackupSection::new(window, cx)),
            updates: cx.new(UpdatesSection::new),
            follow_pair,
            light_theme,
            dark_theme,
            saves: 0,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    /// "Follow the system" with the pair this screen keeps.
    fn following(&self) -> UiThemePreference {
        let (light, dark) = self.follow_pair;
        UiThemePreference::FollowSystem { light, dark }
    }

    fn set_theme(
        &mut self,
        preference: UiThemePreference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let saved = preferences::appearance(cx);
        if preference == saved.theme {
            return;
        }
        self.save(
            Appearance {
                theme: preference,
                ..saved
            },
            cx,
        );
        appearance::follow(preference, appearance::system_mode(window), cx);
        // The pickers show the pair "follow the system" would use now.
        self.follow_pair = match preference {
            UiThemePreference::FollowSystem { light, dark } => (light, dark),
            UiThemePreference::Fixed(theme) => self.following().follow_pair_with(theme),
        };
        let (light, dark) = self.follow_pair;
        for (select, theme) in [
            (self.light_theme.clone(), light),
            (self.dark_theme.clone(), dark),
        ] {
            select.update(cx, |select, cx| {
                select.set_selected_value(&theme, window, cx)
            });
        }
    }

    fn set_layout(&mut self, layout: LayoutId, cx: &mut Context<Self>) {
        let saved = preferences::appearance(cx);
        if layout == saved.layout {
            return;
        }
        self.save(Appearance { layout, ..saved }, cx);
        layout::show(layout, cx);
    }

    fn save(&mut self, appearance: Appearance, cx: &mut Context<Self>) {
        self.error = None;
        self.saves += 1;
        let save = self.saves;
        let saving = preferences::set_appearance(appearance, cx);
        cx.spawn(async move |this, cx| {
            let error = saving.await.err();
            let _ = this.update(cx, |this, cx| {
                if this.saves == save {
                    this.error = error.map(SharedString::from);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn render_appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let saved = preferences::appearance(cx);
        let layouts: Vec<AnyElement> = LayoutId::ALL
            .into_iter()
            .map(|layout| self.layout_card(layout, layout == saved.layout, cx))
            .collect();
        let fixed = match saved.theme {
            UiThemePreference::Fixed(theme) => Some(theme),
            UiThemePreference::FollowSystem { .. } => None,
        };
        let cards: Vec<AnyElement> = UiTheme::ALL
            .into_iter()
            .map(|theme| self.theme_card(theme, fixed == Some(theme), cx))
            .collect();
        let t = look(cx).tokens;
        let following = fixed.is_none();

        let option = |id: &'static str, on: bool, title: &'static str, hint: &'static str| {
            // Wraps the follow row's pickers under its text when the window
            // (or a monospace theme font) leaves no room beside it.
            h_flex()
                .id(id)
                .flex_wrap()
                .gap_3()
                .p_3()
                .rounded(t.radius_lg)
                .border(t.border_width)
                .border_color(if on { t.accent_edge } else { t.frame })
                .bg(if on { t.selected } else { t.surface })
                .cursor_pointer()
                .child(kit::radio(on, cx))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w(px(240.))
                        .child(div().font_medium().child(title))
                        .child(div().text_xs().text_color(t.text2).child(hint)),
                )
        };

        let follow = option(
            "theme-follow",
            following,
            "Seguir o sistema",
            "Muda entre um tema claro e um escuro junto com o Windows ou o Linux.",
        )
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            this.set_theme(this.following(), window, cx);
        }))
        .child(
            h_flex()
                .flex_wrap()
                .gap_2()
                // Opening a picker is not a click on the row: from a fixed
                // theme it would switch to following the system at once.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(div().text_sm().text_color(t.text2).child("Claro"))
                .child(
                    div()
                        .w(px(200.))
                        .child(Select::new(&self.light_theme).small()),
                )
                .child(div().text_sm().text_color(t.text2).child("Escuro"))
                .child(
                    div()
                        .w(px(200.))
                        .child(Select::new(&self.dark_theme).small()),
                ),
        );

        let always = option(
            "theme-fixed",
            !following,
            "Sempre o mesmo tema",
            "Escolha um dos temas abaixo.",
        )
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
            let current = look(cx).theme;
            this.set_theme(UiThemePreference::Fixed(current), window, cx);
        }));

        v_flex()
            .gap_3()
            .child(kit::section_heading("Layout"))
            .child(
                h_flex()
                    .flex_wrap()
                    .items_stretch()
                    .gap_3()
                    .children(layouts),
            )
            .child(div().h_2())
            .child(kit::section_heading("Tema"))
            .child(follow)
            .child(always)
            .child(h_flex().flex_wrap().gap_3().children(cards))
            .into_any_element()
    }

    /// A layout as a sketch of where things go, its name and one line.
    fn layout_card(&self, layout: LayoutId, chosen: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let block = |color: Hsla| div().rounded(t.radius).bg(color);
        let sketch = match layout {
            LayoutId::Workspace => h_flex()
                .size_full()
                .gap_1p5()
                .child(
                    v_flex()
                        .w(px(34.))
                        .h_full()
                        .gap_1()
                        .p_1()
                        .bg(t.surface)
                        .child(block(t.accent).w_full().h(px(5.)))
                        .child(block(t.border_strong).w_full().h(px(5.)))
                        .child(block(t.border_strong).w_full().h(px(5.))),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .h_full()
                        .gap_1()
                        .py_1()
                        .child(block(t.border_strong).w(px(60.)).h(px(6.)))
                        .child(div().grid().grid_cols(3).gap_1().children((0..6).map(|ix| {
                            block(if ix == 2 { t.accent } else { t.sunken })
                                .h(px(18.))
                                .border(t.border_width)
                                .border_color(t.border)
                        }))),
                )
                .into_any_element(),
            LayoutId::Studio => v_flex()
                .size_full()
                .gap_1()
                .child(
                    h_flex()
                        .gap_1()
                        .p_1()
                        .bg(t.surface)
                        .child(block(t.accent).w(px(14.)).h(px(5.)))
                        .children((0..4).map(|_| block(t.border_strong).w(px(18.)).h(px(5.)))),
                )
                .child(v_flex().flex_1().px_1().gap_1().children((0..3).map(|ix| {
                    h_flex()
                        .gap_1()
                        .child(block(t.text2).w(px(10.)).h(px(6.)))
                        .child(
                            block(if ix == 1 { t.accent } else { t.border_strong })
                                .flex_1()
                                .h(px(6.)),
                        )
                })))
                .child(block(t.surface).w_full().h(px(6.)))
                .into_any_element(),
        };
        v_flex()
            .id(("layout-card", layout as usize))
            .w(px(260.))
            .overflow_hidden()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if chosen { t.accent } else { t.frame })
            .when(chosen, |card| card.border_2())
            .bg(t.surface)
            .cursor_pointer()
            .hover(|card| card.border_color(t.accent_edge))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.set_layout(layout, cx)))
            .child(div().h(px(84.)).p_2().bg(t.app).child(sketch))
            .child(
                v_flex()
                    .px_2p5()
                    .py_2()
                    .gap_0p5()
                    .border_t_1()
                    .border_color(t.border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(kit::radio(chosen, cx))
                            .child(div().text_sm().font_medium().child(layout_name(layout))),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text2)
                            .child(layout_description(layout)),
                    ),
            )
            .into_any_element()
    }

    /// A theme in its own colors: a small preview, its name and kind, and
    /// the contrast level its text meets.
    fn theme_card(&self, theme: UiTheme, chosen: bool, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let p = palette(theme);
        let radius = px(f32::from(p.radius));
        let bar = |width: f32, ink: Hsla| div().h(px(5.)).w(px(width)).rounded(radius).bg(ink);
        let preview = h_flex()
            .h(px(70.))
            .bg(color(p.app))
            .child(
                v_flex()
                    .w(px(40.))
                    .h_full()
                    .gap_1()
                    .p_2()
                    .bg(color(p.surface))
                    .border_r_1()
                    .border_color(color(p.border))
                    .child(bar(22., color(p.text3)))
                    .child(bar(22., color(p.accent)))
                    .child(bar(22., color(p.text3))),
            )
            .child(
                v_flex()
                    .flex_1()
                    .gap_1()
                    .p_2()
                    .child(bar(46., color(p.text)))
                    .child(bar(80., color(p.text2)))
                    .child(bar(60., color(p.text3)))
                    .child(
                        h_flex()
                            .gap_1p5()
                            .mt_1()
                            .child(
                                div()
                                    .h(px(12.))
                                    .w(px(36.))
                                    .rounded(radius)
                                    .bg(color(p.accent))
                                    .border_1()
                                    .border_color(color(p.accent_edge)),
                            )
                            .child(
                                div()
                                    .h(px(12.))
                                    .w(px(28.))
                                    .rounded(radius)
                                    .bg(color(p.success_bg))
                                    .border_1()
                                    .border_color(color(p.success)),
                            ),
                    ),
            );
        let level = if theme.family() == ThemeFamily::HighContrast {
            "AAA"
        } else {
            "AA"
        };
        v_flex()
            .id(("theme-card", theme as usize))
            .w(px(186.))
            .overflow_hidden()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if chosen { t.accent } else { t.frame })
            .when(chosen, |card| card.border_2())
            .bg(t.surface)
            .cursor_pointer()
            .hover(|card| card.border_color(t.accent_edge))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                this.set_theme(UiThemePreference::Fixed(theme), window, cx)
            }))
            .child(preview)
            .child(
                v_flex()
                    .px_2p5()
                    .py_2()
                    .border_t_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .text_sm()
                            .font_medium()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(theme_name(theme)),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .justify_between()
                            .child(div().text_xs().text_color(t.text2).child(theme_kind(theme)))
                            .child(
                                h_flex()
                                    .gap_0p5()
                                    .text_xs()
                                    .text_color(t.success)
                                    .child(Icon::new(IconName::Check).size(px(12.)))
                                    .child(level),
                            ),
                    ),
            )
            .into_any_element()
    }
}

impl Render for SettingsScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut parts = ScreenParts::new("Configurações");
        parts.notices.extend(
            self.error
                .clone()
                .map(|error| kit::error_notice(error, cx).into_any_element()),
        );
        parts.content.push(
            v_flex()
                .gap_3()
                .child(div().text_xl().font_semibold().child("Conexões"))
                .child(self.connections.clone())
                .into_any_element(),
        );
        parts.content.push(
            v_flex()
                .gap_3()
                .child(
                    div()
                        .text_xl()
                        .font_semibold()
                        .child("Funcionalidades restritas"),
                )
                .child(self.restricted.clone())
                .into_any_element(),
        );
        parts.content.push(
            v_flex()
                .gap_3()
                .child(div().text_xl().font_semibold().child("Backup"))
                .child(self.backup.clone())
                .into_any_element(),
        );
        parts.content.push(
            v_flex()
                .gap_3()
                .child(div().text_xl().font_semibold().child("Atualizações"))
                .child(self.updates.clone())
                .into_any_element(),
        );
        parts.content.push(
            v_flex()
                .gap_3()
                .child(div().text_xl().font_semibold().child("Aparência"))
                .child(self.render_appearance(cx))
                .into_any_element(),
        );
        layout::screen(parts, cx)
    }
}
