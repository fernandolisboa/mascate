//! Posts de oferta on Divulgação (#102): an Offer Post from an Affiliate
//! Link, its text born from the channel's template, edited with a preview
//! and the problems beside it, then copied with a click. The app gathers
//! the facts the Marketing drafts from (ADR 0031, decision 5): the link and
//! its Program's rules from the Affiliate, the Product's name and photos
//! from the Catalog, the price typed or the one Shopee's search gave. An
//! Amazon price waits for its Restricted Feature; without it the post goes
//! without a price. A post on a Telegram channel also goes out through the
//! bot (#103), and one on a Pinterest channel as a Pin on its board (#155),
//! after the owner reads the final text, picture and channel and confirms;
//! one that may have gone out waits for the owner to look.
//! Confirmed for a time instead, it waits in the queue (#104, and on every
//! destination since #157), listed with what is late, unconfirmed, failed
//! or published today and with any destination at its limit.
//! Scheduling offers the dates of the coming Sales Events (#105).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{NaiveDate, NaiveTime, Utc};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClipboardItem, Entity, Global, Image, ImageFormat, ObjectFit, Subscription,
    Window, div, img, px,
};
use mascate_affiliate::{
    Affiliate, AffiliateError, AffiliateLink, Practice, Program, PromotionRules, RuleWarning,
    ShownPrice, Standing,
};
use mascate_catalog::Catalog;
use mascate_commerce::is_picture;
use mascate_kernel::{Currency, Money, PlatformError, RecordId, Timestamp, parse_amount};
use mascate_marketing::{
    ChannelKind, Contact, DestinationLimit, INSTAGRAM_POSTS_PER_DAY, LATE_AFTER_MINUTES,
    OfferFacts, OfferPost, OfferPosts, OfferPrice, PINTEREST_POSTS_PER_DAY, PRICE_KEEPS_HOURS,
    PostDraft, PostError, PostImage, PostProblem, PostQueue, PostStatus, PostTime,
    PromotionChannel, PublishFailure, PublishRequest, UpcomingEvent, check_post, post_limits,
};
use mascate_platform::Flag;

use crate::affiliate::{affiliate, failure};
use crate::affiliate_links::{promotion, warning_text};
use crate::appearance::look;
use crate::catalog::{self, day_and_time};
use crate::forms::{Outcome, day_text, input, notice};
use crate::kit;
use crate::post_destinations;
use crate::post_queue::{post_time, typed_moment};
use crate::promotion_channels::{ChannelChanges, promotion_channels};
use crate::sales::table_row;
use crate::sales_events::{event_date_text, sales_events};

/// The app's Offer Posts; absent when the database did not open.
pub struct AppOfferPosts(pub Arc<OfferPosts>);

impl Global for AppOfferPosts {}

pub(crate) fn offer_posts(cx: &App) -> Option<Arc<OfferPosts>> {
    cx.try_global::<AppOfferPosts>().map(|app| app.0.clone())
}

/// Counts the changes to the posts, copies and the queue's turns
/// included, so the disclosure Reminder and the lists are read again.
#[derive(Default)]
pub struct PostChanges(pub u64);

impl Global for PostChanges {}

/// Asks Divulgação to start a post from an Affiliate Link.
pub struct StartPost {
    pub link: RecordId,
    /// The price a Platform gave, as Shopee's search does.
    pub found: Option<OfferPrice>,
    /// The Platform's picture of the product.
    pub image: Option<String>,
}

/// GPUI hands an image on the clipboard to other apps on Windows and
/// macOS; on Linux, X11 and Wayland alike, it offers them only text, so
/// the photo opens in its folder instead.
const IMAGE_ON_CLIPBOARD: bool = cfg!(any(target_os = "windows", target_os = "macos"));

/// A post waiting for the owner to confirm it goes out, now or at a time.
struct Confirming {
    request: PublishRequest,
    /// Set when it is to be scheduled.
    schedule: Option<ScheduleChoice>,
}

#[derive(Clone)]
struct ScheduleChoice {
    channel: RecordId,
    /// The channel's next free posting time; `None` without posting times.
    next_free: Option<Timestamp>,
    /// The owner types a day and time instead.
    picking: bool,
    /// The Sales Events coming soon, whose dates the owner can pick.
    events: Vec<UpcomingEvent>,
}

/// How far ahead the Sales Events offered when scheduling are.
const EVENTS_OFFERED_DAYS: u32 = 60;

/// How many Sales Events scheduling offers at most.
const EVENTS_OFFERED: usize = 4;

/// The time a post scheduled for an event's day starts at, when its
/// channel has no posting times.
const EVENT_POST_TIME: NaiveTime = NaiveTime::from_hms_opt(8, 0, 0).expect("a valid time");

/// What a post knows of its offer before the price: the link, its
/// Program, and the Product's name and photos when the link's product is
/// linked to one.
#[derive(Clone)]
pub(crate) struct Offer {
    pub link: AffiliateLink,
    pub program: Program,
    pub product: String,
    pub photos: Vec<PathBuf>,
}

/// Reads the offer of `link`.
pub(crate) async fn read_offer(
    affiliate: &Affiliate,
    catalog: Option<&Catalog>,
    link: AffiliateLink,
) -> Result<Offer, AffiliateError> {
    let program = affiliate.program(link.program.id).await?;
    let linked = affiliate.linked_product(program.id, &link.product).await?;
    let mut product = link.product.clone();
    let mut photos = Vec::new();
    if let (Some(id), Some(catalog)) = (linked, catalog) {
        // A Product deleted since it was linked leaves the Platform's name.
        if let Ok(found) = catalog.product(id).await {
            product = found.name;
            photos = catalog
                .files(id)
                .await
                .map(|files| {
                    files
                        .into_iter()
                        .map(|file| file.path)
                        .filter(|path| is_picture(path))
                        .collect()
                })
                .unwrap_or_default();
        }
    }
    Ok(Offer {
        link,
        program,
        product,
        photos,
    })
}

/// The contacts a Program's rules keep out of a post: an e-mail where
/// e-mail promotion is not allowed, a phone where direct messages are not.
pub(crate) fn forbidden_contacts(rules: &PromotionRules) -> Vec<Contact> {
    [
        (Practice::Email, Contact::Email),
        (Practice::DirectMessage, Contact::Phone),
    ]
    .into_iter()
    .filter(|(practice, _)| rules.standing(*practice) != Standing::Allowed)
    .map(|(_, contact)| contact)
    .collect()
}

/// A price for a post, and whether the owner typed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GivenPrice {
    pub price: OfferPrice,
    pub typed: bool,
}

/// A drafted post and what its Program said of it.
pub(crate) struct Gathered {
    pub draft: PostDraft,
    /// The Restricted Features the post uses, turned on by the owner.
    pub restricted: Vec<RuleWarning>,
    /// The Restricted Feature that took the price off, when it is off.
    pub refused: Option<Flag>,
}

/// Drafts the post of `offer` for `channel` with `price` and `image`. A
/// price a Restricted Feature that is off refuses, as Amazon's typed one,
/// leaves the post without a price.
pub(crate) async fn gather(
    affiliate: &Affiliate,
    posts: &OfferPosts,
    offer: &Offer,
    channel: &PromotionChannel,
    price: Option<GivenPrice>,
    image: Option<PostImage>,
) -> Result<Gathered, String> {
    let mut asked = promotion(channel);
    asked.price = price.map(|given| ShownPrice {
        typed: given.typed,
        seen_at: Some(given.price.seen_at),
    });
    let (price, warnings, refused) = match affiliate.ensure_allowed(offer.program.id, &asked).await
    {
        Ok(warnings) => (price, warnings, None),
        Err(AffiliateError::Restricted(flag)) if asked.price.is_some() => {
            asked.price = None;
            let warnings = affiliate
                .ensure_allowed(offer.program.id, &asked)
                .await
                .map_err(|error| failure(&error))?;
            (None, warnings, Some(flag))
        }
        Err(error) => return Err(failure(&error)),
    };
    let facts = OfferFacts {
        link_id: offer.link.id,
        link: offer.link.tagged.clone(),
        product: offer.product.clone(),
        platform: offer.program.platform.name().to_owned(),
        price: price.map(|given| given.price),
        image,
        price_needs_time: offer.program.rules.price_needs_time,
        forbidden_contacts: forbidden_contacts(&offer.program.rules),
    };
    let draft = posts
        .draft(&facts, channel)
        .await
        .map_err(|error| post_failure(&error))?;
    Ok(Gathered {
        draft,
        restricted: warnings
            .into_iter()
            .filter(|warning| matches!(warning, RuleWarning::Restricted(_)))
            .collect(),
        refused,
    })
}

