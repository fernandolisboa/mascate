//! Links de afiliado on the Afiliação screen (#99): for each Program whose
//! Platform the app tags links of, its Affiliate Links per Promotion
//! Channel with "Copiar", and "Criar link" to tag a pasted link with a
//! channel's marking (or, on Amazon, the tracking id the owner created for
//! it). Shopee's link is the short link its API gives (#100). The Program's
//! rules are checked first: what only warns is shown, a Restricted Feature
//! that is off refuses (ADR 0031). "Encurtar" gives a link its Short Link
//! through Short.io (#158, ADR 0035). "Nova afiliação" adds a Program
//! before its first sale and goes straight to its link (#194).

use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Entity, EventEmitter, Global, Subscription, Window, div, px,
};
use mascate_affiliate::{
    Affiliate, AffiliateError, AffiliateLink, AffiliatePlatform, CreatedLink, LinkTagger,
    NewAffiliateLink, NewProgram, Program, PromotedProduct, Promotion, Reach, ReportedProgram,
    RuleWarning,
};
use mascate_kernel::{PlatformError, RecordId};
use mascate_marketing::{ChannelKind, PromotionChannel};

use crate::affiliate::{
    PlatformSelect, affiliate, failure, form_platform, platform_field, platform_select,
};
use crate::appearance::look;
use crate::forms::{Outcome, input, notice};
use crate::kit;
use crate::offer_posts::StartPost;
use crate::preferences;
use crate::promotion_channels::{ChannelChanges, promotion_channels};
use crate::promotion_rules::{practice_name, standing_name};
use crate::sales::table_row;
use crate::shopee;
use crate::short_io;
use crate::work_context;

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
            | ChannelKind::Pinterest
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

/// A link was created: the sales carrying its marking join its channel.
pub struct AffiliateLinkCreated;

