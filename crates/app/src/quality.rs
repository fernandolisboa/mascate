//! Qualidade (#23, #136): what each Sales Channel says about each open
//! listing (Mercado Livre's score and actions, Amazon's listing issues,
//! Shopee's listings with issues) and Shopee's rating of the shop, the
//! listings to work on first on top (something pending, then units sold and
//! visits in the last 30 days), each action with the link where the channel
//! lets the owner fix it. The quality is read in each Sync of Anúncios and
//! on this screen's own button; the rules live in Marketing (ADR 0021).

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Global, Hsla, SharedString, Window, div};
use mascate_commerce::{ListingStatus, Listings, Orders};
use mascate_integrations::ConnectionState;
use mascate_kernel::{SalesChannelKind, Timestamp};
use mascate_marketing::{
    ActionKind, IMPACT_DAYS, ListedItem, ListingQuality, MetricUnit, PerformanceMetric,
    PerformanceRating, QualityAction, QualityError, QualityLevel, QualityRow, QualitySource,
    QualitySync, Rating, ShopPerformance,
};
use rust_decimal::Decimal;

use crate::appearance::{Tokens, look};
use crate::catalog::{self, NO_DATABASE};
use crate::forms::{Outcome, notice};
use crate::kit;
use crate::layout;
use crate::listings::{self, listings};
use crate::orders::{self, orders};
use crate::parts::ScreenParts;
use crate::sales_channels::{self, ChannelFilter, ChannelPorts, Reach};

/// The app's Listing Quality; absent when the database did not open.
pub struct AppQuality(pub Arc<ListingQuality>);

impl Global for AppQuality {}

pub(crate) fn quality(cx: &App) -> Option<Arc<ListingQuality>> {
    cx.try_global::<AppQuality>().map(|app| app.0.clone())
}

/// The channels that say what to fix in a listing.
fn rating_channels(cx: &App) -> Reach {
    sales_channels::reach(cx).only(|channel| channel.quality.is_some())
}

/// The open listings on the channels `rated`, one per channel id (a
/// listing's variations share it), which the panel rates.
async fn open_listings(
    listings: &Listings,
    rated: &[SalesChannelKind],
) -> Result<Vec<ListedItem>, String> {
    let mut seen = HashSet::new();
    Ok(listings
        .listings()
        .await
        .map_err(|e| listings::failure(&e))?
        .into_iter()
        .filter(|listing| {
            rated.contains(&listing.channel) && listing.listed.status != ListingStatus::Closed
        })
        .filter(|listing| seen.insert((listing.channel, listing.listed.id.clone())))
        .map(|listing| ListedItem {
            channel: listing.channel,
            listing: listing.listed.id,
            title: listing.listed.title,
            link: listing.listed.link,
            units_sold: 0,
        })
        .collect())
}

/// Reads again what `source`'s channel says about each of its open
/// listings, and its rating of the shop.
async fn sync(
    listings: &Listings,
    quality: &ListingQuality,
    source: &dyn QualitySource,
) -> Result<QualitySync, String> {
    let kind = source.rated_channel();
    let ids: Vec<String> = open_listings(listings, &[kind])
        .await?
        .into_iter()
        .map(|listed| listed.listing)
        .collect();
    quality
        .sync(source, &ids)
        .await
        .map_err(|e| failure(kind, &e))
}

/// What a Sync of Listing Quality read on each channel, or why not.
pub(crate) type ChannelsRead = Vec<(SalesChannelKind, Result<QualitySync, String>)>;

/// Each of `channels` that rates its listings, read on its own, as each
/// Sync of Anúncios does after reading the listings: one channel failing
/// leaves the others read.
pub(crate) async fn sync_channels(
    listings: &Listings,
    quality: &ListingQuality,
    channels: &[ChannelPorts],
) -> ChannelsRead {
    let mut read = Vec::new();
    for channel in channels {
        if let Some(source) = &channel.quality {
            read.push((
                channel.kind(),
                sync(listings, quality, source.as_ref()).await,
            ));
        }
    }
    read
}

/// The panel: every open listing on the channels `rated` with its units
/// sold, in the order to work on them.
async fn panel(
    listings: &Listings,
    orders: &Orders,
    quality: &ListingQuality,
    rated: &[SalesChannelKind],
) -> Result<Vec<QualityRow>, String> {
    let mut sold = std::collections::HashMap::new();
    for kind in rated {
        let units = orders
            .units_sold_since(*kind, quality.impact_since())
            .await
            .map_err(|e| orders::failure(&e))?;
        sold.insert(*kind, units);
    }
    let listed: Vec<ListedItem> = open_listings(listings, rated)
        .await?
        .into_iter()
        .map(|listed| ListedItem {
            units_sold: sold
                .get(&listed.channel)
                .and_then(|units| units.get(&listed.listing))
                .copied()
                .unwrap_or(0),
            ..listed
        })
        .collect();
    quality
        .panel(&listed)
        .await
        .map_err(|e| failure_reading(&e))
}