pub(crate) fn post_failure(error: &PostError) -> String {
    match error {
        PostError::Blocked(problems) => problems
            .iter()
            .map(problem_text)
            .collect::<Vec<_>>()
            .join(" "),
        PostError::UnknownVariable(variable) => format!(
            "O modelo usa {variable}, que o app não preenche. Use {{produto}}, {{preco}}, {{de}}, \
             {{desconto}}, {{link}} e {{hora_do_preco}}."
        ),
        PostError::TemplateWithoutLink => {
            "O modelo precisa de {link}: o post sem o link de afiliado não rende comissão.".into()
        }
        PostError::NoSuchChannel => "O canal deste post foi apagado em Divulgação.".into(),
        PostError::NotFound => "Esse post mudou; a lista foi atualizada.".into(),
        PostError::NoDestination => "Só posts de um canal do Telegram, do Pinterest ou do \
                                     Instagram saem pelo app; para os outros canais, copie o \
                                     texto."
            .into(),
        PostError::NoChat => "Informe o chat do canal do Telegram em Canais de divulgação, em \
                              Editar, para publicar por ele."
            .into(),
        PostError::NoBoard => "Escolha a pasta do canal do Pinterest em Canais de divulgação, \
                               em Pasta, para publicar nele."
            .into(),
        PostError::WrongStatus(PostStatus::Sent) => "Esse post já foi publicado.".into(),
        PostError::WrongStatus(PostStatus::Sending) => "Esse post está saindo agora.".into(),
        PostError::WrongStatus(PostStatus::Unconfirmed) => "Esse post pode ter saído: confira o \
                                                            canal e use Marcar como publicado ou \
                                                            Publicar de novo."
            .into(),
        PostError::WrongStatus(PostStatus::PriceExpired) | PostError::OldPrice => format!(
            "O preço deste post foi visto há mais de {PRICE_KEEPS_HOURS} horas, mais do que o \
             programa deixa guardar. Abra o post, digite o preço de agora e gere o texto de novo."
        ),
        PostError::WrongStatus(
            PostStatus::Draft | PostStatus::Scheduled | PostStatus::Late | PostStatus::Failed,
        )
        | PostError::Changed => {
            "O post mudou depois que você conferiu, e nada saiu. Confira de novo e publique.".into()
        }
        PostError::PastTime => "Esse horário já passou: escolha um dia e hora à frente.".into(),
        PostError::TimeTaken => {
            "Outro post deste canal já está agendado nesse horário: escolha outro.".into()
        }
        PostError::NoPostingTimes => "Este canal não tem horários de publicação: escolha dia e \
                                      hora, ou defina os horários do canal em Canais de \
                                      divulgação."
            .into(),
        PostError::FateUnread(why) => format!("Não consegui conferir o post: {why}"),
        PostError::Unreadable(_) | PostError::Sql(_) => {
            format!("Não consegui ler ou gravar os posts no banco: {error}")
        }
    }
}

/// What happens to an Instagram post's text, said before it goes out.
const INSTAGRAM_CAPTION: &str = "O texto vira a legenda do post, uma vez, e o post fica na sua \
                                 conta: o app não apaga posts. No Instagram o link na legenda \
                                 não é clicável: quem vê precisa copiar e colar no navegador.";

/// A problem of the post, as the owner reads it.
pub(crate) fn problem_text(problem: &PostProblem) -> String {
    match problem {
        PostProblem::NoLink => {
            "O texto perdeu o link de afiliado; sem ele o post não salva.".into()
        }
        PostProblem::TooLong {
            units,
            limit,
            caption: true,
        } => format!(
            "Com foto, o texto vai como legenda, que aceita até {limit} caracteres, e ele tem \
             {units}. Encurte o texto ou tire a foto."
        ),
        PostProblem::TooLong { units, limit, .. } => format!(
            "Este canal aceita até {limit} caracteres, e o texto tem {units}. Encurte o texto."
        ),
        PostProblem::NoImage => {
            "Este canal só publica post com foto: escolha uma foto do produto.".into()
        }
        PostProblem::FolderPhoto => "O Instagram busca a foto num endereço da internet, e esta \
                                     está na pasta do produto: escolha a foto da plataforma \
                                     (Link da imagem) para publicar pelo app, ou copie o texto e \
                                     poste à mão com esta foto."
            .into(),
        PostProblem::TooManyHashtags { count, limit } => format!(
            "O texto tem {count} hashtags, e o Instagram aceita até {limit}, contando o #publi. \
             Tire algumas."
        ),
        PostProblem::TooManyMentions { count, limit } => format!(
            "O texto marca {count} perfis com @, e o Instagram aceita até {limit}. Tire algumas \
             marcações."
        ),
        PostProblem::NoDisclosure => "O texto ficou sem a identificação de publicidade (#publi). \
                                      Post de afiliado é publicidade, e o CONAR pede que isso \
                                      fique claro."
            .into(),
        PostProblem::NoPriceTime => "O preço ficou sem a data e a hora em que foi visto, que este \
                                     programa exige."
            .into(),
        PostProblem::NoPriceNotice => "O texto ficou sem o aviso de que preço e disponibilidade \
                                       podem mudar, que este programa exige."
            .into(),
        PostProblem::Contact(contact) => format!(
            "O texto tem {}, e este programa proíbe esse contato ou ainda não tem os termos lidos.",
            contact.name()
        ),
    }
}

/// An amount the owner typed, in reais; `Err` when it is not one.
fn typed_money(text: &str) -> Result<Option<Money>, ()> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    match parse_amount(text) {
        Some(amount) if !amount.is_sign_negative() => Ok(Some(Money::new(amount, Currency::Brl))),
        _ => Err(()),
    }
}

/// "149,90", as the owner types it.
fn money_text(money: Money) -> String {
    money.to_pt_br().trim_start_matches("R$ ").to_owned()
}

/// The picture a post goes with.
#[derive(Clone, PartialEq, Eq)]
enum Picture {
    None,
    File(PathBuf),
    /// The address typed or the Platform's.
    Url,
}

