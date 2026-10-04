//! Qualidade (#23): how Mercado Livre rates each open listing and what it
//! says is missing, the listings to work on first on top (something
//! pending, then units sold and visits in the last 30 days), each action
//! with the link where Mercado Livre lets the owner fix it. The quality is
//! read in each Sync of Anúncios and on this screen's own button; the rules
//! live in Marketing (ADR 0021).

use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Icon, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Global, Hsla, SharedString, Window, div, px};
use mascate_commerce::{ListingStatus, Listings, Orders};
use mascate_integrations::{Connection, ConnectionState};
use mascate_kernel::Timestamp;
use mascate_marketing::{
    ActionKind, IMPACT_DAYS, ListedItem, ListingQuality, QualityAction, QualityError, QualityLevel,
    QualityRow, QualitySource, QualitySync, Rating,
};

use crate::appearance::{Tokens, look};
use crate::catalog::{self, NO_DATABASE};
use crate::connections::AppConnections;
use crate::forms::{Outcome, notice};
use crate::kit;
use crate::layout;
use crate::listings::{self, listings};
use crate::mercado_livre;
use crate::orders::{self, orders};
use crate::parts::ScreenParts;

/// The app's Listing Quality; absent when the database did not open.
pub struct AppQuality(pub Arc<ListingQuality>);

impl Global for AppQuality {}

pub(crate) fn quality(cx: &App) -> Option<Arc<ListingQuality>> {
    cx.try_global::<AppQuality>().map(|app| app.0.clone())
}

/// The channel ids of the listings still open, which the panel rates.
async fn open_listings(listings: &Listings) -> Result<Vec<ListedItem>, String> {
    Ok(listings
        .listings()
        .await
        .map_err(|e| listings::failure(&e))?
        .into_iter()
        .filter(|listing| listing.listed.status != ListingStatus::Closed)
        .map(|listing| ListedItem {
            listing: listing.listed.id,
            title: listing.listed.title,
            link: listing.listed.link,
            units_sold: 0,
        })
        .collect())
}

/// Reads the quality and visits of every open listing again, as each Sync
/// of Anúncios does after reading the listings.
pub(crate) async fn sync(
    listings: &Listings,
    quality: &ListingQuality,
    source: &dyn QualitySource,
) -> Result<QualitySync, String> {
    let ids: Vec<String> = open_listings(listings)
        .await?
        .into_iter()
        .map(|listed| listed.listing)
        .collect();
    quality.sync(source, &ids).await.map_err(|e| failure(&e))
}

/// The panel: every open listing with its units sold, in the order to work
/// on them.
async fn panel(
    listings: &Listings,
    orders: &Orders,
    quality: &ListingQuality,
) -> Result<Vec<QualityRow>, String> {
    let sold = orders
        .units_sold_since(quality.impact_since())
        .await
        .map_err(|e| orders::failure(&e))?;
    let listed: Vec<ListedItem> = open_listings(listings)
        .await?
        .into_iter()
        .map(|listed| ListedItem {
            units_sold: sold.get(&listed.listing).copied().unwrap_or(0),
            ..listed
        })
        .collect();
    quality.panel(&listed).await.map_err(|e| failure(&e))
}

/// One Sync's line for the screen that ran it.
pub(crate) fn synced(report: &QualitySync) -> String {
    let rated = report.checked - report.not_rated;
    let mut text = match (rated, report.pending) {
        (0, _) => "Qualidade lida, mas nenhum anúncio tem nota ainda.".to_owned(),
        (_, 0) => "Qualidade lida: nada pendente nos anúncios.".to_owned(),
        (_, 1) => "Qualidade lida: 1 ação pendente.".to_owned(),
        (_, pending) => format!("Qualidade lida: {pending} ações pendentes."),
    };
    if rated > 0 && report.not_rated > 0 {
        text.push_str(&format!(
            " O Mercado Livre ainda não deu nota a {}.",
            listings_text(report.not_rated)
        ));
    }
    text
}

pub(crate) fn failure(error: &QualityError) -> String {
    match error {
        QualityError::Platform(error) => mercado_livre::failure(error),
        error => format!("Não consegui ler ou gravar a qualidade dos anúncios: {error}"),
    }
}

/// Everything the screen shows, read in one go off the UI thread.
struct Snapshot {
    rows: Vec<QualityRow>,
    last_sync: Option<Timestamp>,
    connection: ConnectionState,
}

pub struct QualityScreen {
    rows: Vec<QualityRow>,
    last_sync: Option<Timestamp>,
    connection: Option<ConnectionState>,
    syncing: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
}

