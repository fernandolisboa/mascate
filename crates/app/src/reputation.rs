//! Reputação (#25): the owner's seller Reputation on Mercado Livre, its
//! color and how far each metric stands from the limit of the green, the
//! tools it unlocks, and the Reviews of each Product with the low ones to
//! look at. Read when the app opens and then hourly, after an Order Sync,
//! window open or in the tray, and on this screen's own button. A new low
//! Review and a tool unlocked come with a system notification. The rules
//! live in Marketing (ADR 0023); nothing is sent to Mercado Livre.

use std::collections::BTreeMap;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, Global, Hsla, SharedString, Subscription, SystemNotification, Task, Window,
    div,
};
use mascate_catalog::Catalog;
use mascate_commerce::Listings;
use mascate_integrations::{Connection, ConnectionState};
use mascate_kernel::{PlatformError, RecordId};
use mascate_marketing::{
    ArrivedReview, LOW_RATING, LowReview, MetricStanding, ProductReviews, Reputation,
    ReputationColor, ReputationError, ReputationStanding, ReputationSync, ReviewedItem, SellerTool,
};
use rust_decimal::Decimal;

use crate::appearance::{Tokens, look};
use crate::catalog::{self, NO_DATABASE};
use crate::connections::AppConnections;
use crate::forms::{Outcome, notice};
use crate::kit;
use crate::layout;
use crate::listings::{self, listings};
use crate::mercado_livre;
use crate::parts::ScreenParts;

/// The app's Reputation; absent when the database did not open.
pub struct AppReputation(pub Arc<Reputation>);

impl Global for AppReputation {}

pub(crate) fn reputation(cx: &App) -> Option<Arc<Reputation>> {
    cx.try_global::<AppReputation>().map(|app| app.0.clone())
}

/// The Syncs of the Reputation since the app opened, and the notices
/// dismissed; the screens that show them read them again after each.
#[derive(Default)]
pub struct ReputationSyncs {
    pub finished: u64,
}

impl Global for ReputationSyncs {}

/// What one Sync did, with the names its notifications use.
pub struct Round {
    synced: ReputationSync,
    names: BTreeMap<String, String>,
}

pub(crate) fn failure(error: &ReputationError) -> String {
    match error {
        ReputationError::Platform(error) => mercado_livre::failure(error),
        ReputationError::NotFound => "Não encontrei essa avaliação; atualize a tela.".into(),
        error => format!("Não consegui ler ou gravar a reputação no banco: {error}"),
    }
}

/// What each listing sells, as Reputation reads it: the open listings,
/// each with its Product (the Product's id groups its listings, and a
/// listing linked to none stands alone), and every listing's name.
async fn reviewed_items(
    listings: &Listings,
    catalog: &Catalog,
) -> Result<(Vec<ReviewedItem>, BTreeMap<String, String>), String> {
    let mut items = Vec::new();
    let mut names = BTreeMap::new();
    for (listing, sold) in listings::listing_products(listings, catalog).await? {
        if sold.open {
            items.push(ReviewedItem {
                product: sold
                    .product
                    .map_or_else(|| format!("listing:{listing}"), |id| id.to_string()),
                listing: listing.clone(),
                name: sold.name.clone(),
            });
        }
        names.insert(listing, sold.name);
    }
    Ok((items, names))
}

/// Runs a Sync of the Reputation and the Reviews off the UI thread: when
/// `forced`, or when the last one is an hour old. `Ok(None)` when the
/// Connection is not up or nothing was due.
pub fn sync_now(cx: &mut App, forced: bool) -> Option<Task<Result<Option<Round>, String>>> {
    let (Some(reputation), Some(channel), Some(listings), Some(catalog)) = (
        reputation(cx),
        mercado_livre::adapter(cx),
        listings(cx),
        catalog::catalog(cx),
    ) else {
        return None;
    };
    let connections = cx.global::<AppConnections>().0.clone();
    Some(cx.background_executor().spawn(async move {
        if connections.state(Connection::MercadoLivre) != ConnectionState::Connected {
            return Ok(None);
        }
        if !forced && !reputation.is_due().await.map_err(|e| failure(&e))? {
            return Ok(None);
        }
        let (items, names) = reviewed_items(&listings, &catalog).await?;
        let open: Vec<String> = items.into_iter().map(|item| item.listing).collect();
        let synced = reputation
            .sync(channel.as_ref(), &open)
            .await
            .map_err(|e| failure(&e))?;
        Ok(Some(Round { synced, names }))
    }))
}