struct Editor {
    /// The saved post, once saved.
    post: Option<RecordId>,
    /// `None` when the post's link was deleted: its text still edits.
    offer: Option<Offer>,
    channel: Option<PromotionChannel>,
    facts: OfferFacts,
    price: Entity<InputState>,
    before: Entity<InputState>,
    /// The price a Platform gave, with when it was seen.
    found: Option<OfferPrice>,
    picture: Picture,
    image_url: Entity<InputState>,
    text: Entity<TextareaState>,
    restricted: Vec<RuleWarning>,
    refused: Option<Flag>,
    form: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl Editor {
    fn draft(&self, cx: &App) -> Option<PostDraft> {
        Some(PostDraft {
            channel: self.channel.as_ref()?.id,
            facts: self.facts.clone(),
            text: self.text.read(cx).value().to_string(),
        })
    }

    /// Everything to read beside the post: what the Marketing finds wrong
    /// for the channel, then what the Program's rules say of the text.
    fn problems(&self, cx: &App) -> (Vec<PostProblem>, Vec<String>) {
        let (Some(draft), Some(channel)) = (self.draft(cx), self.channel.as_ref()) else {
            return (Vec::new(), Vec::new());
        };
        let problems = check_post(&draft, post_limits(channel.kind));
        let mut rules: Vec<String> = Vec::new();
        if let Some(offer) = &self.offer {
            let mut asked = promotion(channel);
            asked.text = draft.text.clone();
            asked.price = self.facts.price.map(|price| ShownPrice {
                typed: self.found != Some(price),
                seen_at: Some(price.seen_at),
            });
            rules.extend(offer.program.rules.check(&asked).iter().map(warning_text));
        }
        rules.extend(self.restricted.iter().map(warning_text));
        (problems, rules)
    }
}

pub struct OfferPostsSection {
    posts: Vec<OfferPost>,
    queue: PostQueue,
    links: Vec<AffiliateLink>,
    channels: Vec<PromotionChannel>,
    /// "Novo post" lists the links to start from.
    picking: bool,
    editor: Option<Editor>,
    /// The post asking to confirm its deletion.
    deleting: Option<RecordId>,
    /// The post waiting for the owner to confirm it goes out.
    confirming: Option<Confirming>,
    /// The day and time typed to schedule a post.
    when: Entity<InputState>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl OfferPostsSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe_global_in::<ChannelChanges>(window, Self::refresh),
            cx.observe_global_in::<PostChanges>(window, Self::refresh),
        ];
        let mut section = Self {
            posts: Vec::new(),
            queue: PostQueue::default(),
            links: Vec::new(),
            channels: Vec::new(),
            picking: false,
            editor: None,
            deleting: None,
            confirming: None,
            when: input("16/10 19:30", window, cx),
            busy: false,
            reads: 0,
            outcome: None,
            _subscriptions: subscriptions,
        };
        section.refresh(window, cx);
        section
    }

    /// Reads the posts, the links and the channels again.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(posts), Some(affiliate), Some(channels)) =
            (offer_posts(cx), affiliate(cx), promotion_channels(cx))
        else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        let working = cx.background_executor().spawn(async move {
            let posts = async { Ok((posts.posts().await?, posts.queue().await?)) }
                .await
                .map_err(|error: PostError| post_failure(&error));
            let links = affiliate.links().await.map_err(|error| failure(&error));
            let channels = channels
                .channels()
                .await
                .map_err(|error| format!("Não consegui ler os canais de divulgação: {error}"));
            (posts, links, channels)
        });
        cx.spawn_in(window, async move |this, cx| {
            let (posts, links, channels) = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.reads != read {
                    return;
                }
                let read = (|| Ok::<_, String>((posts?, links?, channels?)))();
                match read {
                    Ok(((posts, queue), links, channels)) => {
                        this.posts = posts;
                        this.queue = queue;
                        this.links = links;
                        this.channels = channels;
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                }
                if let Some(editor) = this.editor.as_mut()
                    && let Some(channel) = &editor.channel
                {
                    editor.channel = this.channels.iter().find(|c| c.id == channel.id).cloned();
                }
                cx.notify();
            });
        })
        .detach();
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

    /// Opens the editor on a new post from the link `start` names, with the
    /// price and picture it brings.
    pub fn start(&mut self, start: &StartPost, window: &mut Window, cx: &mut Context<Self>) {
        self.picking = false;
        self.outcome = None;
        let (Some(affiliate), Some(channels)) = (affiliate(cx), promotion_channels(cx)) else {
            return;
        };
        let catalog = catalog::catalog(cx);
        let (link, found, image) = (start.link, start.found, start.image.clone());
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            let links = affiliate.links().await.map_err(|error| failure(&error))?;
            let link = links
                .into_iter()
                .find(|known| known.id == link)
                .ok_or_else(|| "Esse link de afiliado foi apagado.".to_owned())?;
            let channel = channels.channel(link.channel).await.map_err(|_| {
                "O canal deste link foi apagado em Divulgação: crie o link de novo num canal."
                    .to_owned()
            })?;
            let offer = read_offer(&affiliate, catalog.as_deref(), link)
                .await
                .map_err(|error| failure(&error))?;
            Ok::<_, String>((offer, channel))
        });
        cx.spawn_in(window, async move |this, cx| {
            let read = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match read {
                    Ok((offer, channel)) => {
                        let picture = if image.is_some() {
                            Picture::Url
                        } else {
                            offer
                                .photos
                                .first()
                                .cloned()
                                .map_or(Picture::None, Picture::File)
                        };
                        let facts = OfferFacts {
                            link_id: offer.link.id,
                            link: offer.link.tagged.clone(),
                            product: offer.product.clone(),
                            platform: offer.program.platform.name().to_owned(),
                            price: None,
                            image: None,
                            price_needs_time: offer.program.rules.price_needs_time,
                            forbidden_contacts: forbidden_contacts(&offer.program.rules),
                        };
                        let editor = this.editor_for(
                            None,
                            Some(offer),
                            Some(channel),
                            facts,
                            found,
                            picture,
                            image,
                            window,
                            cx,
                        );
                        this.editor = Some(editor);
                        this.generate(window, cx);
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(error.into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Opens a saved post in the editor, with its offer when the link is
    /// still there.
    fn open(&mut self, post: OfferPost, window: &mut Window, cx: &mut Context<Self>) {
        self.picking = false;
        self.outcome = None;
        let Some(affiliate) = affiliate(cx) else {
            return;
        };
        let catalog = catalog::catalog(cx);
        let link = self
            .links
            .iter()
            .find(|link| link.id == post.draft.facts.link_id)
            .cloned();
        let channel = self
            .channels
            .iter()
            .find(|channel| channel.id == post.draft.channel)
            .cloned();
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            match link {
                Some(link) => read_offer(&affiliate, catalog.as_deref(), link).await.ok(),
                None => None,
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let offer = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                let facts = post.draft.facts.clone();
                let (picture, url) = match &facts.image {
                    Some(PostImage::File(path)) => (Picture::File(path.clone()), None),
                    Some(PostImage::Url(url)) => (Picture::Url, Some(url.clone())),
                    None => (Picture::None, None),
                };
                let editor = this.editor_for(
                    Some(post.id),
                    offer,
                    channel,
                    facts,
                    None,
                    picture,
                    url,
                    window,
                    cx,
                );
                editor.text.update(cx, |field, cx| {
                    field.set_value(post.draft.text.clone(), window, cx)
                });
                this.editor = Some(editor);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    #[allow(clippy::too_many_arguments)]
    fn editor_for(
        &mut self,
        post: Option<RecordId>,
        offer: Option<Offer>,
        channel: Option<PromotionChannel>,
        facts: OfferFacts,
        found: Option<OfferPrice>,
        picture: Picture,
        image_url: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Editor {
        let price = input("149,90", window, cx);
        let before = input("Vazio sem desconto", window, cx);
        let shown = facts.price.or(found);
        if let Some(shown) = shown {
            price.update(cx, |field, cx| {
                field.set_value(money_text(shown.price), window, cx)
            });
            if let Some(was) = shown.before {
                before.update(cx, |field, cx| field.set_value(money_text(was), window, cx));
            }
        }
        let url = input("https://…", window, cx);
        if let Some(image_url) = image_url {
            url.update(cx, |field, cx| field.set_value(image_url, window, cx));
        }
        let text = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(8, 24)
                .placeholder("O texto do post")
        });
        let subscriptions = vec![
            cx.subscribe_in(&text, window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&url, window, |this, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.apply_picture(cx);
                }
            }),
        ];
        let mut editor = Editor {
            post,
            offer,
            channel,
            facts,
            price,
            before,
            found,
            picture,
            image_url: url,
            text,
            restricted: Vec::new(),
            refused: None,
            form: None,
            _subscriptions: subscriptions,
        };
        editor.facts.image = picture_of(&editor, cx);
        editor
    }

    fn pick_picture(&mut self, picture: Picture, cx: &mut Context<Self>) {
        if let Some(editor) = self.editor.as_mut() {
            editor.picture = picture;
        }
        self.apply_picture(cx);
    }

    /// Puts the picked picture in the post's facts: a photo counts against
    /// Telegram's caption, not its text.
    fn apply_picture(&mut self, cx: &mut Context<Self>) {
        if let Some(editor) = self.editor.as_mut() {
            editor.facts.image = picture_of(editor, cx);
        }
        cx.notify();
    }

    /// Drafts the text again from the price typed and the channel's
    /// template; the price counts as seen now, unless it is the one the
    /// Platform gave.
    fn generate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(affiliate), Some(posts)) = (affiliate(cx), offer_posts(cx)) else {
            return;
        };
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let (Some(offer), Some(channel)) = (editor.offer.clone(), editor.channel.clone()) else {
            editor.form = Some(Outcome::Failed(
                "O link ou o canal deste post foi apagado; o texto ainda pode ser editado.".into(),
            ));
            cx.notify();
            return;
        };
        let typed = (
            typed_money(&editor.price.read(cx).value()),
            typed_money(&editor.before.read(cx).value()),
        );
        let price = match typed {
            (Ok(price), Ok(before)) => price.map(|price| {
                let found = editor
                    .found
                    .filter(|found| found.price == price && found.before == before);
                GivenPrice {
                    price: OfferPrice {
                        price,
                        before,
                        seen_at: found.map_or_else(Utc::now, |found| found.seen_at),
                    },
                    typed: found.is_none(),
                }
            }),
            _ => {
                editor.form = Some(Outcome::Failed(
                    "Digite o preço em reais, como 149,90.".into(),
                ));
                cx.notify();
                return;
            }
        };
        let image = picture_of(editor, cx);
        editor.form = None;
        self.busy = true;
        let working = cx
            .background_executor()
            .spawn(async move { gather(&affiliate, &posts, &offer, &channel, price, image).await });
        cx.spawn_in(window, async move |this, cx| {
            let gathered = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                let Some(editor) = this.editor.as_mut() else {
                    return;
                };
                match gathered {
                    Ok(gathered) => {
                        editor.text.update(cx, |field, cx| {
                            field.set_value(gathered.draft.text.clone(), window, cx)
                        });
                        editor.facts = gathered.draft.facts;
                        editor.restricted = gathered.restricted;
                        editor.refused = gathered.refused;
                    }
                    Err(error) => editor.form = Some(Outcome::Failed(error.into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Saves the post, then hands `done` the saved post.
    fn save_then<D>(&mut self, window: &mut Window, cx: &mut Context<Self>, done: D)
    where
        D: FnOnce(&mut Self, OfferPost, &mut Window, &mut Context<Self>) + 'static,
    {
        let Some(posts) = offer_posts(cx) else {
            return;
        };
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let Some(draft) = editor.draft(cx) else {
            editor.form = Some(Outcome::Failed(
                "O canal deste post foi apagado em Divulgação.".into(),
            ));
            cx.notify();
            return;
        };
        if matches!(editor.picture, Picture::Url) && editor.facts.image.is_none() {
            editor.form = Some(Outcome::Failed(
                "O link da imagem começa com https://.".into(),
            ));
            cx.notify();
            return;
        }
        let saved = editor.post;
        editor.form = None;
        self.busy = true;
        let working = cx.background_executor().spawn(async move {
            match saved {
                Some(id) => posts.update(id, &draft).await,
                None => posts.create(&draft).await,
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let saved = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match saved {
                    Ok(post) => {
                        if let Some(editor) = this.editor.as_mut() {
                            editor.post = Some(post.id);
                        }
                        done(this, post, window, cx);
                        this.refresh(window, cx);
                    }
                    Err(error) => {
                        if let Some(editor) = this.editor.as_mut() {
                            editor.form = Some(Outcome::Failed(post_failure(&error).into()));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_then(window, cx, |this, _, _, _| {
            if let Some(editor) = this.editor.as_mut() {
                editor.form = Some(Outcome::Done("Rascunho salvo.".into()));
            }
        });
    }

    /// Saves the editor's post, then copies its text.
    fn copy_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_then(window, cx, |this, post, window, cx| {
            this.copy_text(post, window, cx)
        });
    }

    /// Copies the post's text and notes it went out.
    fn copy_text(&mut self, post: OfferPost, window: &mut Window, cx: &mut Context<Self>) {
        let Some(posts) = offer_posts(cx) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(post.draft.text.clone()));
        let message = format!(
            "Texto do post de {} copiado: cole no {}.",
            post.draft.facts.product,
            self.channel_name(post.draft.channel)
        );
        match self
            .editor
            .as_mut()
            .filter(|editor| editor.post == Some(post.id))
        {
            Some(editor) => editor.form = Some(Outcome::Done(message.into())),
            None => self.outcome = Some(Outcome::Done(message.into())),
        }
        let id = post.id;
        let working = cx
            .background_executor()
            .spawn(async move { posts.copied(id).await });
        cx.spawn_in(window, async move |this, cx| {
            let copied = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if let Err(error) = copied {
                    this.outcome = Some(Outcome::Failed(post_failure(&error).into()));
                }
                cx.update_default_global::<PostChanges, _>(|changes, _| changes.0 += 1);
                this.refresh(window, cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// Saves the editor's post, then asks to confirm it goes out now, or
    /// with `scheduling` at a time.
    fn confirm_editing(&mut self, scheduling: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.save_then(window, cx, move |this, post, window, cx| {
            this.prepare(post.id, scheduling, window, cx)
        });
    }

    /// Checks the post `id` for its channel and shows what will go out,
    /// waiting for the owner to confirm; with `scheduling`, with the
    /// channel's next free time to confirm it for.
    fn prepare(
        &mut self,
        id: RecordId,
        scheduling: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(posts) = offer_posts(cx) else {
            return;
        };
        self.outcome = None;
        self.busy = true;
        let calendar = sales_events(cx);
        let working = cx.background_executor().spawn(async move {
            let request = posts.prepare(id).await?;
            let schedule = if scheduling {
                let channel = posts.post(id).await?.draft.channel;
                let next_free = posts.next_free(channel, Some(id)).await?;
                let mut events = match calendar {
                    // The dates only help to pick: an unreadable calendar
                    // never stops the scheduling.
                    Some(calendar) => calendar
                        .upcoming(Utc::now(), EVENTS_OFFERED_DAYS)
                        .await
                        .unwrap_or_else(|error| {
                            eprintln!("could not read the Sales Events: {error}");
                            Vec::new()
                        }),
                    None => Vec::new(),
                };
                events.truncate(EVENTS_OFFERED);
                Some(ScheduleChoice {
                    channel,
                    next_free,
                    picking: next_free.is_none(),
                    events,
                })
            } else {
                None
            };
            Ok::<_, PostError>(Confirming { request, schedule })
        });
        cx.spawn_in(window, async move |this, cx| {
            let prepared = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match prepared {
                    Ok(confirming) => {
                        this.when
                            .update(cx, |field, cx| field.set_value("", window, cx));
                        this.confirming = Some(confirming);
                    }
                    Err(error) => this.outcome = Some(Outcome::Failed(post_failure(&error).into())),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The owner confirmed: the post goes to its destination, once.
    fn publish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(kind) = self
            .confirming
            .as_ref()
            .map(|confirming| confirming.request.kind())
        else {
            return;
        };
        let (Some(posts), Some(destination)) =
            (offer_posts(cx), post_destinations::adapter(kind, cx))
        else {
            return;
        };
        let Some(Confirming { request, .. }) = self.confirming.take() else {
            return;
        };
        let channel = request.channel().to_owned();
        self.outcome = None;
        self.busy = true;
        let working = cx
            .background_executor()
            .spawn(async move { posts.publish(destination.as_ref(), request.confirm()).await });
        cx.spawn_in(window, async move |this, cx| {
            let published = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                this.outcome = Some(match published {
                    Ok(post) => {
                        if this
                            .editor
                            .as_ref()
                            .is_some_and(|editor| editor.post == Some(post.id))
                            && !post.status.editable()
                        {
                            this.editor = None;
                        }
                        match (&post.status, &post.failure) {
                            (PostStatus::Sent, _) => {
                                Outcome::Done(format!("Post publicado em {channel}.").into())
                            }
                            (_, Some(failure)) => {
                                if *failure == PublishFailure::Expired {
                                    post_destinations::expired(kind, cx);
                                }
                                Outcome::Failed(
                                    post_destinations::publish_failure(kind, failure).into(),
                                )
                            }
                            (_, None) => Outcome::Failed("O post não saiu.".into()),
                        }
                    }
                    Err(error) => Outcome::Failed(post_failure(&error).into()),
                });
                cx.update_default_global::<PostChanges, _>(|changes, _| changes.0 += 1);
                this.refresh(window, cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// The owner found the unconfirmed post `id` in the channel.
    fn mark_published(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(posts) = offer_posts(cx) else {
            return;
        };
        self.busy = true;
        let working = cx
            .background_executor()
            .spawn(async move { posts.mark_published(id).await });
        cx.spawn_in(window, async move |this, cx| {
            let marked = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                this.outcome = Some(match marked {
                    Ok(_) => Outcome::Done("Post marcado como publicado.".into()),
                    Err(error) => Outcome::Failed(post_failure(&error).into()),
                });
                cx.update_default_global::<PostChanges, _>(|changes, _| changes.0 += 1);
                this.refresh(window, cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// Asks the destination what became of the unconfirmed post `id`.
    fn check_fate(
        &mut self,
        id: RecordId,
        kind: ChannelKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(posts), Some(destination)) =
            (offer_posts(cx), post_destinations::adapter(kind, cx))
        else {
            return;
        };
        self.busy = true;
        self.outcome = None;
        let working = cx
            .background_executor()
            .spawn(async move { posts.check_fate(destination.as_ref(), id).await });
        cx.spawn_in(window, async move |this, cx| {
            let checked = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                let name = post_destinations::destination_name(kind).unwrap_or("canal");
                this.outcome = Some(match checked {
                    Ok(post) => match post.status {
                        PostStatus::Sent => {
                            Outcome::Done(format!("O {name} confirmou: o post saiu.").into())
                        }
                        PostStatus::Failed => Outcome::Done(
                            format!(
                                "O {name} confirmou que o post não saiu. Publique de novo quando \
                                 quiser."
                            )
                            .into(),
                        ),
                        _ => Outcome::Failed(
                            format!(
                                "O {name} ainda não sabe dizer se o post saiu. Confira o perfil \
                                 ou tente de novo daqui a pouco."
                            )
                            .into(),
                        ),
                    },
                    Err(PostError::FateUnread(error)) => {
                        if error == PlatformError::Expired {
                            post_destinations::expired(kind, cx);
                        }
                        Outcome::Failed(post_destinations::fate_failure(kind, &error).into())
                    }
                    Err(error) => Outcome::Failed(post_failure(&error).into()),
                });
                cx.update_default_global::<PostChanges, _>(|changes, _| changes.0 += 1);
                this.refresh(window, cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// The owner confirmed the post for the time picked: it waits in the
    /// queue. A time the queue refuses keeps the confirmation open to pick
    /// another.
    fn schedule(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(posts) = offer_posts(cx) else {
            return;
        };
        let Some(Confirming {
            request,
            schedule: Some(choice),
        }) = self.confirming.as_ref()
        else {
            return;
        };
        let when = if choice.picking {
            match typed_moment(&self.when.read(cx).value(), Utc::now()) {
                Some(at) => PostTime::At(at),
                None => {
                    self.outcome = Some(Outcome::Failed(
                        "Digite o dia e a hora como 16/10 19:30, no horário de Brasília.".into(),
                    ));
                    cx.notify();
                    return;
                }
            }
        } else {
            PostTime::NextFree
        };
        let (confirmed, channel) = (request.clone().confirm(), request.channel().to_owned());
        self.outcome = None;
        self.busy = true;
        let working = cx
            .background_executor()
            .spawn(async move { posts.schedule(confirmed, when).await });
        cx.spawn_in(window, async move |this, cx| {
            let scheduled = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match scheduled {
                    Ok(post) => {
                        this.confirming = None;
                        if this
                            .editor
                            .as_ref()
                            .is_some_and(|editor| editor.post == Some(post.id))
                        {
                            this.editor = None;
                        }
                        let at = post.scheduled_at.map(post_time).unwrap_or_default();
                        this.outcome = Some(Outcome::Done(
                            format!(
                                "Post agendado para {at} em {channel}: sai sozinho na hora, com \
                                 o app aberto ou na bandeja."
                            )
                            .into(),
                        ));
                    }
                    Err(error) => {
                        let retry = matches!(
                            error,
                            PostError::PastTime
                                | PostError::TimeTaken
                                | PostError::NoPostingTimes
                                | PostError::OldPrice
                        );
                        if !retry {
                            this.confirming = None;
                        }
                        this.outcome = Some(Outcome::Failed(post_failure(&error).into()));
                    }
                }
                cx.update_default_global::<PostChanges, _>(|changes, _| changes.0 += 1);
                this.refresh(window, cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// Takes the post `id` out of the queue, as a draft.
    fn unschedule(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(posts) = offer_posts(cx) else {
            return;
        };
        self.busy = true;
        let working = cx
            .background_executor()
            .spawn(async move { posts.unschedule(id).await });
        cx.spawn_in(window, async move |this, cx| {
            let taken = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                this.outcome = Some(match taken {
                    Ok(_) => Outcome::Done(
                        "Post tirado da fila: ficou como rascunho e não sai sozinho.".into(),
                    ),
                    Err(error) => Outcome::Failed(post_failure(&error).into()),
                });
                cx.update_default_global::<PostChanges, _>(|changes, _| changes.0 += 1);
                this.refresh(window, cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// Pauses or resumes the queue.
    fn set_paused(&mut self, paused: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(posts) = offer_posts(cx) else {
            return;
        };
        self.busy = true;
        let working = cx
            .background_executor()
            .spawn(async move { posts.set_queue_paused(paused).await });
        cx.spawn_in(window, async move |this, cx| {
            let set = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                this.outcome = Some(match (set, paused) {
                    (Ok(()), true) => Outcome::Done(
                        "Publicação pausada: nenhum post sai sozinho até você retomar.".into(),
                    ),
                    (Ok(()), false) => Outcome::Done(
                        format!(
                            "Publicação retomada: os posts agendados saem na hora, e os \
                             atrasados mais de {LATE_AFTER_MINUTES} minutos esperam você."
                        )
                        .into(),
                    ),
                    (Err(error), _) => Outcome::Failed(post_failure(&error).into()),
                });
                this.refresh(window, cx);
            });
        })
        .detach();
        cx.notify();
    }

    /// What will go out, and when, for the owner to confirm.
    /// Fills the day and time to schedule with a Sales Event's `date`, at
    /// the channel's first posting time, for the owner to confirm or change.
    fn pick_event_day(&mut self, date: NaiveDate, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Confirming {
            schedule: Some(choice),
            ..
        }) = self.confirming.as_mut()
        else {
            return;
        };
        choice.picking = true;
        let time = self
            .channels
            .iter()
            .find(|channel| channel.id == choice.channel)
            .and_then(|channel| channel.posting_times.first().copied())
            .unwrap_or(EVENT_POST_TIME);
        let moment = format!("{} {}", day_text(date), time.format("%H:%M"));
        self.when
            .update(cx, |field, cx| field.set_value(moment, window, cx));
        cx.notify();
    }

    fn render_confirmation(&self, confirming: &Confirming, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let request = &confirming.request;
        let off = post_destinations::publishing_off(request.kind(), cx);
        let picture = match request.image() {
            Some(PostImage::File(path)) => format!(
                "Foto: {}",
                path.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
            Some(PostImage::Url(url)) => format!("Foto da plataforma: {url}"),
            None => "Sem foto: o Telegram mostra a prévia do link.".to_owned(),
        };
        let Some(choice) = confirming.schedule.as_ref() else {
            return self.render_publish_now(request, picture, off, cx);
        };
        let pick = |label: String, picking: bool, index: usize| {
            let chip = Button::new(("schedule-choice", index))
                .label(label)
                .small()
                .disabled(self.busy);
            if picking == choice.picking {
                chip.primary()
            } else {
                chip.ghost()
            }
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(Confirming {
                    schedule: Some(choice),
                    ..
                }) = this.confirming.as_mut()
                {
                    choice.picking = picking;
                }
                cx.notify();
            }))
        };
        let choices = h_flex()
            .gap_1()
            .flex_wrap()
            .children(choice.next_free.map(|at| {
                pick(
                    format!("Próximo horário livre: {}", post_time(at)),
                    false,
                    0,
                )
            }))
            .child(pick("Escolher dia e hora".into(), true, 1));
        let events = (!choice.events.is_empty()).then(|| {
            h_flex()
                .gap_1()
                .flex_wrap()
                .items_center()
                .child(div().text_xs().text_color(t.text2).child("Datas de venda:"))
                .children(choice.events.iter().enumerate().map(|(index, near)| {
                    let date = near.date;
                    Button::new(("schedule-event", index))
                        .label(format!(
                            "{}: {}",
                            near.event.name,
                            event_date_text(near.date)
                        ))
                        .ghost()
                        .small()
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.pick_event_day(date, window, cx)
                        }))
                }))
        });
        v_flex()
            .gap_2()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.raised)
            .child(
                div()
                    .font_medium()
                    .child(format!("Agendar {}?", where_it_goes(request))),
            )
            .children(
                pin_detail(request).map(|detail| div().text_xs().text_color(t.text2).child(detail)),
            )
            .child(choices)
            .children(events)
            .when(choice.next_free.is_none(), |panel| {
                panel.child(div().text_xs().text_color(t.text2).child(
                    "Este canal não tem horários de publicação: escolha dia e hora, ou defina os \
                     horários do canal em Canais de divulgação.",
                ))
            })
            .when(choice.picking, |panel| {
                panel.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().w(px(180.)).child(Input::new(&self.when).small()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.text2)
                                .child("Dia e hora no horário de Brasília."),
                        ),
                )
            })
            .child(div().text_xs().text_color(t.text2).child(picture))
            .child(
                div()
                    .p_3()
                    .rounded(t.radius_lg)
                    .border(t.border_width)
                    .border_color(t.border)
                    .bg(t.surface)
                    .text_sm()
                    .child(request.text().to_owned()),
            )
            .child(div().text_xs().text_color(t.text2).child(format!(
                "Na hora marcada o post sai sozinho, uma vez, do jeito que está acima, com o app \
                 aberto ou na bandeja. Se a hora passar de {LATE_AFTER_MINUTES} minutos sem ele \
                 sair, ele espera você. Editar o post tira ele da fila."
            )))
            .children(
                limit_note(request.kind())
                    .map(|note| div().text_xs().text_color(t.text2).child(note)),
            )
            .children(off.map(|why| {
                kit::info_notice(format!("{why} Enquanto isso, o post agendado não sai."), cx)
            }))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("confirm-schedule")
                            .label("Confirmar e agendar")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.schedule(window, cx))),
                    )
                    .child(cancel_confirmation(cx)),
            )
            .into_any_element()
    }

    /// "Publicar agora", for the owner to confirm.
    fn render_publish_now(
        &self,
        request: &PublishRequest,
        picture: String,
        off: Option<&'static str>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = look(cx).tokens;
        v_flex()
            .gap_2()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.raised)
            .child(
                div()
                    .font_medium()
                    .child(format!("Publicar agora {}?", where_it_goes(request))),
            )
            .children(
                pin_detail(request).map(|detail| div().text_xs().text_color(t.text2).child(detail)),
            )
            .child(div().text_xs().text_color(t.text2).child(picture))
            .child(
                div()
                    .p_3()
                    .rounded(t.radius_lg)
                    .border(t.border_width)
                    .border_color(t.border)
                    .bg(t.surface)
                    .text_sm()
                    .child(request.text().to_owned()),
            )
            .child(div().text_xs().text_color(t.text2).child(match request.kind() {
                ChannelKind::Pinterest => {
                    "O texto acima vira a descrição do Pin, uma vez, e o Pin fica na pasta: o app \
                     não apaga Pins. Com o acesso Trial do app, o Pin aparece só para você, até o \
                     Pinterest aprovar o acesso Standard."
                }
                ChannelKind::Instagram => INSTAGRAM_CAPTION,
                _ => {
                    "O post sai do jeito que está acima, uma vez, e fica no canal: o app não \
                     apaga posts do Telegram."
                }
            }))
            .children(off.map(|why| kit::error_notice(why, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("confirm-publish")
                            .label("Confirmar e publicar")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy || off.is_some())
                            .on_click(cx.listener(|this, _, window, cx| this.publish(window, cx))),
                    )
                    .child(cancel_confirmation(cx)),
            )
            .into_any_element()
    }

    /// The kind of the channel `channel` names, while it exists.
    fn kind_of(&self, channel: RecordId) -> Option<ChannelKind> {
        self.channels
            .iter()
            .find(|known| known.id == channel)
            .map(|known| known.kind)
    }

    /// Puts the photo on the clipboard, read off the UI thread.
    fn copy_image(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let Some(format) = image_format(&path) else {
            return;
        };
        let reading = cx
            .background_executor()
            .spawn(async move { std::fs::read(&path) });
        cx.spawn(async move |this, cx| {
            let read = reading.await;
            let _ = this.update(cx, |this, cx| {
                let outcome = match read {
                    Ok(bytes) => {
                        cx.write_to_clipboard(ClipboardItem::new_image(&Image::from_bytes(
                            format, bytes,
                        )));
                        Outcome::Done("Foto copiada: cole junto com o texto.".into())
                    }
                    Err(error) => {
                        Outcome::Failed(format!("Não consegui ler a foto: {error}").into())
                    }
                };
                if let Some(editor) = this.editor.as_mut() {
                    editor.form = Some(outcome);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn delete(&mut self, id: RecordId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(posts) = offer_posts(cx) else {
            return;
        };
        self.deleting = None;
        let working = cx
            .background_executor()
            .spawn(async move { posts.delete(id).await });
        cx.spawn_in(window, async move |this, cx| {
            let deleted = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.outcome = Some(match deleted {
                    Ok(()) => Outcome::Done("Post apagado.".into()),
                    Err(error) => Outcome::Failed(post_failure(&error).into()),
                });
                if this
                    .editor
                    .as_ref()
                    .is_some_and(|editor| editor.post == Some(id))
                {
                    this.editor = None;
                }
                this.refresh(window, cx);
            });
        })
        .detach();
    }

    fn render_post(&self, index: usize, post: &OfferPost, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let facts = &post.draft.facts;
        let copied = match post.copied_at {
            Some(at) => format!("copiado em {}", day_and_time(at)),
            None => "ainda não copiado".to_owned(),
        };
        let when = post.scheduled_at.map(post_time).unwrap_or_default();
        let status = match (post.status, &post.sent) {
            (PostStatus::Draft, _) => "rascunho".to_owned(),
            (PostStatus::Scheduled, _) => format!("agendado para {when}"),
            (PostStatus::Late, _) => format!("atrasado: era para {when}"),
            (PostStatus::PriceExpired, _) => format!("preço vencido: era para {when}"),
            (PostStatus::Sending, _) => "publicando…".to_owned(),
            (PostStatus::Sent, Some(sent)) => format!("publicado em {}", day_and_time(sent.at)),
            (PostStatus::Sent, None) => "publicado".to_owned(),
            (PostStatus::Unconfirmed, _) => "não confirmado".to_owned(),
            (PostStatus::Failed, _) => "não publicado".to_owned(),
        };
        let kind = self.kind_of(post.draft.channel);
        let publishes = kind.is_some_and(post_destinations::publishes);
        let (opened, copying, id) = (post.clone(), post.clone(), post.id);
        let again = matches!(post.status, PostStatus::Failed | PostStatus::Unconfirmed);
        let publish = (publishes && post.status.publishable()).then(|| {
            Button::new(("publish-post", index))
                .label(if again {
                    "Publicar de novo"
                } else {
                    "Publicar agora"
                })
                .outline()
                .small()
                .disabled(self.busy || self.confirming.is_some())
                .on_click(
                    cx.listener(move |this, _, window, cx| this.prepare(id, false, window, cx)),
                )
        });
        let schedule =
            (publishes && post.status.publishable() && post.status != PostStatus::Unconfirmed)
                .then(|| {
                    Button::new(("schedule-post", index))
                        .label(if post.status.queued() {
                            "Reagendar"
                        } else {
                            "Agendar"
                        })
                        .outline()
                        .small()
                        .disabled(self.busy || self.confirming.is_some())
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.prepare(id, true, window, cx)
                        }))
                });
        let unschedule = post.status.queued().then(|| {
            Button::new(("unschedule-post", index))
                .label("Tirar da fila")
                .ghost()
                .small()
                .disabled(self.busy)
                .on_click(cx.listener(move |this, _, window, cx| this.unschedule(id, window, cx)))
        });
        let waiting = match post.status {
            PostStatus::Late => Some(format!(
                "A hora passou de {LATE_AFTER_MINUTES} minutos sem o post sair (app fechado, \
                 publicação pausada, conexão fora do ar ou limite do destino), e oferta e preço \
                 podem ter mudado: publique agora ou reagende."
            )),
            PostStatus::PriceExpired => Some(format!(
                "O preço foi visto há mais de {PRICE_KEEPS_HOURS} horas, mais do que {} deixa \
                 guardar: abra o post, digite o preço de agora, gere o texto e agende de novo.",
                facts.platform
            )),
            _ => None,
        };
        let check = kind
            .filter(|&kind| {
                post.status == PostStatus::Unconfirmed && post_destinations::checks_fate(kind)
            })
            .map(|kind| {
                let name = post_destinations::destination_name(kind).unwrap_or("canal");
                Button::new(("check-post-fate", index))
                    .label(format!("Conferir no {name}"))
                    .primary()
                    .small()
                    .disabled(self.busy)
                    .on_click(
                        cx.listener(move |this, _, window, cx| {
                            this.check_fate(id, kind, window, cx)
                        }),
                    )
            });
        let mark = (post.status == PostStatus::Unconfirmed).then(|| {
            Button::new(("mark-post-published", index))
                .label("Marcar como publicado")
                .outline()
                .small()
                .disabled(self.busy)
                .on_click(
                    cx.listener(move |this, _, window, cx| this.mark_published(id, window, cx)),
                )
        });
        let open_sent = post
            .sent
            .as_ref()
            .and_then(|sent| sent.link.clone())
            .map(|link| {
                let name = kind
                    .and_then(post_destinations::destination_name)
                    .unwrap_or("canal");
                kit::external_link(
                    format!("open-sent-post-{index}"),
                    format!("Abrir no {name}"),
                    link,
                    cx,
                )
            });
        let failure = post
            .failure
            .as_ref()
            .filter(|_| matches!(post.status, PostStatus::Failed | PostStatus::Unconfirmed))
            .map(|failure| {
                let text = post_destinations::publish_failure(
                    kind.unwrap_or(ChannelKind::TelegramChannel),
                    failure,
                );
                if failure.may_have_gone_out() {
                    kit::info_notice(text, cx)
                } else {
                    kit::error_notice(text, cx)
                }
            });
        let actions = if self.deleting == Some(id) {
            h_flex()
                .gap_2()
                .items_center()
                .child(div().text_xs().child("Apagar o post?"))
                .child(
                    Button::new(("confirm-delete-post", index))
                        .label("Apagar")
                        .danger()
                        .small()
                        .on_click(
                            cx.listener(move |this, _, window, cx| this.delete(id, window, cx)),
                        ),
                )
                .child(
                    Button::new(("keep-post", index))
                        .label("Manter")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.deleting = None;
                            cx.notify();
                        })),
                )
        } else {
            h_flex()
                .gap_2()
                .flex_wrap()
                .children(open_sent)
                .children(check)
                .children(mark)
                .children(publish)
                .children(schedule)
                .when(post.status.editable(), |actions| {
                    actions.child(
                        Button::new(("open-post", index))
                            .label("Abrir")
                            .outline()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open(opened.clone(), window, cx)
                            })),
                    )
                })
                .child(
                    Button::new(("copy-post", index))
                        .label("Copiar texto")
                        .outline()
                        .small()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.copy_text(copying.clone(), window, cx)
                        })),
                )
                .children(unschedule)
                .child(
                    Button::new(("delete-post", index))
                        .label("Apagar")
                        .ghost()
                        .small()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.deleting = Some(id);
                            cx.notify();
                        })),
                )
        };
        table_row(&t)
            .flex_wrap()
            .child(kit::tag(facts.platform.clone(), t.text2, cx))
            .children(kind.map(|kind| kit::tag(kind.name(), t.text2, cx)))
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(240.))
                    .child(div().font_medium().truncate().child(format!(
                        "{} · {}",
                        facts.product,
                        self.channel_name(post.draft.channel)
                    )))
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text2)
                            .child(format!("{status} · {copied}")),
                    ),
            )
            .child(actions)
            .children(failure.map(|notice| div().w_full().child(notice)))
            .children(waiting.map(|text| div().w_full().child(kit::info_notice(text, cx))))
            .into_any_element()
    }

    fn render_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let rows: Vec<AnyElement> = self
            .links
            .iter()
            .enumerate()
            .map(|(index, link)| {
                let start = StartPost {
                    link: link.id,
                    found: None,
                    image: None,
                };
                table_row(&t)
                    .child(kit::tag(link.program.platform.name(), t.text2, cx))
                    .child(div().flex_1().min_w_0().truncate().child(format!(
                        "{} · {}",
                        link.product,
                        self.channel_name(link.channel)
                    )))
                    .child(
                        Button::new(("start-post", index))
                            .label("Criar post")
                            .primary()
                            .small()
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start(&start, window, cx)
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        v_flex()
            .gap_2()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.border)
            .bg(t.raised)
            .child(div().font_medium().child("De qual link é o post?"))
            .when(rows.is_empty(), |picker| {
                picker.child(div().text_sm().text_color(t.text2).child(
                    "Nenhum link de afiliado ainda: crie um em Afiliação › Links de afiliado ou em \
                     Ofertas › Buscar na Shopee.",
                ))
            })
            .children(rows)
            .child(
                h_flex().child(
                    Button::new("close-post-picker")
                        .label("Fechar")
                        .ghost()
                        .small()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.picking = false;
                            cx.notify();
                        })),
                ),
            )
            .into_any_element()
    }

    fn render_editor(&self, editor: &Editor, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let (problems, rules) = editor.problems(cx);
        let blocked = problems.iter().any(PostProblem::blocks);
        let unpublishable = problems.iter().any(PostProblem::blocks_publishing);
        let kind = editor.channel.as_ref().map(|channel| channel.kind);
        let publishes = kind.is_some_and(post_destinations::publishes);
        let channel = editor.channel.as_ref().map_or_else(
            || "canal apagado".to_owned(),
            |channel| channel.name.clone(),
        );
        let queued = self
            .posts
            .iter()
            .find(|post| Some(post.id) == editor.post && post.status.queued())
            .map(|post| {
                format!(
                    "Este post está na fila para {}. Salvar uma mudança tira ele da fila até você \
                     agendar de novo.",
                    post.scheduled_at.map(post_time).unwrap_or_default()
                )
            });
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .child(div().text_sm().child(label))
                .child(div().w(px(160.)).child(Input::new(input).small()))
        };
        let photos = editor
            .offer
            .as_ref()
            .map(|offer| offer.photos.clone())
            .unwrap_or_default();
        let mut pictures: Vec<(String, Picture)> = vec![("Sem foto".into(), Picture::None)];
        pictures.extend(photos.into_iter().map(|path| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            (name, Picture::File(path))
        }));
        pictures.push(("Link da imagem".into(), Picture::Url));
        let chips = pictures
            .into_iter()
            .enumerate()
            .map(|(index, (label, picture))| {
                let chip = Button::new(("post-picture", index)).label(label).small();
                if picture == editor.picture {
                    chip.primary()
                } else {
                    chip.ghost()
                }
                .on_click(cx.listener(move |this, _, _, cx| this.pick_picture(picture.clone(), cx)))
            });
        let price_note = match (&editor.refused, editor.facts.price_needs_time) {
            (Some(flag), _) => Some(flag.reason.to_owned()),
            (None, true) => Some(
                "Este programa pede a data e a hora do preço: o app põe a hora de quando você \
                 gera o texto, em horário de Brasília, e o aviso da política."
                    .to_owned(),
            ),
            (None, false) => None,
        };
        let image_action: Option<AnyElement> = match &editor.facts.image {
            Some(PostImage::File(path)) if IMAGE_ON_CLIPBOARD => {
                let path = path.clone();
                Some(
                    Button::new("copy-post-image")
                        .label("Copiar imagem")
                        .outline()
                        .small()
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.copy_image(path.clone(), cx)),
                        )
                        .into_any_element(),
                )
            }
            Some(PostImage::File(path)) => {
                let path = path.clone();
                Some(
                    Button::new("open-post-folder")
                        .label("Abrir pasta")
                        .tooltip("Arraste a foto da pasta para o post.")
                        .outline()
                        .small()
                        .on_click(move |_, _, cx| cx.reveal_path(&path))
                        .into_any_element(),
                )
            }
            Some(PostImage::Url(url)) => {
                let url = url.clone();
                Some(
                    Button::new("open-post-image")
                        .label("Abrir imagem")
                        .outline()
                        .small()
                        .on_click(move |_, _, cx| cx.open_url(&url))
                        .into_any_element(),
                )
            }
            None => None,
        };
        let text = editor.text.read(cx).value().to_string();
        let preview = v_flex()
            .gap_2()
            .w(px(360.))
            .flex_none()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.border)
            .bg(t.surface)
            .child(
                div()
                    .text_xs()
                    .text_color(t.text2)
                    .child(format!("Prévia · {channel}")),
            )
            .children(match &editor.facts.image {
                Some(PostImage::File(path)) => Some(
                    img(path.clone())
                        .w_full()
                        .h(px(200.))
                        .object_fit(ObjectFit::Contain)
                        .into_any_element(),
                ),
                Some(PostImage::Url(url)) => Some(
                    div()
                        .text_xs()
                        .text_color(t.text2)
                        .truncate()
                        .child(format!("Imagem da plataforma: {url}"))
                        .into_any_element(),
                ),
                None => None,
            })
            .child(div().text_sm().child(text));
        let editing = v_flex()
            .flex_1()
            .min_w(px(360.))
            .gap_3()
            .child(
                h_flex()
                    .gap_3()
                    .items_end()
                    .flex_wrap()
                    .child(field("Preço (R$)", &editor.price))
                    .child(field("De (R$)", &editor.before))
                    .child(
                        Button::new("generate-post")
                            .label("Gerar texto")
                            .outline()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy || editor.offer.is_none())
                            .on_click(cx.listener(|this, _, window, cx| this.generate(window, cx))),
                    ),
            )
            .children(price_note.map(|note| div().text_xs().text_color(t.text2).child(note)))
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child("Imagem"))
                    .child(h_flex().flex_wrap().gap_1().children(chips))
                    .when(editor.picture == Picture::Url, |images| {
                        images.child(
                            div()
                                .w(px(420.))
                                .child(Input::new(&editor.image_url).small()),
                        )
                    }),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child("Texto"))
                    .child(Textarea::new(&editor.text).small()),
            )
            .when(!problems.is_empty() || !rules.is_empty(), |editing| {
                editing.child(
                    v_flex()
                        .gap_1()
                        .children(problems.iter().map(|problem| {
                            let text = problem_text(problem);
                            if problem.blocks_publishing() {
                                kit::error_notice(text, cx).into_any_element()
                            } else {
                                kit::info_notice(text, cx).into_any_element()
                            }
                        }))
                        .children(
                            rules
                                .into_iter()
                                .map(|rule| kit::info_notice(rule, cx).into_any_element()),
                        ),
                )
            })
            .children(queued.map(|text| kit::info_notice(text, cx)))
            .when(kind == Some(ChannelKind::Instagram), |editor| {
                editor.child(kit::info_notice(INSTAGRAM_CAPTION, cx))
            })
            .children(editor.form.as_ref().map(|outcome| notice(outcome, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .children(publishes.then(|| {
                        Button::new("publish-post-now")
                            .label("Publicar agora")
                            .primary()
                            .small()
                            .disabled(self.busy || unpublishable || self.confirming.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirm_editing(false, window, cx)
                            }))
                    }))
                    .children(publishes.then(|| {
                        Button::new("schedule-post-editing")
                            .label("Agendar")
                            .outline()
                            .small()
                            .disabled(self.busy || blocked || self.confirming.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.confirm_editing(true, window, cx)
                            }))
                    }))
                    .child({
                        let copy = Button::new("copy-post-text")
                            .label("Copiar texto")
                            .small()
                            .disabled(self.busy || blocked)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.copy_editing(window, cx)),
                            );
                        if publishes {
                            copy.outline()
                        } else {
                            copy.primary()
                        }
                    })
                    .children(image_action)
                    .child(
                        Button::new("save-post")
                            .label("Salvar rascunho")
                            .outline()
                            .small()
                            .disabled(self.busy || blocked)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    )
                    .child(
                        Button::new("close-post")
                            .label("Fechar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.editor = None;
                                cx.notify();
                            })),
                    ),
            );
        v_flex()
            .gap_3()
            .p_4()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.frame)
            .bg(t.raised)
            .child(div().font_medium().child(format!(
                "{} · {} · {channel}",
                if editor.post.is_some() {
                    "Post"
                } else {
                    "Novo post"
                },
                editor.facts.product
            )))
            .child(
                h_flex()
                    .gap_4()
                    .items_start()
                    .flex_wrap()
                    .child(editing)
                    .child(preview),
            )
            .into_any_element()
    }
}