/// What a Sync of every channel read, one line per channel, and whether
/// any failed.
pub(crate) fn synced_channels(
    read: &[(SalesChannelKind, Result<QualitySync, String>)],
) -> (String, bool) {
    let lines: Vec<String> = read
        .iter()
        .map(|(kind, read)| match read {
            Ok(report) => synced(*kind, report),
            Err(error) => format!(
                "Não li a qualidade dos anúncios {}: {error}",
                sales_channels::of_channel(*kind)
            ),
        })
        .collect();
    (lines.join(" "), read.iter().any(|(_, read)| read.is_err()))
}

/// One channel's Sync, for the screen that ran it.
fn synced(kind: SalesChannelKind, report: &QualitySync) -> String {
    let rated = report.checked - report.not_rated;
    let channel = sales_channels::of_channel(kind);
    let opening = sales_channels::opening(kind);
    let mut text = match (rated, report.pending) {
        (0, _) if report.checked == 0 => {
            format!("Qualidade {channel} lida: nenhum anúncio aberto.")
        }
        (0, _) => format!(
            "Qualidade {channel} lida, mas {} ainda não avaliou os anúncios.",
            sales_channels::the(kind)
        ),
        (_, 0) => format!("Qualidade {channel} lida: nada pendente nos anúncios."),
        (_, 1) => format!("Qualidade {channel} lida: 1 ação pendente."),
        (_, pending) => format!("Qualidade {channel} lida: {pending} ações pendentes."),
    };
    if rated > 0 && report.not_rated > 0 {
        text.push_str(&format!(
            " {opening} ainda não avaliou {}.",
            listings_text(report.not_rated)
        ));
    }
    match report.below_target {
        Some(1) => text.push_str(" 1 métrica da loja está fora da meta."),
        Some(below) if below > 1 => {
            text.push_str(&format!(" {below} métricas da loja estão fora da meta."))
        }
        _ => {}
    }
    text
}

fn failure(kind: SalesChannelKind, error: &QualityError) -> String {
    match error {
        QualityError::Platform(error) => sales_channels::failure(kind, error),
        error => failure_reading(error),
    }
}

fn failure_reading(error: &QualityError) -> String {
    format!("Não consegui ler ou gravar a qualidade dos anúncios: {error}")
}

/// Everything the screen shows, read in one go off the UI thread.
struct Snapshot {
    rows: Vec<QualityRow>,
    performances: Vec<ShopPerformance>,
    last_sync: Option<Timestamp>,
    connections: Vec<(SalesChannelKind, ConnectionState)>,
}

pub struct QualityScreen {
    rows: Vec<QualityRow>,
    performances: Vec<ShopPerformance>,
    last_sync: Option<Timestamp>,
    /// Each channel that rates its listings, with its Connection.
    connections: Vec<(SalesChannelKind, ConnectionState)>,
    filter: ChannelFilter,
    syncing: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
}

