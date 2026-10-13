//! Links de afiliado on the Afiliação screen (#99): for each Program whose
//! Platform the app tags links of, its Affiliate Links per Promotion
//! Channel with "Copiar", and "Criar link" to tag a pasted link with a
//! channel's marking (or, on Amazon, the tracking id the owner created for
//! it). Shopee's link is the short link its API gives (#100). The Program's
//! rules are checked first: what only warns is shown, a Restricted Feature
//! that is off refuses (ADR 0031).

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, App, ClipboardItem, Entity, Global, Subscription, Window, div, px};
use mascate_affiliate::{
    Affiliate, AffiliateError, AffiliateLink, AffiliatePlatform, CreatedLink, LinkTagger,
    NewAffiliateLink, Program, PromotedProduct, Promotion, Reach, ReportedProgram, RuleWarning,
};
use mascate_kernel::RecordId;
use mascate_marketing::{ChannelKind, PromotionChannel};

use crate::affiliate::{affiliate, failure};
use crate::appearance::look;
use crate::forms::{Outcome, input, notice};
use crate::kit;
use crate::promotion_channels::{ChannelChanges, promotion_channels};
use crate::promotion_rules::{practice_name, standing_name};
use crate::sales::table_row;
use crate::shopee;

/// The Platforms whose links the app tags, each through its adapter. Each
/// Platform's slice adds its tagger here.
pub struct LinkTaggers(pub Vec<Arc<dyn LinkTagger>>);

impl Global for LinkTaggers {}

pub(crate) fn tagger(platform: AffiliatePlatform, cx: &App) -> Option<Arc<dyn LinkTagger>> {
    cx.try_global::<LinkTaggers>()?
        .0
        .iter()
        .find(|tagger| tagger.platform() == platform)
        .cloned()
}

/// A Promotion Channel in the Affiliate's words (ADR 0031, decision 1):
/// paid search is paid search, every other kind is the owner's own channel.
pub(crate) fn promotion(channel: &PromotionChannel) -> Promotion {
    Promotion {
        reach: match channel.kind {
            ChannelKind::PaidSearch => Reach::PaidSearch,
            ChannelKind::TelegramChannel
            | ChannelKind::WhatsAppGroup
            | ChannelKind::Instagram
            | ChannelKind::TikTok
            | ChannelKind::Other => Reach::OwnChannel,
        },
        keywords: channel.keywords.clone(),
        ..Promotion::default()
    }
}

/// Creates the link of `product` at `pasted` for `channel`, with the
/// channel's marking, on the one Program of `tagger`'s Platform: Shopee
/// Afiliados, from a found offer.
pub(crate) async fn create_on_platform(
    affiliate: &Affiliate,
    tagger: &dyn LinkTagger,
    product: String,
    pasted: String,
    channel: &PromotionChannel,
) -> Result<CreatedLink, AffiliateError> {
    let program = affiliate
        .program_of(tagger)
        .await?
        .ok_or(AffiliateError::NoSuchProgram)?;
    affiliate
        .create_link(
            &NewAffiliateLink {
                program: program.id,
                product,
                pasted,
                channel: channel.id,
                tag: channel.default_tag.clone(),
                promotion: promotion(channel),
            },
            tagger,
        )
        .await
}

/// What creating a link for `channel` gave, as the owner reads it.
pub(crate) fn created_text(created: &CreatedLink, channel: &str) -> String {
    if created.existed {
        format!(
            "{} já tinha link para {channel}: é o mesmo de antes.",
            created.link.product
        )
    } else {
        format!(
            "Link de {} para {channel} criado com a marcação {}. Use \"Copiar\" para divulgar.",
            created.link.product, created.link.tag
        )
    }
}

/// Why creating a link on `platform` failed, as the owner reads it: the
/// Platform's own words when its API tags the link.
pub(crate) fn link_failure(platform: Option<AffiliatePlatform>, error: &AffiliateError) -> String {
    match (platform, error) {
        (Some(AffiliatePlatform::Shopee), AffiliateError::Platform(error)) => {
            shopee::link_failure(error)
        }
        _ => failure(error),
    }
}