/// Closes the confirmation; nothing goes out.
fn cancel_confirmation(cx: &mut Context<OfferPostsSection>) -> Button {
    Button::new("cancel-publish")
        .label("Cancelar")
        .ghost()
        .small()
        .on_click(cx.listener(|this, _, _, cx| {
            this.confirming = None;
            cx.notify();
        }))
}

/// Where a confirmed post goes, as the confirmation names it: "em Canal
/// (@chat)", or the connected account on Instagram.
fn where_it_goes(request: &PublishRequest) -> String {
    if request.kind() == ChannelKind::Instagram {
        format!("em {}, na conta do Instagram conectada", request.channel())
    } else {
        format!("em {} ({})", request.channel(), request.place())
    }
}

/// What a Pin carries besides the text and the photo.
fn pin_detail(request: &PublishRequest) -> Option<String> {
    (request.kind() == ChannelKind::Pinterest).then(|| {
        format!(
            "Pin na pasta {}, com o título \"{}\" e o link {}",
            request.place(),
            request.title(),
            request.link()
        )
    })
}

/// How the destination of `kind` limits the posts the queue sends it.
fn limit_note(kind: ChannelKind) -> Option<String> {
    match kind {
        ChannelKind::Instagram => Some(format!(
            "O Instagram aceita até {INSTAGRAM_POSTS_PER_DAY} posts pelo app em 24 horas: com a \
             cota usada, o post espera na fila, sem chamar o Instagram, até abrir espaço."
        )),
        ChannelKind::Pinterest => Some(format!(
            "O app cria até {PINTEREST_POSTS_PER_DAY} Pins em 24 horas, dentro do limite do \
             Pinterest; no limite, ou com o Pinterest pedindo para esperar, o Pin espera na fila."
        )),
        _ => None,
    }
}

