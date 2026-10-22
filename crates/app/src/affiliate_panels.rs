//! The logged-in affiliate panels as Afiliação uses them (#160, ADR 0035
//! decision 4): the one browser they share, each panel built only while
//! its Restricted Feature is on, and its Connection on the screen, which
//! lives in the browser's own profile: whether a browser is installed,
//! "Entrar no painel" to log in by hand in its window, and "Ler painel
//! agora". The hourly Commissions Sync reads every panel that is on and
//! logged in.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, EventEmitter, Global, Window, div};
use mascate_affiliate::AffiliateError;
use mascate_integrations::{
    AffiliatePanel, BrowserSession, MAGALU_AFFILIATE_PANEL, MAGALU_AFFILIATE_PANEL_URL,
    MERCADO_LIVRE_AFFILIATE_PANEL, MERCADO_LIVRE_AFFILIATE_PANEL_URL, MagaluAffiliatePanel,
    MercadoLivreAffiliatePanel, PanelState,
};
use mascate_kernel::PlatformError;
use mascate_platform::{Build, Environment, Flag, FlagError, Flags};

use crate::affiliate::{CommissionSyncs, affiliate, failure, recorded_text};
use crate::appearance::look;
use crate::forms::{Outcome, notice};
use crate::kit;
use crate::restricted::AppFlags;

/// The browser every panel shares, so one profile holds every login and
/// one browser runs at a time, and the panels it can read.
pub struct Panels {
    pub browser: Arc<BrowserSession>,
    pub sites: Vec<PanelSite>,
}

pub struct AppAffiliatePanels(pub Arc<Panels>);

impl Global for AppAffiliatePanels {}

pub fn panels(cx: &App) -> Option<Arc<Panels>> {
    cx.try_global::<AppAffiliatePanels>()
        .map(|app| app.0.clone())
}

/// Where the browser keeps the Mascate's own profile, with the logins:
/// beside the database, never the owner's daily profile.
pub fn profile_folder(data: PathBuf) -> PathBuf {
    data.join("navegador")
}

/// A Platform's affiliate panel.
pub struct PanelSite {
    /// The Platform as the screen names it: "do Mercado Livre".
    pub of: &'static str,
    pub flag: Flag,
    pub url: String,
    build: fn(&str, Arc<BrowserSession>) -> Arc<dyn AffiliatePanel>,
    /// Built the first time its flag is found on, and kept, so its state
    /// lasts while the app runs.
    built: OnceLock<Arc<dyn AffiliatePanel>>,
}

impl PanelSite {
    /// Every panel, in the order Afiliação shows them.
    pub fn all(build: Build, environment: &Environment) -> Vec<Self> {
        vec![
            Self::mercado_livre(build, environment),
            Self::magalu(build, environment),
        ]
    }

    /// Mercado Livre's affiliate panel or, in a development build only,
    /// the URL in `MASCATE_MERCADO_LIVRE_AFFILIATE_PANEL_URL`, a local fake
    /// page.
    pub fn mercado_livre(build: Build, environment: &Environment) -> Self {
        Self {
            of: "do Mercado Livre",
            flag: MERCADO_LIVRE_AFFILIATE_PANEL,
            url: crate::mercado_livre::local_fake(
                build,
                environment,
                "MASCATE_MERCADO_LIVRE_AFFILIATE_PANEL_URL",
            )
            .unwrap_or_else(|| MERCADO_LIVRE_AFFILIATE_PANEL_URL.to_owned()),
            build: |url, browser| Arc::new(MercadoLivreAffiliatePanel::new(url, browser)),
            built: OnceLock::new(),
        }
    }

    /// The Parceiro Magalu panel or, in a development build only, the URL
    /// in `MASCATE_MAGALU_AFFILIATE_PANEL_URL`, a local fake page.
    pub fn magalu(build: Build, environment: &Environment) -> Self {
        Self {
            of: "da Magalu",
            flag: MAGALU_AFFILIATE_PANEL,
            url: crate::mercado_livre::local_fake(
                build,
                environment,
                "MASCATE_MAGALU_AFFILIATE_PANEL_URL",
            )
            .unwrap_or_else(|| MAGALU_AFFILIATE_PANEL_URL.to_owned()),
            build: |url, browser| Arc::new(MagaluAffiliatePanel::new(url, browser)),
            built: OnceLock::new(),
        }
    }
}

