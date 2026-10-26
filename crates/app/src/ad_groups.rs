//! Ads in the Ad Campaigns (#181) in the Product Ads section of Vendas: each
//! Ad Group with the Product it advertises, its campaign and status, and
//! "Pôr em campanha", "Mover", "Pausar"/"Ativar" and "Tirar". A family of
//! variations or a catalog product moves whole, and the confirmation lists
//! every Listing that goes with it. A Product whose ads lose money is marked
//! with the suggestion to pause; nothing goes to Mercado Livre without the
//! owner's confirmation. The rules live in Marketing (ADR 0036).

use std::collections::{BTreeMap, BTreeSet};

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, SharedString, Subscription, Task, Window, div};
use mascate_kernel::PlatformError;
use mascate_marketing::{
    AdCampaigns, AdChange, AdChangeKind, AdGroupKind, AdRequest, AdStatus, AdsError,
    CampaignStatus, ChannelAdGroup, ChannelCampaign,
};

use crate::ads::{AdsSyncs, ad_campaigns, failure};
use crate::appearance::look;
use crate::catalog;
use crate::forms::{Outcome, notice};
use crate::kit;
use crate::listings::{self, listings};
use crate::mercado_livre;
use crate::reputation::reputation;
use crate::sales::table_row;

/// Everything the panel shows, read in one go off the UI thread.
struct Snapshot {
    groups: Vec<ChannelAdGroup>,
    campaigns: Vec<ChannelCampaign>,
    names: BTreeMap<String, String>,
    unlocked: bool,
}

/// The campaign and status the owner is picking for an ad.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Choice {
    /// The listing whose ad goes.
    listing: String,
    campaign: Option<String>,
    status: AdStatus,
}