/// A destination at its limit, as the queue tells it.
fn limit_text(limit: &DestinationLimit) -> String {
    let name = post_destinations::destination_name(limit.kind).unwrap_or("O canal");
    format!(
        "{name} está no limite de posts: os agendados para ele esperam na fila, sem chamar o \
         {name}, até {}. Os que passarem de {LATE_AFTER_MINUTES} minutos da hora esperam você.",
        post_time(limit.until)
    )
}

/// The picture `editor` has picked, as a post keeps it.
fn picture_of(editor: &Editor, cx: &App) -> Option<PostImage> {
    match &editor.picture {
        Picture::None => None,
        Picture::File(path) => Some(PostImage::File(path.clone())),
        Picture::Url => {
            let url = editor.image_url.read(cx).value().trim().to_owned();
            url.strip_prefix("https://")
                .is_some_and(|rest| !rest.is_empty() && !rest.contains(char::is_whitespace))
                .then_some(PostImage::Url(url))
        }
    }
}

fn image_format(path: &Path) -> Option<ImageFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "png" => Some(ImageFormat::Png),
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        _ => None,
    }
}

impl Render for OfferPostsSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let editor = self.editor.take();
        let editing = editor.as_ref().map(|editor| self.render_editor(editor, cx));
        self.editor = editor;
        let confirming = self.confirming.take();
        let confirmation = confirming
            .as_ref()
            .map(|confirming| self.render_confirmation(confirming, cx));
        self.confirming = confirming;
        let queue = self.queue.clone();
        let mut index = 0;
        let mut groups: Vec<AnyElement> = Vec::new();
        for (title, posts) in [
            ("Esperando você", &queue.waiting),
            ("Agendados", &queue.scheduled),
            ("Não confirmados", &queue.unconfirmed),
            ("Com falha", &queue.failed),
            ("Publicados hoje", &queue.published_today),
        ] {
            if posts.is_empty() {
                continue;
            }
            groups.push(
                div()
                    .text_sm()
                    .font_medium()
                    .child(format!("{title} ({})", posts.len()))
                    .into_any_element(),
            );
            for post in posts {
                groups.push(self.render_post(index, post, cx));
                index += 1;
            }
        }
        let mut rows: Vec<AnyElement> = Vec::new();
        for post in self
            .posts
            .clone()
            .iter()
            .filter(|post| !queue.holds(post.id))
        {
            rows.push(self.render_post(index, post, cx));
            index += 1;
        }
        // A deleted channel's posts still show where they stand.
        let queues = !groups.is_empty()
            || self
                .channels
                .iter()
                .any(|channel| channel.kind.has_destination());
        let paused = queue.paused;
        let queue_block = queues.then(|| {
            v_flex()
                .gap_2()
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(kit::section_heading("Fila de publicação"))
                        .child(if paused {
                            kit::tag("publicação pausada", t.text2, cx)
                        } else {
                            kit::tag("publicação ligada", t.accent_text, cx)
                        })
                        .child(
                            Button::new("pause-queue")
                                .label(if paused {
                                    "Retomar publicação"
                                } else {
                                    "Pausar publicação"
                                })
                                .outline()
                                .small()
                                .disabled(self.busy)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.set_paused(!paused, window, cx)
                                })),
                        ),
                )
                .child(div().text_sm().text_color(t.text2).child(format!(
                    "Um post agendado sai sozinho na hora pelo Telegram, Instagram ou Pinterest, \
                     com o app aberto ou na bandeja, um por vez e com alguns segundos entre dois \
                     posts do mesmo chat ou da mesma conta. O Instagram e o Pinterest têm limite \
                     de posts por dia: no limite, o post espera na fila. Se a hora passar de \
                     {LATE_AFTER_MINUTES} minutos sem ele sair, ele espera você. Pausada, a fila \
                     não publica nada."
                )))
                .children(
                    queue
                        .limited
                        .iter()
                        .map(|limit| kit::info_notice(limit_text(limit), cx)),
                )
                .when(groups.is_empty(), |block| {
                    block.child(div().text_sm().text_color(t.text2).child(
                        "Nada na fila: use Agendar num post de canal do Telegram, do Instagram ou \
                         do Pinterest.",
                    ))
                })
                .children(groups)
        });
        v_flex()
            .gap_3()
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .children(confirmation)
            .children(queue_block)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(kit::section_heading("Posts de oferta"))
                    .child(
                        Button::new("new-post")
                            .label("Novo post")
                            .primary()
                            .small()
                            .disabled(self.busy || self.picking)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.picking = true;
                                cx.notify();
                            })),
                    ),
            )
            .child(div().text_sm().text_color(t.text2).child(
                "Um post por link de afiliado, com o modelo do canal: o app preenche produto, \
                 preço, desconto e link, e põe \"#publi · link de afiliado\" no fim. Edite, confira \
                 os avisos e copie para colar no canal, ou publique direto num canal do Telegram, \
                 do Pinterest ou do Instagram.",
            ))
            .children(self.picking.then(|| self.render_picker(cx)))
            .children(editing)
            .when(rows.is_empty(), |section| {
                section.child(
                    div()
                        .text_sm()
                        .text_color(t.text2)
                        .child("Nenhum post ainda."),
                )
            })
            .children(rows)
    }
}

