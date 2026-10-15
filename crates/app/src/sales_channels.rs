//! The Sales Channels the owner sells on (ADR 0033), each behind its
//! Connection with the ports its adapter offers. Each channel's slice adds
//! its adapter here, as each Platform's does to `CommissionSources`.
//! Configurações › Vendas sets the order of the channels.

use std::collections::BTreeSet;
use std::sync::Arc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, Global, Window, div, px};
use mascate_commerce::{
    ChannelBilling, ChannelOrders, ChannelPayments, ChannelRanking, SalesChannel, ShippingLabels,
};
use mascate_integrations::{Connection, ConnectionState, Connections};
use mascate_kernel::SalesChannelKind;

use crate::appearance::look;
use crate::connections::AppConnections;
use crate::forms::{Outcome, notice};
use crate::listings;

/// One Sales Channel's adapter, port by port; a port the channel does not
/// offer is absent.
#[derive(Clone)]
pub struct ChannelPorts {
    pub connection: Connection,
    pub listings: Arc<dyn SalesChannel>,
    pub orders: Arc<dyn ChannelOrders>,
    pub billing: Option<Arc<dyn ChannelBilling>>,
    pub payments: Option<Arc<dyn ChannelPayments>>,
    pub labels: Option<Arc<dyn ShippingLabels>>,
}

impl ChannelPorts {
    pub fn kind(&self) -> SalesChannelKind {
        self.listings.sales_channel()
    }
}

/// Every Sales Channel the app has an adapter for.
pub struct SalesChannels(pub Vec<ChannelPorts>);

impl Global for SalesChannels {}

/// The order the owner ranks the channels in; absent when the database did
/// not open.
pub struct AppChannelRanking(pub Arc<ChannelRanking>);

impl Global for AppChannelRanking {}

pub fn ranking(cx: &App) -> Option<Arc<ChannelRanking>> {
    cx.try_global::<AppChannelRanking>()
        .map(|app| app.0.clone())
}

/// What the app needs to reach the connected channels off the UI thread.
#[derive(Clone)]
pub struct Reach {
    channels: Vec<ChannelPorts>,
    connections: Arc<Connections>,
    ranking: Option<Arc<ChannelRanking>>,
}

pub fn reach(cx: &App) -> Reach {
    Reach {
        channels: cx
            .try_global::<SalesChannels>()
            .map(|channels| channels.0.clone())
            .unwrap_or_default(),
        connections: cx.global::<AppConnections>().0.clone(),
        ranking: ranking(cx),
    }
}

impl Reach {
    /// The channels whose Connection is up, in the owner's order. Each one
    /// takes its place in the order the first time it is seen connected.
    pub async fn connected(&self) -> Vec<ChannelPorts> {
        let mut connected: Vec<ChannelPorts> = self
            .channels
            .iter()
            .filter(|channel| {
                self.connections.state(channel.connection) == ConnectionState::Connected
            })
            .cloned()
            .collect();
        let Some(ranking) = &self.ranking else {
            return connected;
        };
        let mut order = Vec::new();
        for channel in &connected {
            match ranking.connected(channel.kind()).await {
                Ok(ranked) => order = ranked,
                Err(error) => eprintln!("could not rank the Sales Channels: {error}"),
            }
        }
        connected.sort_by_key(|channel| {
            order
                .iter()
                .position(|kind| *kind == channel.kind())
                .unwrap_or(usize::MAX)
        });
        connected
    }
}

/// The channel `kind`, connected or not.
pub fn of(cx: &App, kind: SalesChannelKind) -> Option<ChannelPorts> {
    cx.try_global::<SalesChannels>()?
        .0
        .iter()
        .find(|channel| channel.kind() == kind)
        .cloned()
}

/// "no Mercado Livre", "na Amazon": where something happens on the channel.
pub fn on(kind: SalesChannelKind) -> String {
    format!("n{} {}", article(kind), kind.name())
}

/// "do Mercado Livre", "da Amazon": what belongs to the channel.
pub fn of_channel(kind: SalesChannelKind) -> String {
    format!("d{} {}", article(kind), kind.name())
}

/// "ao Mercado Livre", "à Amazon": where something is sent.
pub fn to(kind: SalesChannelKind) -> String {
    match article(kind) {
        "o" => format!("ao {}", kind.name()),
        _ => format!("à {}", kind.name()),
    }
}

fn article(kind: SalesChannelKind) -> &'static str {
    match kind {
        SalesChannelKind::MercadoLivre => "o",
        SalesChannelKind::Amazon | SalesChannelKind::Shopee | SalesChannelKind::Magalu => "a",
    }
}

/// A list's choice of one Sales Channel, or all of them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChannelFilter(Option<SalesChannelKind>);

impl ChannelFilter {
    /// Whether a row on `kind` shows, of a list holding the channels `held`:
    /// a channel the list no longer holds shows every row.
    pub fn shows(self, kind: SalesChannelKind, held: &BTreeSet<SalesChannelKind>) -> bool {
        match self.0 {
            Some(chosen) if held.contains(&chosen) => chosen == kind,
            _ => true,
        }
    }