pub struct AdGroupsPanel {
    groups: Vec<ChannelAdGroup>,
    campaigns: Vec<ChannelCampaign>,
    /// The name of the Product each listing sells, by the channel's id.
    names: BTreeMap<String, String>,
    /// The listings of the Products whose ads lose money in the period
    /// Vendas shows.
    suggested: BTreeSet<String>,
    unlocked: bool,
    choice: Option<Choice>,
    /// A change checked and waiting for the owner's confirmation.
    pending: Option<AdRequest>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl AdGroupsPanel {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe_global::<AdsSyncs>(Self::refresh)];
        let mut panel = Self {
            groups: Vec::new(),
            campaigns: Vec::new(),
            names: BTreeMap::new(),
            suggested: BTreeSet::new(),
            unlocked: false,
            choice: None,
            pending: None,
            busy: false,
            reads: 0,
            outcome: None,
            _subscriptions: subscriptions,
        };
        panel.refresh(cx);
        panel
    }

    /// Reads the Ad Groups, the campaigns and the Products the listings
    /// sell again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let (Some(campaigns), Some(listings), Some(catalog), Some(reputation)) = (
            ad_campaigns(cx),
            listings(cx),
            catalog::catalog(cx),
            reputation(cx),
        ) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        let reading = cx.background_executor().spawn(async move {
            let names = listings::listing_products(&listings, &catalog)
                .await?
                .into_iter()
                .map(|(listing, product)| (listing, product.name))
                .collect();
            Ok::<_, String>(Snapshot {
                groups: campaigns.ad_groups().await.map_err(|e| failure(&e))?,
                campaigns: campaigns.campaigns().await.map_err(|e| failure(&e))?,
                names,
                unlocked: crate::ads::unlocked(&reputation).await? == Some(true),
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
                        this.groups = snapshot.groups;
                        this.campaigns = snapshot.campaigns;
                        this.names = snapshot.names;
                        this.unlocked = snapshot.unlocked;
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Marks the listings of the Products whose ads lose money in the
    /// period Vendas shows.
    pub fn suggest(&mut self, listings: BTreeSet<String>, cx: &mut Context<Self>) {
        if self.suggested != listings {
            self.suggested = listings;
            cx.notify();
        }
    }

    /// Checks pausing the first running ad among `listings`, as the
    /// suggestion on a Product losing money offers.
    pub fn pause_any(&mut self, listings: &[String], cx: &mut Context<Self>) {
        let running = self.groups.iter().find(|group| {
            group.status == AdStatus::Active
                && group
                    .listings
                    .iter()
                    .any(|listing| listings.contains(listing))
        });
        match running.and_then(|group| Some((group.listings.first()?, group.campaign.clone()?))) {
            Some((listing, campaign)) => {
                let listing = listing.clone();
                self.ask(
                    listing,
                    AdChange::Place {
                        campaign,
                        status: AdStatus::Paused,
                    },
                    cx,
                );
            }
            None => self.fail(
                "Nenhum anúncio ativo deste produto numa campanha: ele já está pausado ou fora \
                 de campanha.",
                cx,
            ),
        }
    }

    fn fail(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.outcome = Some(Outcome::Failed(text.into()));
        cx.notify();
    }

    fn name_of(&self, group: &ChannelAdGroup) -> String {
        group
            .listings
            .iter()
            .find_map(|listing| self.names.get(listing).cloned())
            .or_else(|| group.title.clone())
            .unwrap_or_else(|| group.listings.join(", "))
    }

    fn campaign(&self, id: &str) -> Option<&ChannelCampaign> {
        self.campaigns.iter().find(|campaign| campaign.id == id)
    }

    /// Opens the choice of campaign for the ad of `listing`.
    fn choose(&mut self, group: &ChannelAdGroup, cx: &mut Context<Self>) {
        let Some(listing) = group.listings.first() else {
            return;
        };
        self.choice = Some(Choice {
            listing: listing.clone(),
            campaign: None,
            status: AdStatus::Active,
        });
        self.pending = None;
        self.outcome = None;
        cx.notify();
    }

    /// Checks what the choice holds. Nothing goes to Mercado Livre yet.
    fn check_choice(&mut self, cx: &mut Context<Self>) {
        let Some(choice) = self.choice.clone() else {
            return;
        };
        let Some(campaign) = choice.campaign else {
            return self.fail("Escolha a campanha.", cx);
        };
        self.ask(
            choice.listing,
            AdChange::Place {
                campaign,
                status: choice.status,
            },
            cx,
        );
    }

    /// Checks a change to the ad of `listing`.
    fn ask(&mut self, listing: String, change: AdChange, cx: &mut Context<Self>) {
        let Some(campaigns) = ad_campaigns(cx) else {
            return;
        };
        let unlocked = self.unlocked;
        let preparing: Task<Result<AdRequest, AdsError>> = cx
            .background_executor()
            .spawn(async move { campaigns.prepare_ad(&listing, change, unlocked).await });
        self.busy = true;
        self.pending = None;
        self.outcome = None;
        cx.spawn(async move |this, cx| {
            let prepared = preparing.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match prepared {
                    Ok(request) => {
                        this.choice = None;
                        this.pending = Some(request);
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Sends the confirmed change: only ever on the owner's click.
    fn confirm(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self.pending.take() else {
            return;
        };
        let (Some(campaigns), Some(channel)) = (ad_campaigns(cx), mercado_livre::adapter(cx))
        else {
            return self.fail(mercado_livre::failure(&PlatformError::NotConnected), cx);
        };
        let done = request_text(
            &request,
            |id| self.campaign_name(id),
            &self.name_of(request.group()),
        );
        self.busy = true;
        self.outcome = None;
        let sending = cx
            .background_executor()
            .spawn(async move { campaigns.send_ad(channel.as_ref(), request.confirm()).await });
        cx.spawn(async move |this, cx| {
            let sent = sending.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.outcome = Some(match sent {
                    Ok(_) => Outcome::Done(format!("Enviado ao Mercado Livre: {done}").into()),
                    Err(error) => Outcome::Failed(failure(&error).into()),
                });
                cx.default_global::<AdsSyncs>().finished += 1;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn campaign_name(&self, id: &str) -> String {
        self.campaign(id).map_or_else(
            || format!("Campanha {id}"),
            |campaign| campaign.name.clone(),
        )
    }

    fn render_group(
        &self,
        index: usize,
        group: &ChannelAdGroup,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let placed = group.campaign.as_deref();
        let place = match placed {
            Some(id) => format!("{} · {}", self.campaign_name(id), group.status.name()),
            None => "Fora de campanha".to_owned(),
        };
        let many = (group.kind != AdGroupKind::Item).then(|| {
            format!(
                "{} · {}",
                group.kind.name(),
                match group.listings.len() {
                    1 => "1 anúncio".to_owned(),
                    count => format!("{count} anúncios"),
                }
            )
        });
        let suggested = placed.is_some()
            && group.status == AdStatus::Active
            && group
                .listings
                .iter()
                .any(|listing| self.suggested.contains(listing));
        let disabled = self.busy || !self.unlocked;
        let mut buttons = h_flex().gap_1().flex_none();
        let id = |name: &str| SharedString::from(format!("ad-group-{name}-{index}"));
        match placed {
            None => {
                let group = group.clone();
                buttons = buttons.child(
                    Button::new(id("place"))
                        .label("Pôr em campanha")
                        .outline()
                        .small()
                        .disabled(disabled)
                        .on_click(cx.listener(move |this, _, _, cx| this.choose(&group, cx))),
                );
            }
            Some(campaign) => {
                let listing = group.listings.first().cloned().unwrap_or_default();
                let (label, status) = match group.status {
                    AdStatus::Active => ("Pausar", AdStatus::Paused),
                    _ => ("Ativar", AdStatus::Active),
                };
                let moving = group.clone();
                let campaign = campaign.to_owned();
                let status_listing = listing.clone();
                buttons = buttons
                    .child(
                        Button::new(id("status"))
                            .label(label)
                            .outline()
                            .small()
                            .disabled(disabled)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ask(
                                    status_listing.clone(),
                                    AdChange::Place {
                                        campaign: campaign.clone(),
                                        status,
                                    },
                                    cx,
                                )
                            })),
                    )
                    .child(
                        Button::new(id("move"))
                            .label("Mover")
                            .ghost()
                            .small()
                            .disabled(disabled)
                            .on_click(cx.listener(move |this, _, _, cx| this.choose(&moving, cx))),
                    )
                    .child(
                        Button::new(id("remove"))
                            .label("Tirar")
                            .ghost()
                            .small()
                            .disabled(disabled)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.ask(listing.clone(), AdChange::Remove, cx)
                            })),
                    );
            }
        }
        table_row(&t)
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
                                    .truncate()
                                    .font_medium()
                                    .child(self.name_of(group)),
                            )
                            .children(
                                suggested.then(|| kit::tag("sugestão: pausar", t.danger, cx)),
                            ),
                    )
                    .child(
                        div().text_xs().text_color(t.text2).child(
                            many.into_iter()
                                .chain([place])
                                .collect::<Vec<_>>()
                                .join(" · "),
                        ),
                    ),
            )
            .child(buttons)
            .into_any_element()
    }

    fn render_choice(&self, choice: &Choice, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let group = self
            .groups
            .iter()
            .find(|group| group.listings.contains(&choice.listing));
        let now = group.and_then(|group| group.campaign.clone());
        let lead = group.map(|group| {
            div().text_sm().child(format!(
                "Para onde vai o anúncio de “{}”:",
                self.name_of(group)
            ))
        });
        let campaigns = self
            .campaigns
            .iter()
            .filter(|campaign| {
                matches!(
                    campaign.status,
                    CampaignStatus::Active | CampaignStatus::Paused
                ) && Some(&campaign.id) != now.as_ref()
            })
            .map(|campaign| {
                let id = campaign.id.clone();
                let button = Button::new(SharedString::from(format!("choose-{}", campaign.id)))
                    .label(format!(
                        "{} ({})",
                        campaign.name,
                        campaign.status.name().to_lowercase()
                    ))
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(choice) = &mut this.choice {
                            choice.campaign = Some(id.clone());
                        }
                        cx.notify();
                    }));
                if choice.campaign.as_ref() == Some(&campaign.id) {
                    button.primary()
                } else {
                    button.outline()
                }
            })
            .collect::<Vec<_>>();
        let statuses = [AdStatus::Active, AdStatus::Paused].map(|status| {
            let button = Button::new(SharedString::from(format!("ad-status-{}", status.code())))
                .label(status.name())
                .small()
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(choice) = &mut this.choice {
                        choice.status = status;
                    }
                    cx.notify();
                }));
            if choice.status == status {
                button.primary()
            } else {
                button.outline()
            }
        });
        let empty = campaigns.is_empty().then(|| {
            div()
                .text_sm()
                .text_color(t.text2)
                .child("Nenhuma outra campanha ativa ou pausada; crie uma em Campanhas.")
        });
        v_flex()
            .gap_3()
            .pt_3()
            .border_t(t.border_width)
            .border_color(t.frame)
            .children(lead)
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().font_medium().child("Campanha"))
                    .children(empty)
                    .child(h_flex().gap_1().flex_wrap().children(campaigns)),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().font_medium().child("O anúncio fica"))
                    .child(h_flex().gap_1().children(statuses)),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("check-ad")
                            .label("Conferir")
                            .icon(IconName::Search)
                            .outline()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.check_choice(cx))),
                    )
                    .child(
                        Button::new("close-ad")
                            .label("Cancelar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.choice = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_pending(&self, request: &AdRequest, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let text = request_text(
            request,
            |id| self.campaign_name(id),
            &self.name_of(request.group()),
        );
        let together = (request.listings().len() > 1).then(|| {
            v_flex()
                .gap_1()
                .child(div().text_sm().child(format!(
                    "Vão juntos os {} anúncios desta {}:",
                    request.listings().len(),
                    match request.group().kind {
                        AdGroupKind::Catalog => "ficha de catálogo",
                        _ => "família de variações",
                    }
                )))
                .children(request.listings().iter().map(|listing| {
                    let name = self
                        .names
                        .get(listing)
                        .map_or_else(String::new, |name| format!(" ({name})"));
                    div().text_sm().pl_3().child(format!("• {listing}{name}"))
                }))
        });
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.accent)
            .child(
                div()
                    .font_medium()
                    .child("Conferido. Confirme para enviar:"),
            )
            .child(div().text_sm().child(text))
            .children(together)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("send-ad")
                            .label("Confirmar e enviar ao Mercado Livre")
                            .icon(IconName::Check)
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                    )
                    .child(
                        Button::new("back-ad")
                            .label("Voltar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pending = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for AdGroupsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let rows: Vec<AnyElement> = self
            .groups
            .clone()
            .iter()
            .enumerate()
            .map(|(index, group)| self.render_group(index, group, cx))
            .collect();
        let empty = rows.is_empty().then(|| {
            div().text_sm().text_color(t.text2).child(
                "Nenhum anúncio lido no Product Ads ainda. Um anúncio só entra numa campanha \
                 depois que o Mercado Livre cria o Ad Group dele; o app lê os Ad Groups uma vez \
                 por dia.",
            )
        });
        let choice = self
            .choice
            .clone()
            .map(|choice| self.render_choice(&choice, cx));
        let pending = self
            .pending
            .clone()
            .map(|request| self.render_pending(&request, cx));
        v_flex()
            .gap_2()
            .child(div().font_medium().child("Anúncios nas campanhas"))
            .child(div().text_xs().text_color(t.text2).child(
                "Pôr, mover, pausar ou tirar um anúncio só acontece depois da sua confirmação. \
                 \"Sugestão: pausar\" marca o produto que gastou pelo menos R$ 20,00 no período \
                 com ROAS real abaixo do de equilíbrio; o app nunca pausa sozinho.",
            ))
            .children(empty)
            .children(rows)
            .children(choice)
            .children(pending)
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
    }
}