/// Tells the owner what a Sync unlocked and the low Reviews it brought,
/// and the screens that it ran.
pub fn finished(outcome: &Result<Option<Round>, String>, cx: &mut App) {
    match outcome {
        Ok(Some(round)) => {
            for tool in &round.synced.unlocked {
                notify_unlocked(*tool, cx);
            }
            notify_arrived(&round.synced.arrived, &round.names, cx);
        }
        Ok(None) => {}
        Err(error) => eprintln!("could not sync the reputation: {error}"),
    }
    cx.default_global::<ReputationSyncs>().finished += 1;
}

fn notify_unlocked(tool: SellerTool, cx: &App) {
    cx.show_system_notification(SystemNotification {
        tag: format!("unlocked-{}", tool.name()).into(),
        title: unlocked_title(tool).into(),
        body: format!(
            "Sua reputação chegou ao que o Mercado Livre pede: {}.",
            tool.requirement()
        )
        .into(),
        actions: Vec::new(),
    });
}

/// Above this many new low Reviews in one Sync, one notification sums them
/// up instead of one each.
const NOTIFIED_ONE_BY_ONE: usize = 3;

fn notify_arrived(arrived: &[ArrivedReview], names: &BTreeMap<String, String>, cx: &App) {
    let name = |arrived: &ArrivedReview| {
        names
            .get(&arrived.listing)
            .cloned()
            .unwrap_or_else(|| arrived.listing.clone())
    };
    if arrived.len() > NOTIFIED_ONE_BY_ONE {
        let mut listed: Vec<String> = arrived.iter().map(name).collect();
        listed.sort();
        listed.dedup();
        cx.show_system_notification(SystemNotification {
            tag: "low-reviews".into(),
            title: format!("{} avaliações baixas no Mercado Livre", arrived.len()).into(),
            body: format!("Em {}.", listed.join(", ")).into(),
            actions: Vec::new(),
        });
        return;
    }
    for review in arrived {
        cx.show_system_notification(SystemNotification {
            tag: format!("review-{}", review.review.id).into(),
            title: format!(
                "Avaliação de {} no Mercado Livre",
                stars_text(review.review.rating)
            )
            .into(),
            body: format!(
                "{}: “{}”",
                name(review),
                review_line(&review.review.title, &review.review.text)
            )
            .into(),
            actions: Vec::new(),
        });
    }
}

pub(crate) fn unlocked_title(tool: SellerTool) -> String {
    match tool {
        SellerTool::Promotions => "Promoções liberadas no Mercado Livre".into(),
        SellerTool::ProductAds => "Product Ads liberado no Mercado Livre".into(),
    }
}

/// "1 estrela", "3 estrelas".
pub(crate) fn stars_text(stars: u8) -> String {
    match stars {
        1 => "1 estrela".into(),
        stars => format!("{stars} estrelas"),
    }
}

/// A Review's title, or its text when it has none.
pub(crate) fn review_line(title: &str, text: &str) -> String {
    let title = title.trim();
    if title.is_empty() {
        text.trim().to_owned()
    } else {
        title.to_owned()
    }
}

pub(crate) fn color_ink(color: ReputationColor, t: &Tokens) -> Hsla {
    match color {
        ReputationColor::Green | ReputationColor::LightGreen => t.success,
        ReputationColor::Yellow => t.accent_text,
        ReputationColor::Orange | ReputationColor::Red => t.danger,
    }
}

/// How far a metric is from the limit of the green, in sales.
fn room_text(standing: &MetricStanding) -> String {
    match standing.room {
        None => "sem vendas no período para contar".into(),
        Some(0) => "no limite da verde: mais 1 passa dele".into(),
        Some(1) => "cabe mais 1 antes de passar o limite da verde".into(),
        Some(room) if room > 0 => {
            format!("cabem mais {room} antes de passar o limite da verde")
        }
        Some(-1) => "1 acima do que a verde permite".into(),
        Some(room) => format!("{} acima do que a verde permite", -room),
    }
}