    /// "Todos os canais" and a chip for each channel the list holds; none
    /// while it holds a single one.
    pub fn chips<T: 'static>(
        self,
        id: &'static str,
        held: &BTreeSet<SalesChannelKind>,
        cx: &mut Context<T>,
        pick: fn(&mut T, ChannelFilter, &mut Context<T>),
    ) -> Option<AnyElement> {
        if held.len() < 2 {
            return None;
        }
        let chosen = self.0.filter(|kind| held.contains(kind));
        let chip = |index: usize, label: &'static str, value: Option<SalesChannelKind>| {
            let button = Button::new((id, index)).label(label).small();
            if chosen == value {
                button.primary()
            } else {
                button.outline()
            }
            .on_click(cx.listener(move |this, _, _, cx| pick(this, ChannelFilter(value), cx)))
        };
        let mut row = h_flex()
            .flex_wrap()
            .gap_1()
            .child(chip(0, "Todos os canais", None));
        for (index, kind) in held.iter().enumerate() {
            row = row.child(chip(index + 1, kind.name(), Some(*kind)));
        }
        Some(row.into_any_element())
    }
}

/// Configurações › Vendas: the order of the Sales Channels, first to last.
pub struct ChannelRankingSection {
    /// `None` until the first read finishes.
    ranking: Option<Vec<SalesChannelKind>>,
    busy: bool,
    outcome: Option<Outcome>,
}

impl ChannelRankingSection {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut section = Self {
            ranking: None,
            busy: false,
            outcome: None,
        };
        section.save(None, cx);
        section
    }

    /// Saves `ranking` when there is one, then reads the order back.
    fn save(&mut self, ranking: Option<Vec<SalesChannelKind>>, cx: &mut Context<Self>) {
        let Some(stored) = self::ranking(cx) else {
            return;
        };
        self.busy = true;
        self.outcome = None;
        let saving = cx.background_executor().spawn(async move {
            let saved = match &ranking {
                Some(ranking) => stored.rank(ranking).await.map(|()| true),
                None => Ok(false),
            };
            (saved, stored.ranking().await)
        });
        cx.spawn(async move |this, cx| {
            let (saved, read) = saving.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match read {
                    Ok(read) => this.ranking = Some(read),
                    Err(error) => {
                        this.outcome = Some(Outcome::Failed(listings::failure(&error).into()))
                    }
                }
                match saved {
                    Ok(true) => {
                        this.outcome = Some(Outcome::Done("Ordem dos canais salva.".into()))
                    }
                    Ok(false) => {}
                    Err(error) => {
                        this.outcome = Some(Outcome::Failed(listings::failure(&error).into()))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Swaps the channel at `index` with the one before it.
    fn raise(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(mut ranking) = self.ranking.clone() else {
            return;
        };
        if index == 0 || index >= ranking.len() {
            return;
        }
        ranking.swap(index - 1, index);
        self.save(Some(ranking), cx);
    }
}

impl Render for ChannelRankingSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let ranking = self.ranking.clone().unwrap_or_default();
        let count = ranking.len();
        let rows = ranking.into_iter().enumerate().map(|(index, kind)| {
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .w_6()
                        .text_sm()
                        .text_color(t.text2)
                        .child(format!("{}º", index + 1)),
                )
                .child(div().flex_1().text_sm().font_medium().child(kind.name()))
                .child(
                    Button::new(("raise-channel", index))
                        .icon(IconName::ArrowUp)
                        .ghost()
                        .small()
                        .disabled(self.busy || index == 0)
                        .on_click(cx.listener(move |this, _, _, cx| this.raise(index, cx))),
                )
                .child(
                    Button::new(("lower-channel", index))
                        .icon(IconName::ArrowDown)
                        .ghost()
                        .small()
                        .disabled(self.busy || index + 1 == count)
                        .on_click(cx.listener(move |this, _, _, cx| this.raise(index + 1, cx))),
                )
        });
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.surface)
            .child(div().text_sm().text_color(t.text2).child(
                "A ordem dos canais de venda: o Mercado Livre vem primeiro até você mudar, e cada \
                 canal conectado depois entra no fim. Os Syncs de pedidos seguem esta ordem.",
            ))
            .child(v_flex().gap_1().w(px(320.)).children(rows))
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_channel_filter_shows_its_channel_or_everything() {
        let held = BTreeSet::from([SalesChannelKind::MercadoLivre, SalesChannelKind::Amazon]);
        let all = ChannelFilter::default();
        let amazon = ChannelFilter(Some(SalesChannelKind::Amazon));
        let shopee = ChannelFilter(Some(SalesChannelKind::Shopee));

        assert!(all.shows(SalesChannelKind::MercadoLivre, &held));
        assert!(amazon.shows(SalesChannelKind::Amazon, &held));
        assert!(!amazon.shows(SalesChannelKind::MercadoLivre, &held));
        // A channel the list no longer holds shows every row.
        assert!(shopee.shows(SalesChannelKind::MercadoLivre, &held));
    }

    #[test]
    fn each_channel_reads_with_its_article() {
        assert_eq!(on(SalesChannelKind::MercadoLivre), "no Mercado Livre");
        assert_eq!(on(SalesChannelKind::Amazon), "na Amazon");
        assert_eq!(of_channel(SalesChannelKind::Shopee), "da Shopee");
        assert_eq!(to(SalesChannelKind::MercadoLivre), "ao Mercado Livre");
        assert_eq!(to(SalesChannelKind::Magalu), "à Magalu");
    }
}
