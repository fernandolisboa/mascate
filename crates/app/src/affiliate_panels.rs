//! The logged-in affiliate panels as Afiliação uses them (#160, ADR 0035
//! decision 4): the one browser they share, each panel built only while
//! its Restricted Feature is on, and its Connection on the screen, which
//! lives in the browser's own profile: whether a browser is installed,
//! "Entrar no painel" to log in by hand in its window, and "Ler painel
//! agora". The hourly Commissions Sync reads every panel that is on and
//! logged in. The logged-in affiliate markets of Hotmart (#221), Kiwify
//! (#222), Eduzz (#223) and Monetizze (#224) share the browser and its
//! profile, but only Garimpo reads them, on the owner's click.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, EventEmitter, Global, Window, div};
use mascate_affiliate::{AffiliateError, AffiliatePlatform};
use mascate_integrations::{
    AffiliateMarket, AffiliatePanel, BrowserSession, EDUZZ_AFFILIATE_MARKET,
    EDUZZ_AFFILIATE_MARKET_URL, EduzzAffiliateMarket, HOTMART_AFFILIATE_MARKET,
    HOTMART_AFFILIATE_MARKET_URL, HotmartAffiliateMarket, KIWIFY_AFFILIATE_MARKET,
    KIWIFY_AFFILIATE_MARKET_URL, KiwifyAffiliateMarket, MAGALU_AFFILIATE_PANEL,
    MAGALU_AFFILIATE_PANEL_URL, MERCADO_LIVRE_AFFILIATE_PANEL, MERCADO_LIVRE_AFFILIATE_PANEL_URL,
    MONETIZZE_AFFILIATE_MARKET, MONETIZZE_AFFILIATE_MARKET_URL, MagaluAffiliatePanel,
    MercadoLivreAffiliatePanel, MonetizzeAffiliateMarket, PanelState, browser_name,
};
use mascate_kernel::{PlatformError, SystemClock, ThreadPause};
use mascate_platform::{Build, Environment, Flag, FlagError, Flags};

use crate::affiliate::{CommissionSyncs, affiliate, failure, recorded_text};
use crate::appearance::look;
use crate::forms::{Outcome, notice};
use crate::kit;
use crate::preferences;
use crate::restricted::AppFlags;

/// The browser every panel shares, so one profile holds every login and
/// one browser runs at a time, and the panels it can read.
pub struct Panels {
    pub browser: Arc<BrowserSession>,
    pub sites: Vec<PanelSite>,
    pub markets: Vec<MarketSite>,
}

pub struct AppAffiliatePanels(pub Arc<Panels>);

impl Global for AppAffiliatePanels {}

pub fn panels(cx: &App) -> Option<Arc<Panels>> {
    cx.try_global::<AppAffiliatePanels>()
        .map(|app| app.0.clone())
}

/// Where `browser` keeps the Mascate's own profile, with the logins:
/// beside the database, never the owner's daily profile. Each browser has
/// its own, since one cannot open a profile another wrote; switching the
/// default browser asks for one login in the new one.
pub fn profile_folder(data: PathBuf, browser: Option<&Path>) -> PathBuf {
    match browser.and_then(browser_name) {
        Some(name) => data.join(format!("navegador-{}", name.to_lowercase())),
        None => data.join("navegador"),
    }
}

/// Where the reading of `platform`'s affiliate market is stopped, so a
/// stop outlasts a restart: beside the browser's profile, never inside it.
pub fn market_stop_file(data: PathBuf, platform: AffiliatePlatform) -> PathBuf {
    data.join(format!(
        "mercado-{}-parado",
        platform.name().to_lowercase().replace(' ', "-")
    ))
}

/// How Garimpo names a Platform's logged-in market and what it reads.
#[derive(Debug, Clone, Copy)]
pub struct MarketTexts {
    pub platform: AffiliatePlatform,
    pub flag: Flag,
    /// "o mercado de afiliação", "a Vitrine", with its article.
    pub called: &'static str,
    /// What one click reads: "até 5 páginas por clique".
    pub pages: &'static str,
    /// What a read brings: "comissão, preço e …".
    pub reads: &'static str,
    /// Where a read without a category looked: "em todas as categorias".
    pub everything: &'static str,
}