/// "0,8 ponto abaixo", "1,12 ponto acima".
fn points_text(points: Decimal) -> String {
    let distance = points.abs().normalize().to_string().replace('.', ",");
    let unit = if points.abs() == Decimal::ONE {
        "ponto"
    } else {
        "pontos"
    };
    if points.is_sign_negative() && !points.is_zero() {
        format!("{distance} {unit} acima do limite")
    } else {
        format!("{distance} {unit} abaixo do limite")
    }
}

/// "4,3".
fn average_text(average: Decimal) -> String {
    average
        .round_dp(1)
        .normalize()
        .to_string()
        .replace('.', ",")
}

fn count_text(count: u32, one: &str, many: &str) -> String {
    match count {
        1 => format!("1 {one}"),
        count => format!("{count} {many}"),
    }
}

/// Everything the screen shows, read in one go off the UI thread.
struct Snapshot {
    standing: Option<ReputationStanding>,
    products: Vec<ProductReviews>,
    low: Vec<LowReview>,
    names: BTreeMap<String, String>,
    connection: ConnectionState,
}

pub struct ReputationScreen {
    standing: Option<ReputationStanding>,
    products: Vec<ProductReviews>,
    low: Vec<LowReview>,
    names: BTreeMap<String, String>,
    connection: Option<ConnectionState>,
    syncing: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl ReputationScreen {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe_global::<ReputationSyncs>(Self::refresh)];
        let mut screen = Self {
            standing: None,
            products: Vec::new(),
            low: Vec::new(),
            names: BTreeMap::new(),
            connection: None,
            syncing: false,
            reads: 0,
            outcome: None,
            _subscriptions: subscriptions,
        };
        screen.refresh(cx);
        screen
    }

    /// Reads everything again, as when the screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (Some(reputation), Some(listings), Some(catalog)) =
            (reputation(cx), listings(cx), catalog::catalog(cx))
        else {
            return;
        };
        let connections = cx.global::<AppConnections>().0.clone();
        self.reads += 1;
        let read = self.reads;
        let reading = cx.background_executor().spawn(async move {
            let (items, names) = reviewed_items(&listings, &catalog).await?;
            Ok::<_, String>(Snapshot {
                standing: reputation.standing().await.map_err(|e| failure(&e))?,
                products: reputation.reviews(&items).await.map_err(|e| failure(&e))?,
                low: reputation.low_reviews().await.map_err(|e| failure(&e))?,
                names,
                connection: connections.state(Connection::MercadoLivre),
            })
        });
        cx.spawn(async move |this, cx| {
            let snapshot = reading.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                match snapshot {
                    Ok(snapshot) => {
                        this.standing = snapshot.standing;
                        this.products = snapshot.products;
                        this.low = snapshot.low;
                        this.names = snapshot.names;
                        this.connection = Some(snapshot.connection);
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads the Reputation and the Reviews from Mercado Livre now.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(round) = sync_now(cx, true) else {
            return;
        };
        self.syncing = true;
        self.outcome = None;
        cx.spawn(async move |this, cx| {
            let outcome = round.await;
            let _ = this.update(cx, |this, cx| {
                this.syncing = false;
                this.outcome = Some(match &outcome {
                    Ok(Some(round)) => Outcome::Done(synced_text(&round.synced).into()),
                    Ok(None) => {
                        Outcome::Failed(mercado_livre::failure(&PlatformError::NotConnected).into())
                    }
                    Err(error) => Outcome::Failed(error.clone().into()),
                });
                finished(&outcome, cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn mark_seen(&mut self, review: RecordId, cx: &mut Context<Self>) {
        let Some(reputation) = reputation(cx) else {
            return;
        };
        let marking = cx
            .background_executor()
            .spawn(async move { reputation.mark_seen(review).await });
        cx.spawn(async move |this, cx| {
            let marked = marking.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(error) = marked {
                    this.outcome = Some(Outcome::Failed(failure(&error).into()));
                }
                cx.default_global::<ReputationSyncs>().finished += 1;
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
        let when = match &self.standing {
            Some(standing) => format!("Lida em {}", catalog::day_and_time(standing.checked_at)),
            None => "Reputação ainda não lida.".into(),
        };
        h_flex()
            .gap_2()
            .items_center()
            .child(div().text_xs().text_color(t.text2).child(when))
            .child(
                Button::new("sync-reputation")
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

    fn render_reputation(&self, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let Some(standing) = &self.standing else {
            return section(
                "Reputação",
                muted(
                    "Ainda sem leitura. Conecte a conta do Mercado Livre e clique em Atualizar; \
                     depois o app lê a reputação a cada hora.",
                    &t,
                ),
                &t,
            );
        };
        let reputation = &standing.reputation;
        let (color, ink) = match reputation.color {
            Some(color) => (color.name(), color_ink(color, &t)),
            None => ("Sem cor ainda", t.text2),
        };
        let period = match reputation.period_days {
            Some(days) => format!(
                "{} nos últimos {days} dias",
                count_text(reputation.sales, "venda concluída", "vendas concluídas")
            ),
            None => count_text(reputation.sales, "venda concluída", "vendas concluídas"),
        };
        let mut facts = vec![period];
        facts.push(format!("{} no total", reputation.transactions));
        let mut lines = Vec::new();
        if reputation.color.is_none() {
            lines.push(muted(
                "O Mercado Livre dá a cor depois das primeiras vendas; até lá as métricas \
                 abaixo já mostram onde você está.",
                &t,
            ));
        }
        if let Some(until) = reputation.protected_until {
            let real = reputation
                .real_color
                .map(|color| {
                    format!(
                        " Sem a proteção, a cor seria {}.",
                        color.name().to_lowercase()
                    )
                })
                .unwrap_or_default();
            lines.push(muted(
                format!(
                    "Protegida pelo Mercado Livre até {}: as métricas abaixo são as reais, que \
                     contam quando a proteção acaba.{real}",
                    catalog::day(until)
                ),
                &t,
            ));
        }
        let body = v_flex()
            .gap_3()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().text_xl().font_semibold().text_color(ink).child(color))
                    .child(div().text_xs().text_color(t.text2).child(facts.join(" · "))),
            )
            .children(lines)
            .children(
                standing
                    .metrics
                    .iter()
                    .map(|metric| render_metric(metric, cx)),
            )
            .child(div().text_xs().text_color(t.text2).child(
                "Limites da tabela do Mercado Livre Brasil para a reputação verde; a cor de \
                 cada métrica é a melhor que ela permite sozinha.",
            ));
        section("Reputação", body.into_any_element(), &t)
    }

    fn render_tools(&self, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let rows = SellerTool::ALL.into_iter().map(|tool| {
            let unlocked = self.standing.as_ref().and_then(|standing| {
                standing
                    .tools
                    .iter()
                    .find(|(each, _)| *each == tool)
                    .map(|(_, unlocked)| *unlocked)
            });
            let (state, ink, detail) = match unlocked {
                Some(true) => (
                    "Disponível",
                    t.success,
                    format!(
                        "Sua reputação já tem o que é pedido: {}.",
                        tool.requirement()
                    ),
                ),
                Some(false) => (
                    "Bloqueado",
                    t.text2,
                    format!("Precisa de {}.", tool.requirement()),
                ),
                None => (
                    "Sem leitura",
                    t.text2,
                    format!("Precisa de {}.", tool.requirement()),
                ),
            };
            h_flex()
                .gap_3()
                .items_center()
                .child(div().w_32().font_medium().child(tool.name()))
                .child(kit::tag(state, ink, cx))
                .child(div().text_sm().text_color(t.text2).child(detail))
                .into_any_element()
        });
        section(
            "Ferramentas que a reputação libera",
            v_flex()
                .gap_2()
                .children(rows)
                .child(div().text_xs().text_color(t.text2).child(
                    "Quando uma delas é liberada, o app avisa uma vez, aqui, em Hoje e numa \
                     notificação.",
                ))
                .into_any_element(),
            &t,
        )
    }

    fn render_products(&self, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let body = if self.products.is_empty() {
            muted(
                "Nenhum anúncio aberto. Sincronize em Anúncios; as avaliações de cada um são \
                 lidas junto com a reputação.",
                &t,
            )
        } else {
            v_flex()
                .gap_2()
                .children(
                    self.products
                        .iter()
                        .map(|product| render_product(product, cx)),
                )
                .into_any_element()
        };
        section("Avaliações por produto", body, &t)
    }

    fn render_low(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let body = if self.low.is_empty() {
            muted(
                format!("Nenhuma avaliação de {} ou menos.", stars_text(LOW_RATING)),
                &t,
            )
        } else {
            v_flex()
                .gap_2()
                .children(
                    self.low
                        .iter()
                        .enumerate()
                        .map(|(index, low)| self.render_low_review(index, low, cx)),
                )
                .into_any_element()
        };
        section(
            &format!("Avaliações de {} ou menos", stars_text(LOW_RATING)),
            body,
            &t,
        )
    }

    fn render_low_review(
        &self,
        index: usize,
        low: &LowReview,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let name = self
            .names
            .get(&low.listing)
            .cloned()
            .unwrap_or_else(|| format!("Anúncio {}", low.listing));
        let id = low.id;
        h_flex()
            .gap_3()
            .items_start()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(kit::tag(
                stars_text(low.review.rating),
                if low.seen { t.text2 } else { t.danger },
                cx,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .text_sm()
                    .when(!low.review.title.trim().is_empty(), |column| {
                        column.child(div().font_medium().child(low.review.title.clone()))
                    })
                    .when(!low.review.text.trim().is_empty(), |column| {
                        column.child(div().child(format!("“{}”", low.review.text.trim())))
                    })
                    .child(div().text_xs().text_color(t.text2).child(format!(
                        "{name} · {} · {}",
                        low.listing,
                        catalog::day(low.review.at)
                    ))),
            )
            .child(if low.seen {
                div()
                    .text_xs()
                    .text_color(t.text2)
                    .child("Vista")
                    .into_any_element()
            } else {
                Button::new(SharedString::from(format!("seen-{index}")))
                    .label("Ciente")
                    .ghost()
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| this.mark_seen(id, cx)))
                    .into_any_element()
            })
            .into_any_element()
    }
}

impl Render for ReputationScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut parts = ScreenParts::new("Reputação e avaliações");
        if reputation(cx).is_none() || listings(cx).is_none() {
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
        parts.content.push(self.render_reputation(cx));
        parts.content.push(self.render_tools(cx));
        parts.content.push(self.render_low(cx));
        parts.content.push(self.render_products(cx));
        layout::screen(parts, cx)
    }
}

/// One Sync's line for the screen that ran it.
fn synced_text(report: &ReputationSync) -> String {
    let color = match report.color {
        Some(color) => format!("Reputação lida: {}.", color.name().to_lowercase()),
        None => "Reputação lida: ainda sem cor.".into(),
    };
    let reviews = match report.arrived.len() {
        0 => format!(
            " Avaliações de {} lidas, nenhuma baixa nova.",
            count_text(report.reviewed as u32, "anúncio", "anúncios")
        ),
        1 => " 1 avaliação baixa nova.".into(),
        arrived => format!(" {arrived} avaliações baixas novas."),
    };
    format!("{color}{reviews}")
}

fn render_metric(metric: &MetricStanding, cx: &App) -> AnyElement {
    let t = look(cx).tokens;
    h_flex()
        .gap_3()
        .items_center()
        .pl_3()
        .border_l_2()
        .border_color(t.border)
        .child(
            div()
                .w_48()
                .flex_none()
                .font_medium()
                .text_sm()
                .child(metric.metric.name()),
        )
        .child(div().w_40().flex_none().text_sm().child(format!(
            "{} · {}",
            metric.reading.rate.to_pt_br(),
            count_text(metric.reading.count, "venda", "vendas")
        )))
        .child(h_flex().w_24().flex_none().child(kit::tag(
            metric.fits.name(),
            color_ink(metric.fits, &t),
            cx,
        )))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .text_xs()
                .text_color(t.text2)
                .child(format!(
                    "Limite da verde: {} ({})",
                    metric.green_limit.to_pt_br(),
                    points_text(metric.points_to_green())
                ))
                .child(room_text(metric)),
        )
        .into_any_element()
}

fn render_product(product: &ProductReviews, cx: &App) -> AnyElement {
    let t = look(cx).tokens;
    let mut facts = vec![count_text(product.count(), "avaliação", "avaliações")];
    if product.low() > 0 {
        facts.push(count_text(product.low(), "baixa", "baixas"));
    }
    facts.push(count_text(
        product.listings.len() as u32,
        "anúncio",
        "anúncios",
    ));
    let (average, ink) = match product.average() {
        Some(average) => (
            format!("{} de 5", average_text(average)),
            if average <= Decimal::from(LOW_RATING) {
                t.danger
            } else {
                t.text
            },
        ),
        None => ("Sem avaliações".into(), t.text2),
    };
    h_flex()
        .gap_3()
        .items_center()
        .p_3()
        .rounded(t.radius_lg)
        .border(t.border_width)
        .border_color(t.frame)
        .bg(t.surface)
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
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
                                .child(product.name.clone()),
                        )
                        .when(product.unseen_low > 0, |row| {
                            row.child(kit::tag(
                                count_text(product.unseen_low as u32, "baixa nova", "baixas novas"),
                                t.danger,
                                cx,
                            ))
                        }),
                )
                .child(div().text_xs().text_color(t.text2).child(facts.join(" · "))),
        )
        .child(
            div()
                .flex_none()
                .text_lg()
                .font_semibold()
                .text_color(ink)
                .child(average),
        )
        .into_any_element()
}

fn section(title: &str, body: AnyElement, t: &Tokens) -> AnyElement {
    v_flex()
        .gap_2()
        .child(kit::section_heading(title.to_owned()))
        .child(
            div()
                .p_3()
                .rounded(t.radius_lg)
                .border(t.border_width)
                .border_color(t.frame)
                .bg(t.surface)
                .child(body),
        )
        .into_any_element()
}

fn muted(text: impl Into<SharedString>, t: &Tokens) -> AnyElement {
    div()
        .text_sm()
        .text_color(t.text2)
        .child(text.into())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use mascate_kernel::Percentage;
    use mascate_marketing::{MetricReading, ReputationMetric};

    use super::*;

    fn standing(room: Option<i64>) -> MetricStanding {
        MetricStanding {
            metric: ReputationMetric::Claims,
            reading: MetricReading::NONE,
            green_limit: Percentage::new(2.into()).unwrap(),
            fits: ReputationColor::Green,
            room,
        }
    }

    #[test]
    fn the_room_to_the_green_reads_in_sales() {
        assert_eq!(
            room_text(&standing(Some(3))),
            "cabem mais 3 antes de passar o limite da verde"
        );
        assert_eq!(
            room_text(&standing(Some(0))),
            "no limite da verde: mais 1 passa dele"
        );
        assert_eq!(
            room_text(&standing(Some(-2))),
            "2 acima do que a verde permite"
        );
        assert_eq!(
            room_text(&standing(None)),
            "sem vendas no período para contar"
        );
    }

    #[test]
    fn points_and_averages_read_in_portuguese() {
        assert_eq!(
            points_text(Decimal::new(-112, 2)),
            "1,12 pontos acima do limite"
        );
        assert_eq!(
            points_text(Decimal::new(531, 2)),
            "5,31 pontos abaixo do limite"
        );
        assert_eq!(points_text(Decimal::ONE), "1 ponto abaixo do limite");
        assert_eq!(average_text(Decimal::new(371428, 5)), "3,7");
        assert_eq!(average_text(Decimal::from(5)), "5");
    }

    #[test]
    fn a_review_without_a_title_reads_by_its_text() {
        assert_eq!(review_line("  ", " Chegou quebrado "), "Chegou quebrado");
        assert_eq!(review_line("Ruim", "Chegou quebrado"), "Ruim");
    }
}