impl Panels {
    /// The panels whose flag is on, each with its reader; off, a panel is
    /// never built and no browser opens for it.
    pub async fn on(
        &self,
        flags: &Flags,
    ) -> Result<Vec<(&PanelSite, Arc<dyn AffiliatePanel>)>, FlagError> {
        let mut on = Vec::new();
        for site in &self.sites {
            if flags.is_on(&site.flag).await? {
                let panel = site
                    .built
                    .get_or_init(|| (site.build)(&site.url, self.browser.clone()))
                    .clone();
                on.push((site, panel));
            }
        }
        Ok(on)
    }
}

/// Why a panel's reading failed, as the owner reads it.
pub fn failure_text(site: &PanelSite, error: &AffiliateError) -> String {
    let AffiliateError::Platform(error) = error else {
        return failure(error);
    };
    let of = site.of;
    match error {
        PlatformError::NotConnected => format!(
            "Falta entrar no painel de afiliado {of} uma vez: clique em Entrar no painel, aqui \
             em Afiliação."
        ),
        PlatformError::Expired => format!(
            "O painel de afiliado {of} pediu login de novo: clique em Entrar no painel, entre na \
             janela que abrir e depois em Ler painel agora."
        ),
        PlatformError::Refused(why) => format!("Não li o painel de afiliado {of}: {why}."),
        PlatformError::RateLimited => format!(
            "O painel de afiliado {of} pediu para esperar; a próxima leitura tenta de novo."
        ),
        PlatformError::NotFound => format!("O painel de afiliado {of} não abriu o relatório."),
        PlatformError::Failed(why) => {
            format!("Não consegui ler o painel de afiliado {of}: {why}")
        }
    }
}

/// What the panel's state means for the owner.
fn state_text(site: &PanelSite, state: PanelState) -> String {
    let of = site.of;
    match state {
        PanelState::NoBrowser => format!(
            "Não há Edge nem Chrome instalado neste computador, e o app lê o painel {of} por um \
             deles. Instale um e abra o app de novo."
        ),
        PanelState::NeverLoggedIn => format!(
            "Falta entrar uma vez: clique em Entrar no painel, entre com a sua conta {of} na \
             janela que abrir e feche a janela quando o painel aparecer. A senha fica só no \
             navegador."
        ),
        PanelState::LoggedOut => format!(
            "A última leitura caiu no login {of}. Clique em Entrar no painel, entre de novo na \
             janela e depois em Ler painel agora; até lá, a leitura de cada hora espera."
        ),
        PanelState::Ready => format!(
            "Conectado pelo navegador do Mascate, num perfil só dele. O app lê o relatório {of} \
             a cada hora, sem janela, e só lê: não clica, não digita senha e não resolve captcha."
        ),
    }
}

/// A panel's reading recorded Commissions.
pub struct PanelRead;

/// One panel that is on, as the screen shows it.
struct ShownPanel {
    site: usize,
    panel: Arc<dyn AffiliatePanel>,
    state: PanelState,
}

/// The panels that are on, each with its Connection and its buttons;
/// nothing while every panel's flag is off.
#[derive(Default)]
pub struct AffiliatePanelsSection {
    shown: Vec<ShownPanel>,
    /// The panel working now, by site.
    busy: Option<usize>,
    /// What the last button did, by site.
    outcome: Option<(usize, Outcome)>,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
}

impl EventEmitter<PanelRead> for AffiliatePanelsSection {}