impl MarketTexts {
    /// "no mercado de afiliação", "na Vitrine": where a read looks.
    pub fn called_in(&self) -> String {
        match self.called.split_once(' ') {
            Some(("o", rest)) => format!("no {rest}"),
            Some(("a", rest)) => format!("na {rest}"),
            _ => format!("em {}", self.called),
        }
    }
}

/// A Platform's logged-in affiliate market, read for Garimpo.
pub struct MarketSite {
    pub texts: MarketTexts,
    pub url: String,
    stop_file: Option<PathBuf>,
    build: fn(&str, Arc<BrowserSession>) -> NewMarket,
    /// Built the first time its flag is found on, and kept, so its memory
    /// of pages, its interval and its stop last while the app runs.
    built: OnceLock<Arc<dyn AffiliateMarket>>,
}

/// A market reader, before its stop file is set.
enum NewMarket {
    Hotmart(HotmartAffiliateMarket),
    Kiwify(KiwifyAffiliateMarket),
    Monetizze(MonetizzeAffiliateMarket),
    Eduzz(EduzzAffiliateMarket),
}

impl NewMarket {
    fn kept_in(self, stop_file: Option<PathBuf>) -> Arc<dyn AffiliateMarket> {
        match (self, stop_file) {
            (NewMarket::Hotmart(market), Some(file)) => Arc::new(market.with_stop_file(file)),
            (NewMarket::Hotmart(market), None) => Arc::new(market),
            (NewMarket::Kiwify(market), Some(file)) => Arc::new(market.with_stop_file(file)),
            (NewMarket::Kiwify(market), None) => Arc::new(market),
            (NewMarket::Monetizze(market), Some(file)) => Arc::new(market.with_stop_file(file)),
            (NewMarket::Monetizze(market), None) => Arc::new(market),
            (NewMarket::Eduzz(market), Some(file)) => Arc::new(market.with_stop_file(file)),
            (NewMarket::Eduzz(market), None) => Arc::new(market),
        }
    }
}

impl MarketSite {
    /// Every market the app reads, in the Work Context's order; each stop
    /// is kept beside `data`, when there is one.
    pub fn all(build: Build, environment: &Environment, data: Option<PathBuf>) -> Vec<Self> {
        vec![
            Self::hotmart(build, environment, data.clone()),
            Self::kiwify(build, environment, data.clone()),
            Self::monetizze(build, environment, data.clone()),
            Self::eduzz(build, environment, data),
        ]
    }

    /// Hotmart's Mercado de Afiliação or, in a development build only, the
    /// URL in `MASCATE_HOTMART_AFFILIATE_MARKET_URL`, a local fake page.
    pub fn hotmart(build: Build, environment: &Environment, data: Option<PathBuf>) -> Self {
        Self {
            texts: MarketTexts {
                platform: AffiliatePlatform::Hotmart,
                flag: HOTMART_AFFILIATE_MARKET,
                called: "o mercado de afiliação",
                pages: "até 5 páginas por clique",
                reads: "temperatura, comissão em % e em R$, Blueprint, satisfação, preço e se a \
                        afiliação é imediata",
                everything: "em todas as categorias",
            },
            url: crate::mercado_livre::local_fake(
                build,
                environment,
                "MASCATE_HOTMART_AFFILIATE_MARKET_URL",
            )
            .unwrap_or_else(|| HOTMART_AFFILIATE_MARKET_URL.to_owned()),
            stop_file: data.map(|data| market_stop_file(data, AffiliatePlatform::Hotmart)),
            build: |url, browser| {
                NewMarket::Hotmart(HotmartAffiliateMarket::new(
                    url,
                    browser,
                    Arc::new(SystemClock),
                    Arc::new(ThreadPause),
                ))
            },
            built: OnceLock::new(),
        }
    }