/// A rule the link breaks or may break, as the owner reads it.
pub(crate) fn warning_text(warning: &RuleWarning) -> String {
    match warning {
        RuleWarning::Practice { practice, standing } => format!(
            "{}: {} neste programa.",
            practice_name(*practice),
            standing_name(*standing).to_lowercase()
        ),
        RuleWarning::BrandKeyword(keyword) => {
            format!("A palavra-chave \"{keyword}\" é marca da plataforma em busca paga.")
        }
        RuleWarning::PriceWithoutTime => {
            "O preço precisa da data e da hora em que foi visto.".into()
        }
        RuleWarning::Restricted(flag) => format!(
            "\"{}\" está ligada por você em Funcionalidades restritas: {}",
            flag.name, flag.risk
        ),
    }
}

/// What one read of the section brings.
struct Snapshot {
    links: Vec<AffiliateLink>,
    programs: Vec<Program>,
    promoted: Vec<PromotedProduct>,
}

async fn read(affiliate: &Affiliate) -> Result<Snapshot, AffiliateError> {
    Ok(Snapshot {
        links: affiliate.links().await?,
        programs: affiliate.programs().await?,
        promoted: affiliate.promoted_products().await?,
    })
}

pub struct AffiliateLinksSection {
    links: Vec<AffiliateLink>,
    programs: Vec<Program>,
    promoted: Vec<PromotedProduct>,
    channels: Vec<PromotionChannel>,
    /// The Program the form creates a link for.
    creating: Option<RecordId>,
    product: Entity<InputState>,
    pasted: Entity<InputState>,
    /// The Amazon tracking id of the channel.
    tracking_id: Entity<InputState>,
    channel: Option<RecordId>,
    /// What the last link created breaks or may break.
    warnings: Vec<String>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    form: Option<Outcome>,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl AffiliateLinksSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            // A channel created, renamed or deleted in Divulgação.
            cx.observe_global_in::<ChannelChanges>(window, Self::refresh),
        ];
        Self {
            links: Vec::new(),
            programs: Vec::new(),
            promoted: Vec::new(),
            channels: Vec::new(),
            creating: None,
            product: input("Como a plataforma chama o produto", window, cx),
            pasted: input("https://go.hotmart.com/…", window, cx),
            tracking_id: input("mascate-tg-20", window, cx),
            channel: None,
            warnings: Vec::new(),
            busy: false,
            reads: 0,
            form: None,
            outcome: None,
            _subscriptions: subscriptions,
        }
    }

    /// Reads the links, the Programs and the channels again.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run(window, cx, |_| async { Ok(()) }, |_, _, _, _| {});
    }

    /// Runs `change` off the UI thread, then reads everything again and
    /// hands `done` what `change` gave.
    fn run<T, F, Fut, D>(&mut self, window: &mut Window, cx: &mut Context<Self>, change: F, done: D)
    where
        T: Send + 'static,
        F: FnOnce(Arc<Affiliate>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, AffiliateError>> + Send,
        D: FnOnce(&mut Self, T, &mut Window, &mut Context<Self>) + 'static,
    {
        let Some(affiliate) = affiliate(cx) else {
            return;
        };
        let channels = promotion_channels(cx);
        self.reads += 1;
        let read_number = self.reads;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let changed = change(affiliate.clone()).await;
            let channels = match channels {
                Some(channels) => channels.channels().await.map_err(|error| error.to_string()),
                None => Ok(Vec::new()),
            };
            (changed, read(&affiliate).await, channels)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (changed, snapshot, channels) = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reads == read_number {
                    this.busy = false;
                    match snapshot {
                        Ok(snapshot) => {
                            this.links = snapshot.links;
                            this.programs = snapshot.programs;
                            this.promoted = snapshot.promoted;
                        }
                        Err(error) => this.outcome = Some(Outcome::Failed(failure(&error).into())),
                    }
                    match channels {
                        Ok(channels) => this.channels = channels,
                        Err(error) => {
                            this.outcome = Some(Outcome::Failed(
                                format!("Não consegui ler os canais de divulgação: {error}").into(),
                            ))
                        }
                    }
                }
                match changed {
                    Ok(value) => done(this, value, window, cx),
                    Err(error) => {
                        let platform = this.creating_platform();
                        this.form = Some(Outcome::Failed(link_failure(platform, &error).into()))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Opens "Criar link" for `product` of `program`, or for the Program's
    /// only product when `product` is `None`.
    pub fn open(
        &mut self,
        program: RecordId,
        product: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let product = product.unwrap_or_else(|| {
            let mut products = self
                .promoted
                .iter()
                .filter(|promoted| promoted.program.id == program);
            match (products.next(), products.next()) {
                (Some(only), None) => only.product.clone(),
                _ => String::new(),
            }
        });
        self.product
            .update(cx, |field, cx| field.set_value(product, window, cx));
        self.pasted
            .update(cx, |field, cx| field.set_value("", window, cx));
        self.creating = Some(program);
        self.channel = None;
        self.warnings.clear();
        self.form = None;
        cx.notify();
    }

    /// Opens "Criar link" on the one Program of `tagger`'s Platform,
    /// creating it when no Conversion brought it yet.
    fn open_platform(
        &mut self,
        tagger: Arc<dyn LinkTagger>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run(
            window,
            cx,
            move |affiliate| async move { affiliate.program_of(tagger.as_ref()).await },
            |this, program: Option<Program>, window, cx| {
                if let Some(program) = program {
                    this.open(program.id, None, window, cx);
                }
            },
        );
    }

    fn creating_platform(&self) -> Option<AffiliatePlatform> {
        let creating = self.creating?;
        self.programs
            .iter()
            .find(|program| program.id == creating)
            .map(|program| program.platform)
    }

    fn pick_channel(&mut self, channel: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.channel = Some(channel);
        // The tracking id the channel already used on Amazon, if any.
        let used = self
            .links
            .iter()
            .find(|link| {
                link.channel == channel && link.program.platform == AffiliatePlatform::Amazon
            })
            .map(|link| link.tag.clone());
        if let Some(used) = used {
            self.tracking_id
                .update(cx, |field, cx| field.set_value(used, window, cx));
        }
        cx.notify();
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(program) = self
            .creating
            .and_then(|id| self.programs.iter().find(|program| program.id == id))
            .cloned()
        else {
            return;
        };
        let Some(channel) = self
            .channel
            .and_then(|id| self.channels.iter().find(|channel| channel.id == id))
            .cloned()
        else {
            self.form = Some(Outcome::Failed("Escolha o canal onde vai divulgar.".into()));
            cx.notify();
            return;
        };
        let Some(tagger) = tagger(program.platform, cx) else {
            return;
        };
        let new = NewAffiliateLink {
            program: program.id,
            product: self.product.read(cx).value().to_string(),
            pasted: self.pasted.read(cx).value().to_string(),
            channel: channel.id,
            tag: if program.platform == AffiliatePlatform::Amazon {
                self.tracking_id.read(cx).value().to_string()
            } else {
                channel.default_tag.clone()
            },
            promotion: promotion(&channel),
        };
        self.form = None;
        self.warnings.clear();
        self.run(
            window,
            cx,
            move |affiliate| async move { affiliate.create_link(&new, tagger.as_ref()).await },
            move |this, created: CreatedLink, window, cx| {
                this.warnings = created.warnings.iter().map(warning_text).collect();
                this.outcome = Some(Outcome::Done(created_text(&created, &channel.name).into()));
                if this.warnings.is_empty() {
                    this.creating = None;
                }
                this.pasted
                    .update(cx, |field, cx| field.set_value("", window, cx));
            },
        );
    }

    fn delete(&mut self, link: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        self.run(
            window,
            cx,
            move |affiliate| async move { affiliate.delete_link(link).await },
            |this, _, _, _| {
                this.outcome = Some(Outcome::Done(
                    "Link apagado. As vendas que ele já trouxe continuam com o canal.".into(),
                ));
            },
        );
    }

    fn copy(&mut self, link: &AffiliateLink, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(link.tagged.clone()));
        self.outcome = Some(Outcome::Done(
            format!("Link de {} copiado.", link.product).into(),
        ));
        cx.notify();
    }

    fn channel_name(&self, id: RecordId) -> String {
        self.channels
            .iter()
            .find(|channel| channel.id == id)
            .map_or_else(
                || "canal apagado".to_owned(),
                |channel| channel.name.clone(),
            )
    }

    fn render_link(
        &self,
        index: usize,
        link: &AffiliateLink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let copied = link.clone();
        let id = link.id;
        table_row(&t)
            .flex_wrap()
            .ml_6()
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(260.))
                    .child(div().font_medium().truncate().child(format!(
                        "{} · {}",
                        link.product,
                        self.channel_name(link.channel)
                    )))
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text2)
                            .truncate()
                            .child(link.tagged.clone()),
                    ),
            )
            .child(kit::tag(
                format!("marcação {}", link.tag),
                t.accent_text,
                cx,
            ))
            .child(
                Button::new(("copy-link", index))
                    .label("Copiar")
                    .outline()
                    .small()
                    .on_click(cx.listener(move |this, _, _, cx| this.copy(&copied, cx))),
            )
            .child(
                Button::new(("delete-link", index))
                    .label("Apagar")
                    .ghost()
                    .small()
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| this.delete(id, window, cx))),
            )
            .into_any_element()
    }

    fn render_form(&self, program: &Program, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .child(div().text_sm().child(label))
                .child(div().w(px(420.)).child(Input::new(input).small()))
        };
        let chips = self
            .channels
            .iter()
            .enumerate()
            .map(|(index, channel)| {
                let id = channel.id;
                let chip = Button::new(("link-channel", index))
                    .label(channel.name.clone())
                    .small()
                    .disabled(self.busy);
                if self.channel == Some(id) {
                    chip.primary()
                } else {
                    chip.ghost()
                }
                .on_click(cx.listener(move |this, _, window, cx| this.pick_channel(id, window, cx)))
            })
            .collect::<Vec<_>>();
        let amazon = program.platform == AffiliatePlatform::Amazon;
        let shopee = program.platform == AffiliatePlatform::Shopee;
        v_flex()
            .gap_3()
            .ml_6()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.border)
            .bg(t.raised)
            .child(
                div()
                    .font_medium()
                    .child(format!("Criar link · {}", program.name)),
            )
            .child(field("Produto", &self.product))
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child("Canal"))
                    .child(if chips.is_empty() {
                        div()
                            .text_sm()
                            .text_color(t.text2)
                            .child("Nenhum canal ainda: crie um em Divulgação.")
                            .into_any_element()
                    } else {
                        h_flex()
                            .flex_wrap()
                            .gap_1()
                            .children(chips)
                            .into_any_element()
                    }),
            )
            .child(if shopee {
                field("Link do produto ou da loja na Shopee", &self.pasted).child(
                    div().text_xs().text_color(t.text2).child(
                        "O app pede à Shopee o link curto de afiliado, com a marcação do canal, \
                         e manda o link colado como está. Em Ofertas › Buscar na Shopee, \
                         \"Criar link\" faz o mesmo sem colar nada.",
                    ),
                )
            } else {
                field("Link de afiliado", &self.pasted).child(
                    div().text_xs().text_color(t.text2).child(format!(
                        "O link que a {} dá na sua área de afiliado. O app só acrescenta a \
                         marcação do canal; o resto fica como está.",
                        program.platform.name()
                    )),
                )
            })
            .when(amazon, |form| {
                form.child(field("Tracking id do canal", &self.tracking_id).child(
                    div().text_xs().text_color(t.text2).child(
                        "Crie um tracking id para este canal no painel do Associados \
                         (Gerenciar IDs de rastreamento; a conta tem até 100) e digite-o \
                         aqui. Ele termina em -20.",
                    ),
                ))
            })
            .children(self.form.as_ref().map(|outcome| notice(outcome, cx)))
            .when(!self.warnings.is_empty(), |form| {
                form.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_sm()
                                .font_medium()
                                .child("Confira as regras deste programa antes de divulgar:"),
                        )
                        .children(self.warnings.iter().map(|warning| {
                            kit::info_notice(warning.clone(), cx).into_any_element()
                        })),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("create-link")
                            .label("Criar link")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy || self.channels.is_empty())
                            .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
                    )
                    .child(
                        Button::new("cancel-link")
                            .label(if self.warnings.is_empty() {
                                "Cancelar"
                            } else {
                                "Fechar"
                            })
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.creating = None;
                                this.warnings.clear();
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_program(
        &self,
        index: usize,
        program: &Program,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        let links: Vec<AnyElement> = self
            .links
            .iter()
            .enumerate()
            .filter(|(_, link)| link.program.id == program.id)
            .map(|(at, link)| self.render_link(at, link, cx))
            .collect();
        let count = links.len();
        let form = (self.creating == Some(program.id)).then(|| self.render_form(program, cx));
        let id = program.id;
        v_flex()
            .gap_2()
            .child(
                table_row(&t)
                    .flex_wrap()
                    .child(kit::tag(program.platform.name(), t.text2, cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(220.))
                            .child(div().font_medium().truncate().child(program.name.clone()))
                            .child(div().text_xs().text_color(t.text2).child(match count {
                                0 => "nenhum link ainda".to_owned(),
                                1 => "1 link".to_owned(),
                                count => format!("{count} links"),
                            })),
                    )
                    .child(
                        Button::new(("open-create-link", index))
                            .label("Criar link")
                            .outline()
                            .small()
                            .disabled(self.busy || form.is_some())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open(id, None, window, cx)
                            })),
                    ),
            )
            .children(form)
            .children(links)
            .into_any_element()
    }
}