impl AffiliatePanelsSection {
    /// Reads which panels are on and their state again, as when the screen
    /// comes into view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(panels), Some(flags)) = (
            panels(cx),
            cx.try_global::<AppFlags>().map(|app| app.flags.clone()),
        ) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        let reading = cx.background_executor().spawn(async move {
            panels.on(&flags).await.map(|on| {
                on.into_iter()
                    .map(|(site, panel)| ShownPanel {
                        site: panels
                            .sites
                            .iter()
                            .position(|known| std::ptr::eq(known, site))
                            .unwrap_or_default(),
                        state: panel.state(),
                        panel,
                    })
                    .collect::<Vec<_>>()
            })
        });
        cx.spawn_in(window, async move |this, cx| {
            let shown = reading.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                match shown {
                    Ok(shown) => this.shown = shown,
                    Err(error) => {
                        this.shown.clear();
                        this.outcome = Some((
                            0,
                            Outcome::Failed(
                                format!("Não consegui ler as funcionalidades restritas: {error}")
                                    .into(),
                            ),
                        ));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn log_in(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let shown = &self.shown[index];
        let (site, panel) = (shown.site, shown.panel.clone());
        self.busy = Some(site);
        self.outcome = None;
        let opening = cx
            .background_executor()
            .spawn(async move { panel.open_login() });
        cx.spawn_in(window, async move |this, cx| {
            let opened = opening.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                let Some(panels) = panels(cx) else {
                    return;
                };
                let site_info = &panels.sites[site];
                this.outcome = Some((
                    site,
                    match opened {
                        Ok(()) => Outcome::Done(
                            format!(
                                "Janela aberta no painel {}. Entre com a sua conta, confira que \
                                 o painel aparece e feche a janela; depois, Ler painel agora.",
                                site_info.of
                            )
                            .into(),
                        ),
                        Err(error) => Outcome::Failed(
                            failure_text(site_info, &AffiliateError::Platform(error)).into(),
                        ),
                    },
                ));
                this.refresh(window, cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn read(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(affiliate) = affiliate(cx) else {
            return;
        };
        let shown = &self.shown[index];
        let (site, panel) = (shown.site, shown.panel.clone());
        self.busy = Some(site);
        self.outcome = None;
        // Whatever the state says: the owner asked.
        let reading = cx
            .background_executor()
            .spawn(async move { affiliate.sync(panel.as_ref()).await });
        cx.spawn_in(window, async move |this, cx| {
            let read = reading.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                let Some(panels) = panels(cx) else {
                    return;
                };
                let site_info = &panels.sites[site];
                this.outcome = Some((
                    site,
                    match &read {
                        Ok(sync) => Outcome::Done(
                            format!(
                                "Painel {} lido: {}.",
                                site_info.of,
                                recorded_text(&sync.recorded)
                            )
                            .into(),
                        ),
                        Err(error) => Outcome::Failed(failure_text(site_info, error).into()),
                    },
                ));
                if read.is_ok_and(|sync| sync.changed_anything()) {
                    cx.emit(PanelRead);
                }
                this.refresh(window, cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn render_panel(&self, index: usize, panels: &Panels, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let shown = &self.shown[index];
        let site = &panels.sites[shown.site];
        // The hourly Sync may be reading this panel already.
        let syncing = cx
            .try_global::<CommissionSyncs>()
            .is_some_and(|syncs| syncs.running);
        let busy = self.busy == Some(shown.site) || syncing;
        let working = self.busy.is_some() || syncing;
        let can_open = shown.state != PanelState::NoBrowser;
        let can_read = can_open && shown.state != PanelState::NeverLoggedIn;
        let state = state_text(site, shown.state);
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(if shown.state == PanelState::Ready {
                t.frame
            } else {
                t.accent_edge
            })
            .bg(t.surface)
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_center()
                    .child(kit::section_heading(format!(
                        "Painel de afiliado {}",
                        site.of
                    )))
                    .child(kit::tag(
                        match shown.state {
                            PanelState::NoBrowser => "sem navegador",
                            PanelState::NeverLoggedIn => "falta entrar",
                            PanelState::LoggedOut => "entre de novo",
                            PanelState::Ready => "conectado",
                        },
                        if shown.state == PanelState::Ready {
                            t.success
                        } else {
                            t.danger
                        },
                        cx,
                    ))
                    .child(kit::tag("funcionalidade restrita", t.text2, cx)),
            )
            .child(if shown.state == PanelState::NoBrowser {
                kit::error_notice(state, cx).into_any_element()
            } else {
                div()
                    .text_sm()
                    .text_color(t.text2)
                    .child(state)
                    .into_any_element()
            })
            .when(can_open, |section| {
                section.child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new(("log-in-panel", shown.site))
                                .label("Entrar no painel")
                                .icon(IconName::ExternalLink)
                                .outline()
                                .small()
                                .disabled(working)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.log_in(index, window, cx)
                                })),
                        )
                        .when(can_read, |row| {
                            row.child(
                                Button::new(("read-panel", shown.site))
                                    .label("Ler painel agora")
                                    .icon(IconName::RefreshCw)
                                    .primary()
                                    .small()
                                    .loading(busy)
                                    .disabled(working)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.read(index, window, cx)
                                    })),
                            )
                        }),
                )
            })
            .children(
                self.outcome
                    .as_ref()
                    .filter(|(at, _)| *at == shown.site)
                    .map(|(_, outcome)| notice(outcome, cx)),
            )
            .into_any_element()
    }
}

impl Render for AffiliatePanelsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(panels) = panels(cx) else {
            return div().into_any_element();
        };
        let failed = self
            .outcome
            .as_ref()
            .filter(|_| self.shown.is_empty())
            .map(|(_, outcome)| notice(outcome, cx));
        let shown: Vec<AnyElement> = (0..self.shown.len())
            .map(|index| self.render_panel(index, &panels, cx))
            .collect();
        v_flex()
            .gap_3()
            .children(failed)
            .children(shown)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use mascate_kernel::SystemClock;
    use mascate_kernel::testing::SequentialIds;
    use mascate_platform::{Database, Registry, migrate};

    use super::*;

    fn nowhere() -> Environment {
        Box::new(|_: &str| None)
    }

    #[test]
    fn a_panel_is_built_only_while_its_flag_is_on_and_no_browser_opens_meanwhile() {
        block_on(async {
            let dir = tempfile::tempdir().unwrap();
            let database = Arc::new(Database::open(&dir.path().join("m.db")).await.unwrap());
            migrate(&database, &SystemClock, &[mascate_platform::MIGRATIONS])
                .await
                .unwrap();
            let flags = Flags::new(
                database,
                Registry::new(&[mascate_integrations::FLAGS]).unwrap(),
                Arc::new(SystemClock),
                Arc::new(SequentialIds::default()),
            );
            let profile = dir.path().join("navegador");
            let panels = Panels {
                browser: Arc::new(BrowserSession::new(None, profile.clone())),
                sites: PanelSite::all(Build::Release, &nowhere()),
            };
            let shown = |on: &[(&PanelSite, Arc<dyn AffiliatePanel>)]| {
                on.iter()
                    .map(|(site, panel)| (site.of, panel.state()))
                    .collect::<Vec<_>>()
            };

            let off = shown(&panels.on(&flags).await.unwrap());
            let request = flags.request_turn_on(MAGALU_AFFILIATE_PANEL.key).unwrap();
            flags.turn_on(request.confirm("fernando")).await.unwrap();
            let magalu_on = shown(&panels.on(&flags).await.unwrap());
            let built = |site: usize| panels.sites[site].built.get().is_some();
            let built_with_magalu_on = (built(0), built(1));
            let request = flags
                .request_turn_on(MERCADO_LIVRE_AFFILIATE_PANEL.key)
                .unwrap();
            flags.turn_on(request.confirm("fernando")).await.unwrap();
            let both_on = shown(&panels.on(&flags).await.unwrap());

            assert!(off.is_empty());
            assert_eq!(built_with_magalu_on, (false, true));
            assert_eq!(magalu_on, [("da Magalu", PanelState::NoBrowser)]);
            assert_eq!(
                both_on,
                [
                    ("do Mercado Livre", PanelState::NoBrowser),
                    ("da Magalu", PanelState::NoBrowser),
                ]
            );
            assert!(!profile.exists());
        });
    }

    #[test]
    fn each_panel_is_the_real_one_unless_a_development_build_points_elsewhere() {
        let release = PanelSite::mercado_livre(Build::Release, &nowhere());
        let development = PanelSite::mercado_livre(
            Build::Development,
            &(Box::new(|name: &str| {
                (name == "MASCATE_MERCADO_LIVRE_AFFILIATE_PANEL_URL")
                    .then(|| "http://127.0.0.1:8160/afiliados".to_owned())
            }) as Environment),
        );

        assert_eq!(release.url, "https://www.mercadolivre.com.br/afiliados");
        assert_eq!(
            release.flag.key,
            "integrations.mercado_livre_affiliate_panel"
        );
        assert_eq!(development.url, "http://127.0.0.1:8160/afiliados");
        let magalu = PanelSite::magalu(Build::Release, &nowhere());
        assert_eq!(magalu.url, "https://www.magazinevoce.com.br/painel");
        assert_eq!(magalu.flag.key, "integrations.magalu_affiliate_panel");
        assert_eq!(
            profile_folder(PathBuf::from("dados")),
            PathBuf::from("dados").join("navegador")
        );
    }

    #[test]
    fn each_failure_tells_the_owner_what_to_do() {
        let site = PanelSite::mercado_livre(Build::Release, &nowhere());
        let text = |error: PlatformError| failure_text(&site, &AffiliateError::Platform(error));

        assert!(text(PlatformError::Expired).contains("Entrar no painel"));
        assert!(text(PlatformError::NotConnected).contains("uma vez"));
        assert_eq!(
            text(PlatformError::Refused(
                "não há Edge nem Chrome instalado neste computador".into()
            )),
            "Não li o painel de afiliado do Mercado Livre: não há Edge nem Chrome instalado \
             neste computador."
        );
        assert!(state_text(&site, PanelState::NoBrowser).contains("Instale"));
        assert!(state_text(&site, PanelState::LoggedOut).contains("espera"));
    }
}