pub struct AffiliateLinksSection {
    links: Vec<AffiliateLink>,
    programs: Vec<Program>,
    promoted: Vec<PromotedProduct>,
    channels: Vec<PromotionChannel>,
    /// The Program the form creates a link for.
    creating: Option<RecordId>,
    /// "Nova afiliação" is open: it fills the product and the link, then
    /// becomes "Criar link" for the Program it adds.
    adding: bool,
    platform: PlatformSelect,
    name: Entity<InputState>,
    product: Entity<InputState>,
    pasted: Entity<InputState>,
    /// The Amazon tracking id of the channel.
    tracking_id: Entity<InputState>,
    channel: Option<RecordId>,
    /// What the last link created breaks or may break.
    warnings: Vec<String>,
    /// What the last Short Link breaks or may break.
    short_warnings: Vec<String>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    form: Option<Outcome>,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<AffiliateLinkCreated> for AffiliateLinksSection {}

impl EventEmitter<StartPost> for AffiliateLinksSection {}

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
            adding: false,
            platform: platform_select(window, cx),
            name: input("Igual ao produto", window, cx),
            product: input("Como a plataforma chama o produto", window, cx),
            pasted: input("https://go.hotmart.com/…", window, cx),
            tracking_id: input("mascate-tg-20", window, cx),
            channel: None,
            warnings: Vec::new(),
            short_warnings: Vec::new(),
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
        self.adding = false;
        self.channel = None;
        self.warnings.clear();
        self.form = None;
        cx.notify();
    }

    /// Opens "Nova afiliação" with empty fields.
    fn open_adding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for field in [&self.name, &self.product, &self.pasted] {
            field.update(cx, |field, cx| field.set_value("", window, cx));
        }
        self.adding = true;
        self.creating = None;
        self.warnings.clear();
        self.form = None;
        cx.notify();
    }

    /// Adds the Program the owner typed, then turns the form into "Criar
    /// link" for it with the product and the link kept, where the app
    /// tags links of its Platform.
    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(platform) = form_platform(&self.platform, cx) else {
            self.form = Some(Outcome::Failed("Escolha a plataforma.".into()));
            cx.notify();
            return;
        };
        let new = NewProgram {
            platform,
            name: self.name.read(cx).value().to_string(),
            product: self.product.read(cx).value().to_string(),
        };
        self.form = None;
        self.run(
            window,
            cx,
            move |affiliate| async move { affiliate.add_program(&new).await },
            move |this, program: Program, _, cx| {
                this.adding = false;
                if tagger(program.platform, cx).is_some() {
                    this.creating = Some(program.id);
                    this.channel = None;
                    this.outcome = Some(Outcome::Done(
                        format!(
                            "Afiliação {} cadastrada. Escolha o canal para criar o link.",
                            program.name
                        )
                        .into(),
                    ));
                } else {
                    this.outcome = Some(Outcome::Done(
                        format!(
                            "Afiliação {} cadastrada. O app ainda não marca links da {}.",
                            program.name,
                            program.platform.name()
                        )
                        .into(),
                    ));
                }
            },
        );
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

    /// Follows a new Work Context: "Criar link" for a Program of a Platform
    /// it does not show closes without creating anything.
    pub fn follow_context(&mut self, cx: &mut Context<Self>) {
        let context = preferences::work_context(cx);
        let programs = &self.programs;
        work_context::close_outside_platform(&mut self.creating, context, |creating| {
            programs
                .iter()
                .find(|program| program.id == *creating)
                .map(|program| program.platform)
        });
        if self.creating.is_none() {
            self.warnings.clear();
        }
        cx.notify();
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
                if !created.existed {
                    cx.emit(AffiliateLinkCreated);
                }
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

    /// Gives the link its Short Link through Short.io, after its Program's
    /// rules.
    fn shorten(&mut self, link: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(shortener) = short_io::adapter(cx) else {
            return;
        };
        self.outcome = None;
        self.short_warnings.clear();
        self.run(
            window,
            cx,
            move |affiliate| async move { Ok(affiliate.shorten(link, shortener.as_ref()).await) },
            |this, shortened, _, cx| match shortened {
                Ok(shortened) => {
                    this.short_warnings = shortened.warnings.iter().map(warning_text).collect();
                    this.outcome = Some(Outcome::Done(short_io::shortened_text(&shortened).into()));
                }
                Err(error) => {
                    if matches!(error, AffiliateError::Platform(PlatformError::Expired)) {
                        short_io::expired(cx);
                    }
                    this.outcome = Some(Outcome::Failed(short_io::shorten_failure(&error).into()));
                }
            },
        );
    }

    fn copy(&mut self, product: &str, link: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(link));
        self.outcome = Some(Outcome::Done(format!("Link de {product} copiado.").into()));
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
        let (product, tagged) = (link.product.clone(), link.tagged.clone());
        let (id, platform) = (link.id, link.program.platform);
        let short_action = match &link.short {
            Some(short) => {
                let (product, short) = (link.product.clone(), short.clone());
                Button::new(("copy-short-link", index))
                    .label("Copiar curto")
                    .outline()
                    .small()
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.copy(&product, short.clone(), cx)),
                    )
            }
            None => Button::new(("shorten-link", index))
                .label("Encurtar no Short.io")
                .outline()
                .small()
                .disabled(self.busy)
                .on_click(cx.listener(move |this, _, window, cx| this.shorten(id, window, cx))),
        };
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
                    )
                    .children(link.short.as_ref().map(|short| {
                        div()
                            .text_xs()
                            .text_color(t.accent_text)
                            .truncate()
                            .child(format!("curto: {short}"))
                    })),
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
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.copy(&product, tagged.clone(), cx)),
                    ),
            )
            .child(short_action)
            .child(
                Button::new(("post-link", index))
                    .label("Criar post")
                    .outline()
                    .small()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(StartPost {
                            link: id,
                            platform,
                            found: None,
                            image: None,
                        })
                    })),
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
                            // Only Shopee is asked for the link; the others
                            // are tagged in the app.
                            .label(if shopee {
                                "Criar link na Shopee"
                            } else {
                                "Criar link"
                            })
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

    fn render_adding(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .child(div().text_sm().child(label))
                .child(div().w(px(420.)).child(Input::new(input).small()))
        };
        let platform = form_platform(&self.platform, cx);
        let single = platform.is_some_and(|platform| platform.own_program().is_some());
        let platform_name = platform.map_or("plataforma", AffiliatePlatform::name);
        v_flex()
            .gap_3()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.border)
            .bg(t.raised)
            .child(div().font_medium().child("Nova afiliação"))
            .child(div().text_sm().text_color(t.text2).child(
                "Cadastre a afiliação que você acabou de pegar, como aparece na página de                  afiliação, para criar o link e divulgar antes da primeira venda. Quando a                  primeira venda chegar, o app pergunta se é desta afiliação.",
            ))
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child("Plataforma"))
                    .child(platform_field(&self.platform, cx)),
            )
            .child(if single {
                div()
                    .text_xs()
                    .text_color(t.text2)
                    .child(format!(
                        "Na {platform_name} a afiliação é uma só para todos os produtos: o                          link entra nela."
                    ))
                    .into_any_element()
            } else {
                field("Nome da afiliação", &self.name).into_any_element()
            })
            .child(field("Produto", &self.product))
            .child(field("Link de afiliado", &self.pasted).child(
                div().text_xs().text_color(t.text2).child(format!(
                    "O HotLink ou o link que a {platform_name} dá na página de afiliação.                      Pode deixar para o próximo passo."
                )),
            ))
            .children(self.form.as_ref().map(|outcome| notice(outcome, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("add-program")
                            .label("Continuar para o link")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.add(window, cx))),
                    )
                    .child(
                        Button::new("cancel-program")
                            .label("Cancelar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.adding = false;
                                this.form = None;
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
                            .child(div().text_xs().text_color(t.text2).child(format!(
                                "{}{}",
                                waiting_prefix(program.provisional),
                                match count {
                                    0 => "nenhum link ainda".to_owned(),
                                    1 => "1 link".to_owned(),
                                    count => format!("{count} links"),
                                }
                            ))),
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

/// What a provisional Program's line starts with.
fn waiting_prefix(provisional: bool) -> &'static str {
    if provisional {
        "aguardando a primeira venda · "
    } else {
        ""
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
        let context = preferences::work_context(cx);
        let (taggable, untagged): (Vec<Program>, Vec<Program>) = self
            .programs
            .clone()
            .into_iter()
            .filter(|program| context.shows_platform(program.platform))
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
                    .filter(|tagger| context.shows_platform(tagger.platform()))
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
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_4()
                    .items_center()
                    .child(kit::section_heading("Links de afiliado"))
                    .child(
                        Button::new("open-add-program")
                            .label("Nova afiliação")
                            .outline()
                            .small()
                            .disabled(self.busy || self.adding)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.open_adding(window, cx)),
                            ),
                    ),
            )
            .child(div().text_sm().text_color(t.text2).child(
                "Cole o link de afiliado de um produto e escolha o canal: o app acrescenta a \
                 marcação do canal, que volta em cada venda e liga a comissão a ele. Na Shopee, \
                 a própria Shopee devolve o link curto já marcado. \"Encurtar\" cria um link curto \
                 no seu domínio do Short.io, que conta os cliques; os termos da Amazon, da Shopee \
                 e do Mercado Livre proíbem encurtar.",
            ))
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .children(
                self.short_warnings
                    .iter()
                    .map(|warning| kit::info_notice(warning.clone(), cx).into_any_element()),
            )
            .when(self.adding, |section| section.child(self.render_adding(cx)))
            .when(rows.is_empty(), |section| {
                section.child(div().text_sm().text_color(t.text2).child(
                    "Nenhum programa da Hotmart, Kiwify, Monetizze, Eduzz ou Amazon ainda: eles \
                     aparecem aqui com a primeira comissão, ou antes dela em \"Nova afiliação\".",
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