impl QualityScreen {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut screen = Self {
            rows: Vec::new(),
            last_sync: None,
            connection: None,
            syncing: false,
            reads: 0,
            outcome: None,
        };
        screen.refresh(cx);
        screen
    }

    /// Reads the panel again, as when the screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.read(cx, None);
    }

    /// Reads the quality from Mercado Livre, then the panel.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(source) = mercado_livre::adapter(cx) else {
            return;
        };
        self.syncing = true;
        self.outcome = None;
        self.read(cx, Some(source));
        cx.notify();
    }

    /// Runs a Sync with `source`, if given, off the UI thread, then reads
    /// the panel again.
    fn read(&mut self, cx: &mut Context<Self>, source: Option<Arc<dyn QualitySource>>) {
        let (Some(listings), Some(orders), Some(quality)) = (listings(cx), orders(cx), quality(cx))
        else {
            return;
        };
        let connections = cx.global::<AppConnections>().0.clone();
        self.reads += 1;
        let read = self.reads;
        let working = cx.background_executor().spawn(async move {
            let rated = match source {
                Some(source) => Some(sync(&listings, &quality, source.as_ref()).await),
                None => None,
            };
            let snapshot = async {
                Ok::<_, String>(Snapshot {
                    rows: panel(&listings, &orders, &quality).await?,
                    last_sync: quality.last_sync().await.map_err(|e| failure(&e))?,
                    connection: connections.state(Connection::MercadoLivre),
                })
            }
            .await;
            (rated, snapshot)
        });
        cx.spawn(async move |this, cx| {
            let (rated, snapshot) = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads == read {
                    this.syncing = false;
                    match snapshot {
                        Ok(snapshot) => {
                            this.rows = snapshot.rows;
                            this.last_sync = snapshot.last_sync;
                            this.connection = Some(snapshot.connection);
                        }
                        Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                    }
                }
                match rated {
                    Some(Ok(report)) => this.outcome = Some(Outcome::Done(synced(&report).into())),
                    Some(Err(error)) => this.outcome = Some(Outcome::Failed(error.into())),
                    None => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn connected(&self) -> bool {
        self.connection == Some(ConnectionState::Connected)
    }

    fn render_sync(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let when = match self.last_sync {
            Some(at) => format!("Lida em {}", catalog::day_and_time(at)),
            None => "Qualidade ainda não lida.".into(),
        };
        h_flex()
            .gap_2()
            .items_center()
            .child(div().text_xs().text_color(t.text2).child(when))
            .child(
                Button::new("sync-quality")
                    .label("Atualizar")
                    .icon(IconName::RefreshCw)
                    .primary()
                    .small()
                    .loading(self.syncing)
                    .disabled(self.syncing || !self.connected())
                    .on_click(cx.listener(|this, _, _, cx| this.sync(cx))),
            )
            .into_any_element()
    }

    fn render_row(&self, index: usize, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let row = &self.rows[index];
        let listed = &row.listing;
        let mut facts = vec![
            listed.listing.clone(),
            sold_text(listed.units_sold),
            row.visits
                .map_or_else(|| "visitas ainda não lidas".into(), visits_text),
        ];
        if let Some(at) = row.checked_at {
            facts.push(format!("lida em {}", catalog::day(at)));
        }
        let (rating, ink) = rating_tag(&row.rating, &t);
        let body = match &row.rating {
            Rating::NotChecked => muted(
                "Ainda sem leitura de qualidade: clique em Atualizar ou sincronize em Anúncios.",
                &t,
            ),
            Rating::NotRated => muted(
                "O Mercado Livre ainda não calculou a qualidade deste anúncio.",
                &t,
            ),
            Rating::Rated(quality) if quality.pending.is_empty() => {
                muted("Nada pendente: o Mercado Livre não pede mais nada.", &t)
            }
            Rating::Rated(quality) => v_flex()
                .gap_2()
                .children(
                    quality
                        .pending
                        .iter()
                        .enumerate()
                        .map(|(at, action)| render_action(index, at, action, cx)),
                )
                .into_any_element(),
        };
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        v_flex()
                            .min_w_0()
                            .flex_1()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .font_medium()
                                            .truncate()
                                            .child(listed.title.clone()),
                                    )
                                    .child(kit::tag(rating, ink, cx)),
                            )
                            .child(
                                h_flex()
                                    .flex_wrap()
                                    .gap_x_2()
                                    .text_xs()
                                    .text_color(t.text2)
                                    .child(facts.join(" · "))
                                    .children(listed.link.clone().map(|link| {
                                        external_link(
                                            format!("open-{index}"),
                                            "Abrir no Mercado Livre",
                                            link,
                                            &t,
                                        )
                                    })),
                            ),
                    )
                    .children(score(&row.rating).map(|score| {
                        div()
                            .flex_none()
                            .text_xl()
                            .font_semibold()
                            .text_color(ink)
                            .child(format!("{score}/100"))
                    })),
            )
            .child(body)
            .into_any_element()
    }
}