#[cfg(test)]
mod tests {
    use mascate_affiliate::{
        AMAZON_TYPED_PRICE, AffiliatePlatform, CommissionStatus, LinkTagger, ManualCommission,
        NewAffiliateLink, Promotion, TagRefusal,
    };
    use mascate_kernel::{SystemClock, Timestamp, UuidV7Generator};
    use mascate_marketing::{
        ChannelKind, NewPromotionChannel, PRICE_NOTICE, PromotionChannels, price_time,
    };
    use mascate_platform::{Database, Flags, Registry, migrate};

    use super::*;
    use crate::startup::MODULE_MIGRATIONS;

    struct AmazonTagger;

    impl LinkTagger for AmazonTagger {
        fn platform(&self) -> AffiliatePlatform {
            AffiliatePlatform::Amazon
        }

        fn tag(&self, link: &str, tag: &str) -> Result<String, TagRefusal> {
            Ok(format!("{link}?tag={tag}"))
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        flags: Flags,
        affiliate: Affiliate,
        posts: OfferPosts,
        offer: Offer,
        channel: PromotionChannel,
    }

    async fn amazon() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        let (clock, ids) = (Arc::new(SystemClock), Arc::new(UuidV7Generator));
        migrate(&database, clock.as_ref(), MODULE_MIGRATIONS)
            .await
            .unwrap();
        let affiliate = Affiliate::new(database.clone(), clock.clone(), ids.clone());
        let commission = affiliate
            .record_manual(ManualCommission {
                platform: AffiliatePlatform::Amazon,
                program: String::new(),
                product: "Kindle".into(),
                sold_at: Utc::now(),
                amount: Money::new(5.into(), Currency::Brl),
                status: CommissionStatus::Pending,
            })
            .await
            .unwrap();
        let channel = PromotionChannels::new(database.clone(), clock.clone(), ids.clone())
            .create(&NewPromotionChannel {
                name: "Ofertas".into(),
                kind: ChannelKind::TelegramChannel,
                default_tag: "ofertas".into(),
                keywords: Vec::new(),
                telegram_chat: None,
            })
            .await
            .unwrap();
        let link = affiliate
            .create_link(
                &NewAffiliateLink {
                    program: commission.program,
                    product: "Kindle".into(),
                    pasted: "https://www.amazon.com.br/dp/B0ABCDEFGH".into(),
                    channel: channel.id,
                    tag: "mascate-tg-20".into(),
                    promotion: Promotion::default(),
                },
                &AmazonTagger,
            )
            .await
            .unwrap()
            .link;
        let offer = read_offer(&affiliate, None, link).await.unwrap();
        Fixture {
            _dir: dir,
            flags: Flags::new(
                database.clone(),
                Registry::new(&[mascate_affiliate::FLAGS]).unwrap(),
                clock.clone(),
                ids.clone(),
            ),
            posts: OfferPosts::new(database, clock, ids),
            affiliate,
            offer,
            channel,
        }
    }

