//! Posts de oferta on Divulgação (#102): an Offer Post from an Affiliate
//! Link, its text born from the channel's template, edited with a preview
//! and the problems beside it, then copied with a click. The app gathers
//! the facts the Marketing drafts from (ADR 0031, decision 5): the link and
//! its Program's rules from the Affiliate, the Product's name and photos
//! from the Catalog, the price typed or the one Shopee's search gave. An
//! Amazon price waits for its Restricted Feature; without it the post goes
//! without a price.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
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
use mascate_kernel::{Currency, Money, RecordId, parse_amount};
use mascate_marketing::{
    Contact, OfferFacts, OfferPost, OfferPosts, OfferPrice, PostDraft, PostError, PostImage,
    PostProblem, PromotionChannel, check_post, post_limits,
};
use mascate_platform::Flag;

use crate::affiliate::{affiliate, failure};
use crate::affiliate_links::{promotion, warning_text};
use crate::appearance::look;
use crate::catalog::{self, day_and_time};
use crate::forms::{Outcome, input, notice};
use crate::kit;
use crate::promotion_channels::{ChannelChanges, promotion_channels};
use crate::sales::table_row;

/// The app's Offer Posts; absent when the database did not open.
pub struct AppOfferPosts(pub Arc<OfferPosts>);

impl Global for AppOfferPosts {}

pub(crate) fn offer_posts(cx: &App) -> Option<Arc<OfferPosts>> {
    cx.try_global::<AppOfferPosts>().map(|app| app.0.clone())
}

/// Counts the posts copied, so the disclosure Reminder is read again.
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
        PostError::Unreadable(_) | PostError::Sql(_) => {
            format!("Não consegui ler ou gravar os posts no banco: {error}")
        }
    }
}

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
            "Com foto, o Telegram aceita legenda de até {limit} caracteres, e o texto tem {units}. \
             Encurte o texto ou tire a foto."
        ),
        PostProblem::TooLong { units, limit, .. } => format!(
            "O Telegram aceita até {limit} caracteres, e o texto tem {units}. Encurte o texto."
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
    links: Vec<AffiliateLink>,
    channels: Vec<PromotionChannel>,
    /// "Novo post" lists the links to start from.
    picking: bool,
    editor: Option<Editor>,
    /// The post asking to confirm its deletion.
    deleting: Option<RecordId>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl OfferPostsSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe_global_in::<ChannelChanges>(window, Self::refresh)];
        let mut section = Self {
            posts: Vec::new(),
            links: Vec::new(),
            channels: Vec::new(),
            picking: false,
            editor: None,
            deleting: None,
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
            let posts = posts.posts().await.map_err(|error| post_failure(&error));
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
                    Ok((posts, links, channels)) => {
                        this.posts = posts;
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
        let (opened, copying, id) = (post.clone(), post.clone(), post.id);
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
                .child(
                    Button::new(("open-post", index))
                        .label("Abrir")
                        .outline()
                        .small()
                        .disabled(self.busy)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open(opened.clone(), window, cx)
                        })),
                )
                .child(
                    Button::new(("copy-post", index))
                        .label("Copiar texto")
                        .outline()
                        .small()
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.copy_text(copying.clone(), window, cx)
                        })),
                )
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
                            .child(format!("rascunho · {copied}")),
                    ),
            )
            .child(actions)
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
        let channel = editor.channel.as_ref().map_or_else(
            || "canal apagado".to_owned(),
            |channel| channel.name.clone(),
        );
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
                            if problem.blocks() {
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
            .children(editor.form.as_ref().map(|outcome| notice(outcome, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("copy-post-text")
                            .label("Copiar texto")
                            .primary()
                            .small()
                            .disabled(self.busy || blocked)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.copy_editing(window, cx)),
                            ),
                    )
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
        let rows: Vec<AnyElement> = self
            .posts
            .clone()
            .iter()
            .enumerate()
            .map(|(index, post)| self.render_post(index, post, cx))
            .collect();
        v_flex()
            .gap_3()
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
                 os avisos e copie para colar no canal.",
            ))
            .children(self.outcome.as_ref().map(|outcome| notice(outcome, cx)))
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