    /// Kiwify's Marketplace or, in a development build only, the URL in
    /// `MASCATE_KIWIFY_AFFILIATE_MARKET_URL`, a local fake page.
    pub fn kiwify(build: Build, environment: &Environment, data: Option<PathBuf>) -> Self {
        Self {
            texts: MarketTexts {
                platform: AffiliatePlatform::Kiwify,
                flag: KIWIFY_AFFILIATE_MARKET,
                called: "o marketplace",
                pages: "a primeira página da lista por clique",
                reads: "comissão em % e em R$, preço, garantia, o produtor e se a aprovação é \
                        automática",
                everything: "na primeira página",
            },
            url: crate::mercado_livre::local_fake(
                build,
                environment,
                "MASCATE_KIWIFY_AFFILIATE_MARKET_URL",
            )
            .unwrap_or_else(|| KIWIFY_AFFILIATE_MARKET_URL.to_owned()),
            stop_file: data.map(|data| market_stop_file(data, AffiliatePlatform::Kiwify)),
            build: |url, browser| {
                NewMarket::Kiwify(KiwifyAffiliateMarket::new(
                    url,
                    browser,
                    Arc::new(SystemClock),
                    Arc::new(ThreadPause),
                ))
            },
            built: OnceLock::new(),
        }
    }

    /// Monetizze's Vitrine de afiliação or, in a development build only,
    /// the URL in `MASCATE_MONETIZZE_AFFILIATE_MARKET_URL`, a local fake
    /// page.
    pub fn monetizze(build: Build, environment: &Environment, data: Option<PathBuf>) -> Self {
        Self {
            texts: MarketTexts {
                platform: AffiliatePlatform::Monetizze,
                flag: MONETIZZE_AFFILIATE_MARKET,
                called: "a Vitrine de afiliação",
                pages: "a primeira página da Vitrine por clique",
                reads: "comissão em % e em R$, preço, BARs (a pontuação do Hall da Fama), se é \
                        produto físico ou digital, a categoria, o produtor e se a afiliação é \
                        aberta",
                everything: "na primeira página",
            },
            url: crate::mercado_livre::local_fake(
                build,
                environment,
                "MASCATE_MONETIZZE_AFFILIATE_MARKET_URL",
            )
            .unwrap_or_else(|| MONETIZZE_AFFILIATE_MARKET_URL.to_owned()),
            stop_file: data.map(|data| market_stop_file(data, AffiliatePlatform::Monetizze)),
            build: |url, browser| {
                NewMarket::Monetizze(MonetizzeAffiliateMarket::new(
                    url,
                    browser,
                    Arc::new(SystemClock),
                    Arc::new(ThreadPause),
                ))
            },
            built: OnceLock::new(),
        }
    }

    /// Eduzz's Vitrine or, in a development build only, the URL in
    /// `MASCATE_EDUZZ_AFFILIATE_MARKET_URL`, a local fake page.
    pub fn eduzz(build: Build, environment: &Environment, data: Option<PathBuf>) -> Self {
        Self {
            texts: MarketTexts {
                platform: AffiliatePlatform::Eduzz,
                flag: EDUZZ_AFFILIATE_MARKET,
                called: "a Vitrine",
                pages: "a primeira página da Vitrine por clique",
                reads: "comissão por venda em % e em R$, preço, nota, garantia, o produtor e se a \
                        afiliação é automática",
                everything: "na primeira página",
            },
            url: crate::mercado_livre::local_fake(
                build,
                environment,
                "MASCATE_EDUZZ_AFFILIATE_MARKET_URL",
            )
            .unwrap_or_else(|| EDUZZ_AFFILIATE_MARKET_URL.to_owned()),
            stop_file: data.map(|data| market_stop_file(data, AffiliatePlatform::Eduzz)),
            build: |url, browser| {
                NewMarket::Eduzz(EduzzAffiliateMarket::new(
                    url,
                    browser,
                    Arc::new(SystemClock),
                    Arc::new(ThreadPause),
                ))
            },
            built: OnceLock::new(),
        }
    }
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

impl Panels {
    /// The logged-in affiliate markets whose flag is on, each with its
    /// reader; off, a market is never built and no browser opens for it.
    pub async fn markets_on(
        &self,
        flags: &Flags,
    ) -> Result<Vec<(&MarketSite, Arc<dyn AffiliateMarket>)>, FlagError> {
        let mut on = Vec::new();
        for site in &self.markets {
            if flags.is_on(&site.texts.flag).await? {
                let market = site
                    .built
                    .get_or_init(|| {
                        (site.build)(&site.url, self.browser.clone())
                            .kept_in(site.stop_file.clone())
                    })
                    .clone();
                on.push((site, market));
            }
        }
        Ok(on)
    }