/// What goes to Mercado Livre, in words.
fn request_text(request: &AdRequest, campaign: impl Fn(&str) -> String, name: &str) -> String {
    let from = request.from().map(|from| from.name.clone());
    let to = request
        .to()
        .map(|to| to.name.clone())
        .or_else(|| request.group().campaign.as_deref().map(&campaign));
    let status = request
        .status()
        .map(|status| status.name().to_lowercase())
        .unwrap_or_default();
    match (request.kind(), from, to) {
        (AdChangeKind::Place, _, Some(to)) => {
            format!("Pôr o anúncio de “{name}” na campanha “{to}”, {status}.")
        }
        (AdChangeKind::Move, Some(from), Some(to)) => {
            format!("Mover o anúncio de “{name}” da campanha “{from}” para “{to}”, {status}.")
        }
        (AdChangeKind::Pause, _, Some(to)) => format!(
            "Pausar o anúncio de “{name}” na campanha “{to}”: ele para de aparecer e de gastar \
             até você ativar de novo."
        ),
        (AdChangeKind::Activate, _, Some(to)) => format!(
            "Ativar o anúncio de “{name}” na campanha “{to}”: ele volta a aparecer e a gastar."
        ),
        (AdChangeKind::Remove, Some(from), _) => format!(
            "Tirar o anúncio de “{name}” da campanha “{from}”: ele deixa de ser patrocinado."
        ),
        _ => format!("Mudar o anúncio de “{name}”."),
    }
}