impl Render for QualityScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let mut parts = ScreenParts::new("Qualidade dos anúncios");
        if quality(cx).is_none() || listings(cx).is_none() || orders(cx).is_none() {
            parts
                .notices
                .push(kit::error_notice(NO_DATABASE, cx).into_any_element());
            return layout::screen(parts, cx);
        }
        parts.actions.push(self.render_sync(cx));
        if let Some(state) = &self.connection
            && *state != ConnectionState::Connected
        {
            parts
                .notices
                .push(kit::info_notice(mercado_livre::not_connected(state), cx).into_any_element());
        }
        parts
            .notices
            .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        let pending = self
            .rows
            .iter()
            .filter(|row| !row.pending().is_empty())
            .count();
        let heading = match (self.rows.len(), pending) {
            (0, _) => "Anúncios".to_owned(),
            (count, 0) => format!("{}, nada pendente", listings_text(count)),
            (count, pending) => format!("{}, {} com o que melhorar", listings_text(count), pending),
        };
        parts.content.push(
            v_flex()
                .gap_3()
                .child(v_flex().gap_1().child(kit::section_heading(heading)).child(
                    div().text_xs().text_color(t.text2).child(format!(
                        "Primeiro os anúncios com algo a melhorar, dos que mais venderam e \
                             mais receberam visitas nos últimos {IMPACT_DAYS} dias. As correções \
                             são feitas no Mercado Livre, pelo link de cada ação."
                    )),
                ))
                .when(self.rows.is_empty(), |list| {
                    list.child(muted(
                        "Nenhum anúncio aberto. Sincronize em Anúncios para trazer os seus do \
                         Mercado Livre; a qualidade de cada um é lida no mesmo Sync.",
                        &t,
                    ))
                })
                .children((0..self.rows.len()).map(|index| self.render_row(index, cx)))
                .into_any_element(),
        );
        layout::screen(parts, cx)
    }
}

/// One pending action: what it weighs, the channel's words and the link to
/// fix it.
fn render_action(row: usize, at: usize, action: &QualityAction, cx: &App) -> AnyElement {
    let t = look(cx).tokens;
    let (kind, ink) = match action.kind {
        ActionKind::Problem => ("Derruba a nota", t.danger),
        ActionKind::Opportunity => ("Sugestão", t.accent_text),
    };
    h_flex()
        .gap_2()
        .items_start()
        .pl_3()
        .border_l_2()
        .border_color(t.border)
        .child(kit::tag(kind, ink, cx))
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap_0p5()
                .text_sm()
                .child(action.text.clone())
                .children(action.link.clone().map(|link| {
                    external_link(format!("fix-{row}-{at}"), action.label.clone(), link, &t)
                })),
        )
        .into_any_element()
}

fn external_link(
    id: String,
    label: impl Into<SharedString>,
    link: String,
    t: &Tokens,
) -> gpui_kit::Stateful<gpui_kit::Div> {
    div()
        .id(SharedString::from(id))
        .flex()
        .gap_1()
        .items_center()
        .text_xs()
        .text_color(t.accent_text)
        .cursor_pointer()
        .child(label.into())
        .child(Icon::new(IconName::ExternalLink).size(px(12.)))
        .on_click(move |_, _, cx| cx.open_url(&link))
}

fn muted(text: impl Into<SharedString>, t: &Tokens) -> AnyElement {
    div()
        .text_sm()
        .text_color(t.text2)
        .child(text.into())
        .into_any_element()
}

fn rating_tag(rating: &Rating, t: &Tokens) -> (&'static str, Hsla) {
    match rating {
        Rating::NotChecked => ("Sem leitura", t.text2),
        Rating::NotRated => ("Sem nota", t.text2),
        Rating::Rated(quality) => (
            quality.level.name(),
            match quality.level {
                QualityLevel::Basic => t.danger,
                QualityLevel::Standard => t.accent_text,
                QualityLevel::Professional => t.success,
            },
        ),
    }
}

fn score(rating: &Rating) -> Option<u8> {
    match rating {
        Rating::Rated(quality) => Some(quality.score),
        _ => None,
    }
}

fn sold_text(units: u32) -> String {
    match units {
        0 => format!("nenhuma venda em {IMPACT_DAYS} dias"),
        1 => format!("1 vendida em {IMPACT_DAYS} dias"),
        units => format!("{units} vendidas em {IMPACT_DAYS} dias"),
    }
}

fn visits_text(visits: u32) -> String {
    match visits {
        1 => "1 visita".into(),
        visits => format!("{visits} visitas"),
    }
}

fn listings_text(count: usize) -> String {
    match count {
        1 => "1 anúncio".into(),
        count => format!("{count} anúncios"),
    }
}