impl QualityScreen {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut screen = Self {
            rows: Vec::new(),
            performances: Vec::new(),
            last_sync: None,
            connections: Vec::new(),
            filter: ChannelFilter::default(),
            syncing: false,
            reads: 0,
            outcome: None,
        };
        screen.refresh(cx);
        screen
    }

    /// Reads the panel again, as when the screen comes into view.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.read(cx, false);
    }

    /// Reads the quality from every connected channel, then the panel.
    fn sync(&mut self, cx: &mut Context<Self>) {
        self.syncing = true;
        self.outcome = None;
        self.read(cx, true);
        cx.notify();
    }

    /// Runs a Sync of the connected channels when `syncing`, off the UI
    /// thread, then reads the panel again.
    fn read(&mut self, cx: &mut Context<Self>, syncing: bool) {
        let (Some(listings), Some(orders), Some(quality)) = (listings(cx), orders(cx), quality(cx))
        else {
            return;
        };
        let reach = rating_channels(cx);
        self.reads += 1;
        let read = self.reads;
        let working = cx.background_executor().spawn(async move {
            let rated = if syncing {
                let channels = reach.connected().await;
                Some(sync_channels(&listings, &quality, &channels).await)
            } else {
                None
            };
            let connections = reach.states();
            let kinds: Vec<SalesChannelKind> = connections.iter().map(|(kind, _)| *kind).collect();
            let snapshot = async {
                Ok::<_, String>(Snapshot {
                    rows: panel(&listings, &orders, &quality, &kinds).await?,
                    performances: quality
                        .performances()
                        .await
                        .map_err(|e| failure_reading(&e))?,
                    last_sync: quality.last_sync().await.map_err(|e| failure_reading(&e))?,
                    connections,
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
                            this.performances = snapshot.performances;
                            this.last_sync = snapshot.last_sync;
                            this.connections = snapshot.connections;
                        }
                        Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                    }
                }
                match rated {
                    Some(read) if read.is_empty() => {}
                    Some(read) => {
                        let (text, failed) = synced_channels(&read);
                        this.outcome = Some(if failed {
                            Outcome::Failed(text.into())
                        } else {
                            Outcome::Done(text.into())
                        });
                    }
                    None => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn pick(&mut self, filter: ChannelFilter, cx: &mut Context<Self>) {
        self.filter = filter;
        cx.notify();
    }

    /// The channels the panel holds rows of.
    fn held(&self) -> BTreeSet<SalesChannelKind> {
        self.rows.iter().map(|row| row.listing.channel).collect()
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
                    .disabled(self.syncing || !sales_channels::any_connected(&self.connections))
                    .on_click(cx.listener(|this, _, _, cx| this.sync(cx))),
            )
            .into_any_element()
    }

    fn render_row(&self, index: usize, several: bool, cx: &App) -> AnyElement {
        let t = look(cx).tokens;
        let row = &self.rows[index];
        let listed = &row.listing;
        let kind = listed.channel;
        let mut facts = vec![listed.listing.clone(), sold_text(listed.units_sold)];
        match row.visits {
            Some(visits) => facts.push(visits_text(visits)),
            // Only Mercado Livre tells the visits.
            None if kind == SalesChannelKind::MercadoLivre => {
                facts.push("visitas ainda não lidas".into())
            }
            None => {}
        }
        if let Some(at) = row.checked_at {
            facts.push(format!("lida em {}", catalog::day(at)));
        }
        let (rating, ink) = rating_tag(row, &t);
        let body = match &row.rating {
            Rating::NotChecked => muted(
                "Ainda sem leitura de qualidade: clique em Atualizar ou sincronize em Anúncios.",
                &t,
            ),
            Rating::NotRated => muted(
                format!(
                    "{} ainda não avaliou a qualidade deste anúncio.",
                    sales_channels::opening(kind)
                ),
                &t,
            ),
            Rating::Rated(quality) if quality.pending.is_empty() => muted(
                format!(
                    "Nada pendente: {} não pede mais nada.",
                    sales_channels::the(kind)
                ),
                &t,
            ),
            Rating::Rated(quality) => v_flex()
                .gap_2()
                .children(
                    quality
                        .pending
                        .iter()
                        .enumerate()
                        .map(|(at, action)| render_action(kind, index, at, action, cx)),
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
                                    .when(several, |line| {
                                        line.child(kit::tag(kind.name(), t.text2, cx))
                                    })
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
                                        kit::external_link(
                                            format!("open-{index}"),
                                            format!("Abrir {}", sales_channels::on(kind)),
                                            link,
                                            cx,
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

    /// Each channel's rating of the shop, and every metric against its
    /// target, those missing it first.
    fn render_performance(&self, cx: &App) -> Option<AnyElement> {
        let t = look(cx).tokens;
        let held = self
            .performances
            .iter()
            .map(|shop| shop.channel)
            .chain(self.held())
            .collect();
        let shown: Vec<&ShopPerformance> = self
            .performances
            .iter()
            .filter(|shop| self.filter.shows(shop.channel, &held))
            .collect();
        if shown.is_empty() {
            return None;
        }
        Some(
            v_flex()
                .gap_3()
                .children(shown.into_iter().map(|shop| {
                    let performance = &shop.performance;
                    let below = performance.below_target();
                    let mut metrics: Vec<&PerformanceMetric> = performance.metrics.iter().collect();
                    metrics.sort_by_key(|metric| metric.meets_target() != Some(false));
                    let summary = match below {
                        0 => "Todas as métricas dentro da meta.".to_owned(),
                        1 => "1 métrica fora da meta.".to_owned(),
                        below => format!("{below} métricas fora da meta."),
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
                                .gap_2()
                                .items_center()
                                .child(kit::section_heading(format!(
                                    "Desempenho da loja {}",
                                    sales_channels::on(shop.channel)
                                )))
                                .children(performance.rating.map(|rating| {
                                    kit::tag(rating.name(), performance_ink(rating, &t), cx)
                                })),
                        )
                        .child(div().text_xs().text_color(t.text2).child(format!(
                            "{summary} Lido em {}. Os números são {}; as correções são feitas \
                             na Central do Vendedor.",
                            catalog::day_and_time(shop.checked_at),
                            sales_channels::of_channel(shop.channel)
                        )))
                        .children(
                            metrics
                                .into_iter()
                                .map(|metric| render_metric(metric, &t, cx)),
                        )
                        .into_any_element()
                }))
                .into_any_element(),
        )
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
        for text in sales_channels::connection_notices(&self.connections) {
            parts
                .notices
                .push(kit::info_notice(text, cx).into_any_element());
        }
        parts
            .notices
            .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        let held = self.held();
        let shown: Vec<usize> = (0..self.rows.len())
            .filter(|index| self.filter.shows(self.rows[*index].listing.channel, &held))
            .collect();
        let pending = shown
            .iter()
            .filter(|index| !self.rows[**index].pending().is_empty())
            .count();
        let heading = match (shown.len(), pending) {
            (0, _) => "Anúncios".to_owned(),
            (count, 0) => format!("{}, nada pendente", listings_text(count)),
            (count, pending) => format!("{}, {} com o que melhorar", listings_text(count), pending),
        };
        parts
            .content
            .extend(self.filter.chips("quality-channel", &held, cx, Self::pick));
        parts.content.extend(self.render_performance(cx));
        parts.content.push(
            v_flex()
                .gap_3()
                .child(v_flex().gap_1().child(kit::section_heading(heading)).child(
                    div().text_xs().text_color(t.text2).child(format!(
                        "Primeiro os anúncios com algo a melhorar, dos que mais venderam e \
                             mais receberam visitas nos últimos {IMPACT_DAYS} dias. As correções \
                             são feitas no canal, pelo link de cada ação."
                    )),
                ))
                .when(self.rows.is_empty(), |list| {
                    list.child(muted(
                        "Nenhum anúncio aberto. Sincronize em Anúncios para trazer os seus dos \
                         canais conectados; a qualidade de cada um é lida no mesmo Sync.",
                        &t,
                    ))
                })
                .children(
                    shown
                        .into_iter()
                        .map(|index| self.render_row(index, held.len() > 1, cx)),
                )
                .into_any_element(),
        );
        layout::screen(parts, cx)
    }
}

/// One pending action: what it weighs, the channel's words and the link to
/// fix it.
fn render_action(
    kind: SalesChannelKind,
    row: usize,
    at: usize,
    action: &QualityAction,
    cx: &App,
) -> AnyElement {
    let t = look(cx).tokens;
    let (weighs, ink) = match action.kind {
        // Only Mercado Livre scores the listing.
        ActionKind::Problem if kind == SalesChannelKind::MercadoLivre => {
            ("Derruba a nota", t.danger)
        }
        ActionKind::Problem => ("Problema", t.danger),
        ActionKind::Opportunity => ("Sugestão", t.accent_text),
    };
    h_flex()
        .gap_2()
        .items_start()
        .pl_3()
        .border_l_2()
        .border_color(t.border)
        .child(kit::tag(weighs, ink, cx))
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap_0p5()
                .text_sm()
                .child(action.text.clone())
                .children(action.link.clone().map(|link| {
                    kit::external_link(format!("fix-{row}-{at}"), action.label.clone(), link, cx)
                })),
        )
        .into_any_element()
}

/// A metric: its name and group, its value now and before, and its target.
fn render_metric(metric: &PerformanceMetric, t: &Tokens, cx: &App) -> AnyElement {
    let mut facts = vec![metric.group.name().to_owned()];
    if let Some(previous) = metric.previous {
        facts.push(format!(
            "período anterior: {}",
            metric_value(previous, metric.unit)
        ));
    }
    if let Some(target) = &metric.target {
        facts.push(format!(
            "meta: {} {}",
            target.comparator.sign(),
            metric_value(target.value, metric.unit)
        ));
    }
    let (value, ink) = match (metric.current, metric.meets_target()) {
        (None, _) => ("sem dado".to_owned(), t.text2),
        (Some(current), Some(false)) => (metric_value(current, metric.unit), t.danger),
        (Some(current), Some(true)) => (metric_value(current, metric.unit), t.success),
        (Some(current), None) => (metric_value(current, metric.unit), t.text),
    };
    h_flex()
        .gap_3()
        .items_center()
        .pl_3()
        .border_l_2()
        .border_color(t.border)
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap_0p5()
                .child(div().text_sm().child(metric.name.clone()))
                .child(div().text_xs().text_color(t.text2).child(facts.join(" · "))),
        )
        .child(
            div()
                .flex_none()
                .font_semibold()
                .text_color(ink)
                .child(value),
        )
        .when(metric.meets_target() == Some(false), |line| {
            line.child(kit::tag("Fora da meta", t.danger, cx))
        })
        .into_any_element()
}

/// "12,5%", "3 dias": a metric's value in its unit.
fn metric_value(value: Decimal, unit: MetricUnit) -> String {
    let number = value.normalize().to_string().replace('.', ",");
    match unit {
        MetricUnit::Number => number,
        MetricUnit::Percent => format!("{number}%"),
        MetricUnit::Seconds => format!("{number} s"),
        MetricUnit::Hours => format!("{number} h"),
        MetricUnit::Days if value == Decimal::ONE => "1 dia".into(),
        MetricUnit::Days => format!("{number} dias"),
    }
}

fn performance_ink(rating: PerformanceRating, t: &Tokens) -> Hsla {
    match rating {
        PerformanceRating::Poor => t.danger,
        PerformanceRating::ImprovementNeeded => t.accent_text,
        PerformanceRating::Good | PerformanceRating::Excellent => t.success,
    }
}

fn muted(text: impl Into<SharedString>, t: &Tokens) -> AnyElement {
    div()
        .text_sm()
        .text_color(t.text2)
        .child(text.into())
        .into_any_element()
}

/// Mercado Livre's level; elsewhere, whether a problem is pending.
fn rating_tag(row: &QualityRow, t: &Tokens) -> (&'static str, Hsla) {
    match &row.rating {
        Rating::NotChecked => ("Sem leitura", t.text2),
        Rating::NotRated => ("Sem avaliação", t.text2),
        Rating::Rated(quality) => match &quality.score {
            Some(score) => (
                score.level.name(),
                match score.level {
                    QualityLevel::Basic => t.danger,
                    QualityLevel::Standard => t.accent_text,
                    QualityLevel::Professional => t.success,
                },
            ),
            None if quality
                .pending
                .iter()
                .any(|action| action.kind == ActionKind::Problem) =>
            {
                ("Com problema", t.danger)
            }
            None if quality.pending.is_empty() => ("Em dia", t.success),
            None => ("Com sugestões", t.accent_text),
        },
    }
}

fn score(rating: &Rating) -> Option<u8> {
    match rating {
        Rating::Rated(quality) => quality.score.map(|score| score.points),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn read(checked: usize, not_rated: usize, pending: usize) -> QualitySync {
        QualitySync {
            checked,
            not_rated,
            pending,
            below_target: None,
        }
    }

    #[test]
    fn each_channels_sync_says_what_it_found_in_its_own_words() {
        assert_eq!(
            synced(SalesChannelKind::MercadoLivre, &read(3, 1, 2)),
            "Qualidade do Mercado Livre lida: 2 ações pendentes. O Mercado Livre ainda não \
             avaliou 1 anúncio."
        );
        assert_eq!(
            synced(
                SalesChannelKind::Shopee,
                &QualitySync {
                    below_target: Some(2),
                    ..read(2, 0, 0)
                }
            ),
            "Qualidade da Shopee lida: nada pendente nos anúncios. 2 métricas da loja estão \
             fora da meta."
        );
        assert_eq!(
            synced(SalesChannelKind::Amazon, &read(2, 2, 0)),
            "Qualidade da Amazon lida, mas a Amazon ainda não avaliou os anúncios."
        );
        let (text, failed) = synced_channels(&[
            (SalesChannelKind::MercadoLivre, Ok(read(1, 0, 1))),
            (SalesChannelKind::Amazon, Err("sem conexão".into())),
        ]);
        assert!(failed);
        assert_eq!(
            text,
            "Qualidade do Mercado Livre lida: 1 ação pendente. Não li a qualidade dos anúncios \
             da Amazon: sem conexão"
        );
    }

    #[test]
    fn a_metric_reads_in_its_unit() {
        assert_eq!(
            metric_value(Decimal::new(125, 1), MetricUnit::Percent),
            "12,5%"
        );
        assert_eq!(metric_value(Decimal::ONE, MetricUnit::Days), "1 dia");
        assert_eq!(
            metric_value(Decimal::new(30, 1), MetricUnit::Days),
            "3 dias"
        );
        assert_eq!(metric_value(Decimal::from(42), MetricUnit::Number), "42");
    }
}