/// Where each listing's ad stands, by the channel's id, as the Listing
/// editor shows it: "Product Ads: campanha “Fones” · Ativo". Nothing
/// without the app's Ad Campaigns.
pub(crate) async fn ad_lines(
    campaigns: Option<&AdCampaigns>,
) -> Result<BTreeMap<String, String>, String> {
    let Some(campaigns) = campaigns else {
        return Ok(BTreeMap::new());
    };
    let groups = campaigns.ad_groups().await.map_err(|e| failure(&e))?;
    let kept = campaigns.campaigns().await.map_err(|e| failure(&e))?;
    let mut lines = BTreeMap::new();
    for group in groups {
        let line = ad_line(&group, &kept);
        for listing in group.listings {
            lines.insert(listing, line.clone());
        }
    }
    Ok(lines)
}

fn ad_line(group: &ChannelAdGroup, campaigns: &[ChannelCampaign]) -> String {
    match &group.campaign {
        Some(id) => {
            let name = campaigns
                .iter()
                .find(|campaign| &campaign.id == id)
                .map_or_else(
                    || format!("Campanha {id}"),
                    |campaign| campaign.name.clone(),
                );
            format!("Product Ads: campanha “{name}” · {}", group.status.name())
        }
        None => "Product Ads: fora de campanha".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(campaign: Option<&str>, status: AdStatus) -> ChannelAdGroup {
        ChannelAdGroup {
            id: "1406300002".into(),
            kind: AdGroupKind::Family,
            title: Some("Fone de Ouvido Over-ear".into()),
            listings: vec!["MLB4100000004".into(), "MLB4100000005".into()],
            campaign: campaign.map(str::to_owned),
            status,
        }
    }

    #[test]
    fn the_listing_editor_reads_the_ads_campaign_and_status() {
        let campaigns = [ChannelCampaign::named(
            "351",
            "Fones",
            CampaignStatus::Active,
        )];
        assert_eq!(
            ad_line(&group(Some("351"), AdStatus::Paused), &campaigns),
            "Product Ads: campanha “Fones” · Pausado"
        );
        assert_eq!(
            ad_line(&group(Some("360"), AdStatus::Active), &campaigns),
            "Product Ads: campanha “Campanha 360” · Ativo"
        );
        assert_eq!(
            ad_line(&group(None, AdStatus::Paused), &campaigns),
            "Product Ads: fora de campanha"
        );
    }
}