impl AffiliateLinksSection {
    /// A Platform with a single Program that no Conversion brought yet, as
    /// Shopee before the first sale: "Criar link" creates the Program.
    fn render_awaited(
        &self,
        index: usize,
        tagger: Arc<dyn LinkTagger>,
        program: &ReportedProgram,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        table_row(&t)
            .flex_wrap()
            .child(kit::tag(tagger.platform().name(), t.text2, cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(220.))
                    .child(div().font_medium().truncate().child(program.name.clone()))
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text2)
                            .child("nenhum link ainda"),
                    ),
            )
            .child(
                Button::new(("open-awaited-link", index))
                    .label("Criar link")
                    .outline()
                    .small()
                    .disabled(self.busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_platform(tagger.clone(), window, cx)
                    })),
            )
            .into_any_element()
    }
}

impl Render for AffiliateLinksSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let (taggable, untagged): (Vec<Program>, Vec<Program>) = self
            .programs
            .clone()
            .into_iter()
            .partition(|program| tagger(program.platform, cx).is_some());
        let mut rows: Vec<AnyElement> = taggable
            .iter()
            .enumerate()
            .map(|(index, program)| self.render_program(index, program, cx))
            .collect();
        let awaited: Vec<(Arc<dyn LinkTagger>, ReportedProgram)> = cx
            .try_global::<LinkTaggers>()
            .map(|taggers| {
                taggers
                    .0
                    .iter()
                    .filter(|tagger| {
                        !self
                            .programs
                            .iter()
                            .any(|program| program.platform == tagger.platform())
                    })
                    .filter_map(|tagger| Some((tagger.clone(), tagger.program()?)))
                    .collect()
            })
            .unwrap_or_default();
        for (index, (tagger, program)) in awaited.into_iter().enumerate() {
            rows.push(self.render_awaited(index, tagger, &program, cx));
        }
        let mut untagged: Vec<&str> = untagged
            .iter()
            .map(|program| program.platform.name())
            .collect();
        untagged.dedup();
        v_flex()
            .gap_3()
            .child(kit::section_heading("Links de afiliado"))
            .child(div().text_sm().text_color(t.text2).child(
                "Cole o link de afiliado de um produto e escolha o canal: o app acrescenta a \
                 marcação do canal, que volta em cada venda e liga a comissão a ele. Na Shopee, \
                 a própria Shopee devolve o link curto já marcado.",
            ))
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .when(rows.is_empty(), |section| {
                section.child(div().text_sm().text_color(t.text2).child(
                    "Nenhum programa da Hotmart, Kiwify, Monetizze, Eduzz ou Amazon ainda: eles \
                     aparecem aqui com a primeira comissão.",
                ))
            })
            .children(rows)
            .when(!untagged.is_empty(), |section| {
                section.child(div().text_xs().text_color(t.text2).child(format!(
                    "O app ainda não marca links de {}.",
                    untagged.join(", ")
                )))
            })
    }
}