    fn typed(seen_at: Timestamp) -> GivenPrice {
        GivenPrice {
            price: OfferPrice {
                price: Money::new(rust_decimal::Decimal::new(14990, 2), Currency::Brl),
                before: None,
                seen_at,
            },
            typed: true,
        }
    }

    #[test]
    fn an_amazon_post_has_no_price_until_its_restricted_feature_is_on() {
        futures::executor::block_on(async {
            let f = amazon().await;
            let seen_at = Utc::now();
            let off = gather(
                &f.affiliate,
                &f.posts,
                &f.offer,
                &f.channel,
                Some(typed(seen_at)),
                None,
            )
            .await
            .unwrap();
            assert_eq!(off.refused, Some(AMAZON_TYPED_PRICE));
            assert_eq!(off.draft.facts.price, None);
            assert!(!off.draft.text.contains("R$"), "{}", off.draft.text);
            assert!(off.draft.text.contains("?tag=mascate-tg-20"));
            // Amazon forbids e-mail and direct messages by default.
            assert_eq!(
                off.draft.facts.forbidden_contacts,
                vec![Contact::Email, Contact::Phone]
            );

            f.flags
                .turn_on(
                    f.flags
                        .request_turn_on(AMAZON_TYPED_PRICE.key)
                        .unwrap()
                        .confirm("teste"),
                )
                .await
                .unwrap();
            let on = gather(
                &f.affiliate,
                &f.posts,
                &f.offer,
                &f.channel,
                Some(typed(seen_at)),
                None,
            )
            .await
            .unwrap();
            assert_eq!(on.refused, None);
            assert_eq!(
                on.restricted,
                vec![RuleWarning::Restricted(AMAZON_TYPED_PRICE)]
            );
            assert!(on.draft.text.contains("R$ 149,90"));
            assert!(on.draft.text.contains(&price_time(seen_at)));
            assert!(on.draft.text.contains(PRICE_NOTICE));
        });
    }

    #[test]
    fn typed_amounts_read_as_reais() {
        assert_eq!(
            typed_money("149,90"),
            Ok(Some(Money::new(
                rust_decimal::Decimal::new(14990, 2),
                Currency::Brl
            )))
        );
        assert_eq!(typed_money("  "), Ok(None));
        assert_eq!(typed_money("caro"), Err(()));
        assert_eq!(typed_money("-3"), Err(()));
        assert_eq!(
            money_text(Money::new(
                rust_decimal::Decimal::new(129990, 2),
                Currency::Brl
            )),
            "1.299,90"
        );
    }
}