    /// The market of `platform`, whether its flag is on or not.
    pub fn market_site(&self, platform: AffiliatePlatform) -> Option<&MarketSite> {
        self.markets
            .iter()
            .find(|site| site.texts.platform == platform)
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
fn state_text(site: &PanelSite, state: PanelState, browser: &str) -> String {
    let of = site.of;
    match state {
        PanelState::NoBrowser => format!(
            "Não há Chrome, Edge, Brave nem Vivaldi instalado neste computador, e o app lê o \
             painel {of} por um deles. Instale um e abra o app de novo."
        ),
        PanelState::NeverLoggedIn => format!(
            "Falta entrar uma vez: clique em Entrar no painel, entre com a sua conta {of} na \
             janela do {browser} que abrir e feche a janela quando o painel aparecer. A senha fica só no \
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
        let state = state_text(
            site,
            shown.state,
            panels.browser.name().unwrap_or("navegador"),
        );
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
        let context = preferences::work_context(cx);
        let shown: Vec<AnyElement> = (0..self.shown.len())
            .filter(|&index| context.shows_platform(self.shown[index].panel.platform()))
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
    use crate::affiliate_markets::MARKET_PLATFORMS;

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
                markets: MarketSite::all(Build::Release, &nowhere(), None),
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
    fn each_market_is_built_only_while_its_flag_is_on_and_opens_nothing_meanwhile() {
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
            let profile = profile_folder(dir.path().to_path_buf(), None);
            let panels = Panels {
                browser: Arc::new(BrowserSession::new(None, profile.clone())),
                sites: PanelSite::all(Build::Release, &nowhere()),
                markets: MarketSite::all(
                    Build::Release,
                    &nowhere(),
                    Some(dir.path().to_path_buf()),
                ),
            };
            let platforms = |on: &[(&MarketSite, Arc<dyn AffiliateMarket>)]| {
                on.iter()
                    .map(|(site, market)| (site.texts.platform, market.platform(), market.state()))
                    .collect::<Vec<_>>()
            };
            let built = |platform| {
                panels
                    .market_site(platform)
                    .is_some_and(|site| site.built.get().is_some())
            };

            let off = platforms(&panels.markets_on(&flags).await.unwrap());
            let built_while_off = [
                AffiliatePlatform::Hotmart,
                AffiliatePlatform::Kiwify,
                AffiliatePlatform::Monetizze,
                AffiliatePlatform::Eduzz,
            ]
            .into_iter()
            .any(built);
            let request = flags.request_turn_on(KIWIFY_AFFILIATE_MARKET.key).unwrap();
            assert_eq!(request.flag().risk, KIWIFY_AFFILIATE_MARKET.risk);
            flags.turn_on(request.confirm("fernando")).await.unwrap();
            let kiwify_on = platforms(&panels.markets_on(&flags).await.unwrap());
            let others_built = built(AffiliatePlatform::Hotmart)
                || built(AffiliatePlatform::Monetizze)
                || built(AffiliatePlatform::Eduzz);
            let request = flags.request_turn_on(HOTMART_AFFILIATE_MARKET.key).unwrap();
            flags.turn_on(request.confirm("fernando")).await.unwrap();
            let both = panels.markets_on(&flags).await.unwrap();
            let again = panels.markets_on(&flags).await.unwrap();
            let request = flags.request_turn_on(EDUZZ_AFFILIATE_MARKET.key).unwrap();
            assert_eq!(request.flag().risk, EDUZZ_AFFILIATE_MARKET.risk);
            flags.turn_on(request.confirm("fernando")).await.unwrap();
            let request = flags
                .request_turn_on(MONETIZZE_AFFILIATE_MARKET.key)
                .unwrap();
            assert_eq!(request.flag().risk, MONETIZZE_AFFILIATE_MARKET.risk);
            flags.turn_on(request.confirm("fernando")).await.unwrap();
            let all_on = platforms(&panels.markets_on(&flags).await.unwrap());

            assert!(off.is_empty());
            assert!(!built_while_off);
            assert_eq!(
                kiwify_on,
                [(
                    AffiliatePlatform::Kiwify,
                    AffiliatePlatform::Kiwify,
                    PanelState::NoBrowser
                )]
            );
            assert!(!others_built);
            assert_eq!(
                platforms(&both),
                [
                    (
                        AffiliatePlatform::Hotmart,
                        AffiliatePlatform::Hotmart,
                        PanelState::NoBrowser
                    ),
                    (
                        AffiliatePlatform::Kiwify,
                        AffiliatePlatform::Kiwify,
                        PanelState::NoBrowser
                    ),
                ]
            );
            assert!(Arc::ptr_eq(&both[1].1, &again[1].1));
            assert_eq!(
                all_on
                    .iter()
                    .map(|(site, market, _)| (*site, *market))
                    .collect::<Vec<_>>(),
                MARKET_PLATFORMS.map(|platform| (platform, platform))
            );
            assert!(!profile.exists());
        });
    }

    #[test]
    fn each_market_is_the_real_one_unless_a_development_build_points_elsewhere() {
        let release = MarketSite::all(Build::Release, &nowhere(), None);
        let development = MarketSite::kiwify(
            Build::Development,
            &(Box::new(|name: &str| {
                (name == "MASCATE_KIWIFY_AFFILIATE_MARKET_URL")
                    .then(|| "http://127.0.0.1:8222/marketplace".to_owned())
            }) as Environment),
            None,
        );

        let urls: Vec<&str> = release.iter().map(|site| site.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://app.hotmart.com/market",
                "https://dashboard.kiwify.com.br/marketplace",
                "https://app.monetizze.com.br/loja",
                "https://my2.eduzz.com/showcase"
            ]
        );
        assert_eq!(development.url, "http://127.0.0.1:8222/marketplace");
        assert_eq!(
            market_stop_file(PathBuf::from("dados"), AffiliatePlatform::Hotmart),
            PathBuf::from("dados").join("mercado-hotmart-parado")
        );
        assert_eq!(
            market_stop_file(PathBuf::from("dados"), AffiliatePlatform::Kiwify),
            PathBuf::from("dados").join("mercado-kiwify-parado")
        );
        assert_eq!(
            market_stop_file(PathBuf::from("dados"), AffiliatePlatform::Eduzz),
            PathBuf::from("dados").join("mercado-eduzz-parado")
        );
        assert_eq!(
            market_stop_file(PathBuf::from("dados"), AffiliatePlatform::Monetizze),
            PathBuf::from("dados").join("mercado-monetizze-parado")
        );
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
        let data = PathBuf::from("dados");
        assert_eq!(profile_folder(data.clone(), None), data.join("navegador"));
        assert_eq!(
            profile_folder(
                data.clone(),
                Some(Path::new("/usr/bin/google-chrome-stable"))
            ),
            data.join("navegador-chrome")
        );
        assert_eq!(
            profile_folder(data.clone(), Some(Path::new("/opt/brave.com/brave/brave"))),
            data.join("navegador-brave")
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
                "não há Chrome, Edge, Brave nem Vivaldi instalado neste computador".into()
            )),
            "Não li o painel de afiliado do Mercado Livre: não há Chrome, Edge, Brave nem \
             Vivaldi instalado neste computador."
        );
        assert!(state_text(&site, PanelState::NoBrowser, "Chrome").contains("Instale"));
        assert!(state_text(&site, PanelState::NeverLoggedIn, "Brave").contains("janela do Brave"));
        assert!(state_text(&site, PanelState::LoggedOut, "Chrome").contains("espera"));
    }
}
