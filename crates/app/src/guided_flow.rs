//! Passo a passo (#198, ADR 0037 decision 8): the Guided Flow takes one
//! affiliate product from its Program to its result in five steps. Each
//! step is the section Afiliação or Divulgação already shows, focused on
//! the product, so it leaves the same records and asks the same
//! confirmations; the screen holds no business rule. The step a product is
//! at comes from what the Affiliate and the Marketing hold (a link, a post,
//! a spend, a sale), never from a state of its own.

use std::collections::HashSet;

use chrono::{TimeDelta, Utc};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{Disableable as _, Sizable as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::{AnyElement, ClipboardItem, Entity, Subscription, Window, div, px};
use mascate_affiliate::{
    AffiliateLink, ClickReport, Commission, CommissionFilter, CommissionGroup, CommissionReport,
    CommissionStatus, GroupOrder, LinkClicks, Program,
};
use mascate_kernel::{ChannelSpend, RecordId, channel_day};
use mascate_marketing::{ChannelKind, OfferPost, PostStatus, PromotionChannel, PromotionSpend};

use crate::affiliate::{CommissionChanges, affiliate};
use crate::affiliate_links::{
    AffiliateLinkCreated, AffiliateLinksSection, LinkFocus, ProgramAdded, tagger,
};
use crate::appearance::{Tokens, look};
use crate::catalog::NO_DATABASE;
use crate::commission_dashboard::{figure, money_text, pause_text, profit_ink, roas_text};
use crate::forms::{Outcome, input, notice};
use crate::kit;
use crate::layout;
use crate::offer_posts::{OfferPostsSection, PostChanges, StartPost, offer_posts};
use crate::parts::ScreenParts;
use crate::preferences;
use crate::promotion_channels::{
    ChannelChanges, failure as channel_failure, kind_note, promotion_channels, typed_channel,
};
use crate::promotion_rules::PromotionRulesSection;
use crate::promotion_spends::{SpendChanges, SpendsSection, promotion_spends};
use crate::result_promises::findings_notice;
use crate::sales::table_row;
use crate::span::{SpanChanged, SpanPicker};

/// Where a product is on its way from the Program to the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Pick or add the Program, and review the producer's rules.
    Product,
    /// The Affiliate Link on a Promotion Channel, and its Short Link.
    Link,
    /// An Offer Post, or the text and link to paste in an ad.
    Promote,
    /// What a paid channel spent.
    Spend,
    /// Clicks, sales, earned, spend, profit and ROAS.
    Result,
}

impl Step {
    const ALL: [Step; 5] = [
        Step::Product,
        Step::Link,
        Step::Promote,
        Step::Spend,
        Step::Result,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Step::Product => "Produto",
            Step::Link => "Link",
            Step::Promote => "Divulgar",
            Step::Spend => "Gasto",
            Step::Result => "Resultado",
        }
    }

    /// The steps a product goes through: only a paid channel has a spend to
    /// record.
    pub fn path(paid: bool) -> Vec<Step> {
        Self::ALL
            .into_iter()
            .filter(|step| paid || *step != Step::Spend)
            .collect()
    }
}

/// What the modules hold of a product.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    /// It has a live Affiliate Link.
    pub linked: bool,
    /// A post of one of its links was copied, scheduled or published.
    pub posted: bool,
    /// One of its links is on a paid channel.
    pub paid: bool,
    /// A paid channel of its links spent since the link was created.
    pub spent: bool,
    /// A sale came in through its links since the first was created.
    pub sold: bool,
}

impl Progress {
    /// The step the product is at: the first one with nothing done. A sale
    /// shows it was promoted, and a paid channel's spend that its ad runs.
    pub fn step(self) -> Step {
        if !self.linked {
            return Step::Link;
        }
        let promoted = self.posted || self.sold;
        match (self.paid, self.spent) {
            (true, false) if promoted => Step::Spend,
            (true, true) => Step::Result,
            _ if promoted => Step::Result,
            _ => Step::Promote,
        }
    }
}

/// A product on its way, as the modules hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Underway {
    pub program: Program,
    /// As the Platform names it.
    pub product: String,
    /// Its live Affiliate Links, one per channel.
    pub links: Vec<AffiliateLink>,
    pub progress: Progress,
}

/// What the Affiliate and the Marketing hold that tells each product's step.
#[derive(Debug, Clone, Default)]
pub struct Held {
    pub programs: Vec<Program>,
    pub links: Vec<AffiliateLink>,
    pub posts: Vec<OfferPost>,
    pub channels: Vec<PromotionChannel>,
    /// The spends of the channels the links are on.
    pub spends: Vec<PromotionSpend>,
    /// The Commissions sold since the first link was created.
    pub commissions: Vec<Commission>,
}

/// Whether a channel of `kind` is paid for, so its spend is recorded.
pub fn paid(kind: ChannelKind) -> bool {
    matches!(kind, ChannelKind::PaidSearch | ChannelKind::PaidSocial)
}

/// Whether `post` went or will go out: copied to share by hand, or
/// confirmed to a destination.
fn promotes(post: &OfferPost) -> bool {
    post.copied_at.is_some()
        || matches!(
            post.status,
            PostStatus::Scheduled
                | PostStatus::Late
                | PostStatus::PriceExpired
                | PostStatus::Sending
                | PostStatus::Sent
                | PostStatus::Unconfirmed
        )
}

/// Every product with an Affiliate Link, and every product added with its
/// Program and no link yet, by Platform, Program and product, with what
/// the modules hold of it.
pub fn underway(held: &Held) -> Vec<Underway> {
    let mut products: Vec<Underway> = Vec::new();
    for program in &held.programs {
        let mut named: Vec<String> = Vec::new();
        for link in held
            .links
            .iter()
            .filter(|link| link.program.id == program.id)
        {
            if !named.contains(&link.product) {
                named.push(link.product.clone());
            }
        }
        if let Some(product) = program.product.as_ref().map(|product| product.trim())
            && !product.is_empty()
            && !named.iter().any(|named| named == product)
        {
            named.push(product.to_owned());
        }
        named.sort();
        for product in named {
            let links: Vec<AffiliateLink> = held
                .links
                .iter()
                .filter(|link| link.program.id == program.id && link.product == product)
                .cloned()
                .collect();
            let progress = progress_of(program, &product, &links, held);
            products.push(Underway {
                program: program.clone(),
                product,
                links,
                progress,
            });
        }
    }
    products
}

fn progress_of(program: &Program, product: &str, links: &[AffiliateLink], held: &Held) -> Progress {
    let kind = |link: &AffiliateLink| {
        held.channels
            .iter()
            .find(|channel| channel.id == link.channel)
            .map(|channel| channel.kind)
    };
    let paid_links: Vec<&AffiliateLink> = links
        .iter()
        .filter(|link| kind(link).is_some_and(paid))
        .collect();
    let ids: HashSet<RecordId> = links.iter().map(|link| link.id).collect();
    let first_link = links.iter().map(|link| link.created_at).min();
    Progress {
        linked: !links.is_empty(),
        posted: held
            .posts
            .iter()
            .any(|post| ids.contains(&post.draft.facts.link_id) && promotes(post)),
        paid: !paid_links.is_empty(),
        spent: paid_links.iter().any(|link| {
            held.spends.iter().any(|spend| {
                spend.channel == link.channel && spend.last_day >= channel_day(link.created_at)
            })
        }),
        sold: first_link.is_some_and(|first| {
            held.commissions.iter().any(|commission| {
                commission.status != CommissionStatus::Cancelled
                    && commission.sold_at >= first
                    && (commission.link.is_some_and(|link| ids.contains(&link))
                        || (commission.program == program.id
                            && commission.product.trim() == product))
            })
        }),
    }
}

/// "Passo 3 de 4 · Divulgar".
pub fn step_text(step: Step, paid: bool) -> String {
    let path = Step::path(paid);
    let at = path.iter().position(|on| *on == step).unwrap_or_default() + 1;
    format!("Passo {at} de {} · {}", path.len(), step.name())
}

/// The product the screen has open, and the step it shows.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Open {
    /// `None` until the first step picks or adds one.
    program: Option<RecordId>,
    /// `None` until the Program names it or a link is created.
    product: Option<String>,
    step: Step,
}

/// What the Resultado step read for the period.
#[derive(Default)]
struct Results {
    reports: Vec<CommissionReport>,
    clicks: Vec<LinkClicks>,
    commissions: Vec<Commission>,
}

/// A Promotion Channel being created from the Link step.
struct NewChannel {
    name: Entity<InputState>,
    kind: ChannelKind,
    keywords: Entity<InputState>,
    chat: Entity<InputState>,
    outcome: Option<Outcome>,
}

pub struct GuidedFlowScreen {
    held: Held,
    underway: Vec<Underway>,
    open: Option<Open>,
    links: Entity<AffiliateLinksSection>,
    rules: Entity<PromotionRulesSection>,
    posts: Entity<OfferPostsSection>,
    spends: Entity<SpendsSection>,
    span: Entity<SpanPicker>,
    results: Option<Result<Results, String>>,
    /// The text of an ad, checked as the owner types.
    ad_text: Entity<TextareaState>,
    new_channel: Option<NewChannel>,
    busy: bool,
    /// Numbers each read, so a slow one never lands over a newer one.
    reads: u64,
    /// The same, for Resultado's reads.
    result_reads: u64,
    outcome: Option<Outcome>,
    _subscriptions: Vec<Subscription>,
}

impl GuidedFlowScreen {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let links = cx.new(|cx| AffiliateLinksSection::focused(window, cx));
        let rules = cx.new(|cx| PromotionRulesSection::focused(window, cx));
        let posts = cx.new(|cx| OfferPostsSection::focused(window, cx));
        let spends = cx.new(SpendsSection::focused);
        let span = cx.new(|cx| SpanPicker::new(window, cx));
        let ad_text = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 12)
                .placeholder("O texto do anúncio, como vai na Meta, no Google ou no TikTok")
        });
        let subscriptions = vec![
            cx.observe_global_in::<ChannelChanges>(window, Self::read),
            cx.observe_global_in::<PostChanges>(window, Self::read),
            cx.observe_global_in::<CommissionChanges>(window, Self::read),
            cx.observe_global_in::<SpendChanges>(window, Self::read),
            // "Nova afiliação" added it, and its form became "Criar link".
            cx.subscribe_in(
                &links,
                window,
                |this, _, added: &ProgramAdded, window, cx| {
                    let product = added.0.product.clone();
                    this.pick(added.0.id, product, window, cx);
                    this.rules.update(cx, |rules, cx| rules.refresh(window, cx));
                },
            ),
            cx.subscribe_in(
                &links,
                window,
                |this, _, created: &AffiliateLinkCreated, window, cx| {
                    if let Some(open) = this.open.as_mut() {
                        open.program = Some(created.0.program.id);
                        open.product = Some(created.0.product.clone());
                    }
                    this.focus_sections(cx);
                    this.read(window, cx);
                },
            ),
            // "Criar post" on a link of the product.
            cx.subscribe_in(&links, window, |this, _, start: &StartPost, window, cx| {
                this.start_post(start, window, cx)
            }),
            cx.subscribe_in(&span, window, |this, _, _: &SpanChanged, window, cx| {
                this.read_results(window, cx)
            }),
            cx.subscribe_in(&ad_text, window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let mut screen = Self {
            held: Held::default(),
            underway: Vec::new(),
            open: None,
            links,
            rules,
            posts,
            spends,
            span,
            results: None,
            ad_text,
            new_channel: None,
            busy: false,
            reads: 0,
            result_reads: 0,
            outcome: None,
            _subscriptions: subscriptions,
        };
        screen.read(window, cx);
        screen
    }

    /// Reads everything again, as when the screen comes into view.
    pub fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.links.update(cx, |links, cx| links.refresh(window, cx));
        self.rules.update(cx, |rules, cx| rules.refresh(window, cx));
        self.posts.update(cx, |posts, cx| posts.refresh(window, cx));
        self.read(window, cx);
    }

    /// Follows a new Work Context: a product of a Platform it does not show
    /// closes, and each step closes what it had open without sending it.
    pub fn follow_context(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let context = preferences::work_context(cx);
        if self
            .open
            .as_ref()
            .and_then(|open| self.program(open.program))
            .is_some_and(|program| !context.shows_platform(program.platform))
        {
            self.close(cx);
        }
        self.links.update(cx, AffiliateLinksSection::follow_context);
        self.rules.update(cx, PromotionRulesSection::follow_context);
        self.posts.update(cx, OfferPostsSection::follow_context);
        self.read(window, cx);
    }

    /// Reads what the modules hold, and derives each product's step.
    fn read(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(affiliate), Some(posts), Some(channels), Some(spends)) = (
            affiliate(cx),
            offer_posts(cx),
            promotion_channels(cx),
            promotion_spends(cx),
        ) else {
            return;
        };
        self.reads += 1;
        let read = self.reads;
        let working = cx.background_executor().spawn(async move {
            let mut held = Held {
                programs: affiliate.programs().await.map_err(|e| e.to_string())?,
                links: affiliate.links().await.map_err(|e| e.to_string())?,
                posts: posts.posts().await.map_err(|e| e.to_string())?,
                channels: channels.channels().await.map_err(|e| e.to_string())?,
                ..Held::default()
            };
            let mut linked: Vec<RecordId> = held.links.iter().map(|link| link.channel).collect();
            linked.sort();
            linked.dedup();
            for channel in held.channels.iter().filter(|channel| paid(channel.kind)) {
                if linked.contains(&channel.id) {
                    held.spends
                        .extend(spends.spends(channel.id).await.map_err(|e| e.to_string())?);
                }
            }
            if let Some(first) = held.links.iter().map(|link| link.created_at).min() {
                held.commissions = affiliate
                    .commissions(&CommissionFilter {
                        statuses: Vec::new(),
                        sold: first..Utc::now() + TimeDelta::days(1),
                    })
                    .await
                    .map_err(|e| e.to_string())?;
            }
            Ok::<_, String>(held)
        });
        cx.spawn_in(window, async move |this, cx| {
            let held = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.reads != read {
                    return;
                }
                match held {
                    Ok(held) => {
                        this.underway = underway(&held);
                        let channels = held.channels.clone();
                        this.held = held;
                        this.spends
                            .update(cx, |spends, cx| spends.set_channels(channels, cx));
                        this.focus_sections(cx);
                        if this
                            .open
                            .as_ref()
                            .is_some_and(|open| open.step == Step::Result)
                        {
                            this.read_results(window, cx);
                        }
                    }
                    Err(error) => {
                        this.outcome = Some(Outcome::Failed(
                            format!("Não consegui ler o passo a passo: {error}").into(),
                        ))
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads the period's Commissions, spends and clicks for Resultado, as
    /// Afiliação sums them for the Work Context.
    fn read_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(affiliate), Some(spending)) = (affiliate(cx), promotion_spends(cx)) else {
            return;
        };
        let during = self.span.read(cx).during();
        let days = self.span.read(cx).days();
        let context = preferences::work_context(cx);
        self.result_reads += 1;
        let read = self.result_reads;
        let working = cx.background_executor().spawn(async move {
            let commissions = affiliate
                .commissions(&CommissionFilter {
                    statuses: Vec::new(),
                    sold: during.clone(),
                })
                .await
                .map_err(|e| e.to_string())?;
            let links = affiliate.links().await.map_err(|e| e.to_string())?;
            let clicks = affiliate
                .link_clicks(during)
                .await
                .map_err(|e| e.to_string())?;
            let spent: Vec<ChannelSpend> = spending.spent(days).await.map_err(|e| e.to_string())?;
            let spends = context.spends_in(&spent, &commissions, &links);
            let shown: Vec<Commission> = commissions
                .into_iter()
                .filter(|commission| context.shows_platform(commission.platform))
                .collect();
            Ok::<_, String>(Results {
                reports: CommissionReport::of(&shown, &spends, GroupOrder::Earned),
                clicks,
                commissions: shown,
            })
        });
        cx.spawn_in(window, async move |this, cx| {
            let results = working.await;
            let _ = this.update(cx, |this, cx| {
                if this.result_reads == read {
                    this.results = Some(results);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn program(&self, id: Option<RecordId>) -> Option<&Program> {
        self.held
            .programs
            .iter()
            .find(|program| Some(program.id) == id)
    }

    fn channel(&self, id: RecordId) -> Option<&PromotionChannel> {
        self.held.channels.iter().find(|channel| channel.id == id)
    }

    fn channel_name(&self, id: RecordId) -> String {
        self.channel(id).map_or_else(
            || "canal apagado".to_owned(),
            |channel| channel.name.clone(),
        )
    }

    /// The open product as the modules hold it, once it has a Program and a
    /// name.
    fn current(&self) -> Option<&Underway> {
        let open = self.open.as_ref()?;
        self.underway.iter().find(|underway| {
            Some(underway.program.id) == open.program
                && Some(&underway.product) == open.product.as_ref()
        })
    }

    /// Points each step's section at the open product.
    fn focus_sections(&mut self, cx: &mut Context<Self>) {
        let focus = self.open.as_ref().and_then(|open| {
            Some(LinkFocus {
                program: open.program?,
                product: open.product.clone(),
            })
        });
        let program = self.open.as_ref().and_then(|open| open.program);
        let links: Vec<&AffiliateLink> = self
            .current()
            .map_or_else(Vec::new, |current| current.links.iter().collect());
        let link_ids: Vec<RecordId> = links.iter().map(|link| link.id).collect();
        let paid_channels: Vec<RecordId> = links
            .iter()
            .filter(|link| self.channel(link.channel).is_some_and(|c| paid(c.kind)))
            .map(|link| link.channel)
            .collect();
        self.links.update(cx, |links, cx| links.focus(focus, cx));
        self.rules.update(cx, |rules, cx| rules.focus(program, cx));
        self.posts.update(cx, |posts, cx| posts.focus(link_ids, cx));
        self.spends
            .update(cx, |spends, cx| spends.focus(paid_channels, cx));
    }

    /// "Começar com outro produto": the first step, with "Nova afiliação"
    /// open and nothing picked.
    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = Some(Open {
            program: None,
            product: None,
            step: Step::Product,
        });
        self.outcome = None;
        self.focus_sections(cx);
        self.links
            .update(cx, |links, cx| links.open_adding(window, cx));
        cx.notify();
    }

    /// Opens a product in progress at the step it is at.
    fn resume(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(underway) = self.underway.get(index) else {
            return;
        };
        let step = underway.progress.step();
        self.open = Some(Open {
            program: Some(underway.program.id),
            product: Some(underway.product.clone()),
            step,
        });
        self.outcome = None;
        self.go(step, window, cx);
    }

    /// The first step picked or added `program`: the link is next.
    fn pick(
        &mut self,
        program: RecordId,
        product: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let product = product.or_else(|| {
            self.program(Some(program))
                .and_then(|program| program.product.clone())
        });
        self.open = Some(Open {
            program: Some(program),
            product: product.clone(),
            step: Step::Product,
        });
        self.go(Step::Link, window, cx);
        self.read(window, cx);
    }

    /// Shows `step` of the open product, opening what the step starts with.
    fn go(&mut self, step: Step, window: &mut Window, cx: &mut Context<Self>) {
        let Some(open) = self.open.as_mut() else {
            return;
        };
        open.step = step;
        let (program, product) = (open.program, open.product.clone());
        self.focus_sections(cx);
        let linked = self
            .current()
            .is_some_and(|current| current.progress.linked);
        if step == Step::Link
            && !linked
            && let Some(program) = program
        {
            self.links.update(cx, |links, cx| {
                links.keep_open(program, product, window, cx)
            });
        }
        if step == Step::Result {
            self.read_results(window, cx);
        }
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.open = None;
        self.new_channel = None;
        self.focus_sections(cx);
        cx.notify();
    }

    /// Starts a post from one of the product's links, on the Divulgar step.
    fn start_post(&mut self, start: &StartPost, window: &mut Window, cx: &mut Context<Self>) {
        self.go(Step::Promote, window, cx);
        self.posts
            .update(cx, |posts, cx| posts.start(start, window, cx));
    }

    fn copy(&mut self, text: String, what: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.outcome = Some(Outcome::Done(format!("{what} copiado.").into()));
        cx.notify();
    }

    fn open_new_channel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_channel = Some(NewChannel {
            name: input("Anúncio Meta · Curso de Fotografia", window, cx),
            kind: ChannelKind::PaidSocial,
            keywords: input("curso de fotografia, fotografia online", window, cx),
            chat: input("@ofertasdofernando ou -100…", window, cx),
            outcome: None,
        });
        cx.notify();
    }

    /// Creates the channel the owner typed, as Divulgação does, and picks it
    /// for the link.
    fn create_channel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(channels), Some(form)) = (promotion_channels(cx), self.new_channel.as_ref())
        else {
            return;
        };
        let typed = typed_channel(
            form.name.read(cx).value().to_string(),
            "",
            form.kind,
            &form.keywords.read(cx).value(),
            form.chat.read(cx).value().to_string(),
        );
        self.busy = true;
        let working = cx
            .background_executor()
            .spawn(async move { channels.create(&typed).await });
        cx.spawn_in(window, async move |this, cx| {
            let created = working.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match created {
                    Ok(channel) => {
                        this.new_channel = None;
                        this.outcome = Some(Outcome::Done(
                            format!(
                                "Canal {} criado, com a marcação {}.",
                                channel.name, channel.default_tag
                            )
                            .into(),
                        ));
                        this.links
                            .update(cx, |links, cx| links.pick_channel(channel.id, window, cx));
                        cx.update_default_global::<ChannelChanges, _>(|changes, _| changes.0 += 1);
                    }
                    Err(error) => {
                        if let Some(form) = this.new_channel.as_mut() {
                            form.outcome = Some(Outcome::Failed(channel_failure(&error).into()));
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}

impl GuidedFlowScreen {
    fn render_list(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let context = preferences::work_context(cx);
        let rows: Vec<AnyElement> = self
            .underway
            .iter()
            .enumerate()
            .filter(|(_, underway)| context.shows_platform(underway.program.platform))
            .map(|(index, underway)| {
                let channels: Vec<String> = underway
                    .links
                    .iter()
                    .map(|link| self.channel_name(link.channel))
                    .collect();
                let detail = if channels.is_empty() {
                    "sem link ainda".to_owned()
                } else {
                    channels.join(", ")
                };
                let progress = underway.progress;
                table_row(&t)
                    .flex_wrap()
                    .child(kit::tag(underway.program.platform.name(), t.text2, cx))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w(px(220.))
                            .child(
                                div()
                                    .font_medium()
                                    .truncate()
                                    .child(underway.product.clone()),
                            )
                            .child(div().text_xs().text_color(t.text2).truncate().child(detail)),
                    )
                    .child(kit::tag(
                        step_text(progress.step(), progress.paid),
                        t.accent_text,
                        cx,
                    ))
                    .child(
                        Button::new(("resume-product", index))
                            .label(if progress.step() == Step::Result {
                                "Ver resultado"
                            } else {
                                "Continuar"
                            })
                            .outline()
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.resume(index, window, cx)
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        v_flex()
            .gap_3()
            .child(kit::section_heading("Em andamento"))
            .child(div().text_sm().text_color(t.text2).child(
                "Cada produto de afiliado com o passo em que está, pelo que o app já guarda: \
                 tem link? tem post? tem gasto? tem venda? Continue de onde parou, ou comece \
                 com outro produto.",
            ))
            .when(rows.is_empty(), |list| {
                list.child(div().text_sm().text_color(t.text2).child(
                    "Nenhum produto em andamento. Use \"Começar com outro produto\" para \
                     cadastrar a afiliação e seguir os passos até o resultado.",
                ))
            })
            .children(rows)
            .into_any_element()
    }

    fn render_steps(&self, open: &Open, cx: &mut Context<Self>) -> AnyElement {
        let progress = self.current().map(|current| current.progress);
        let paid = progress.is_some_and(|progress| progress.paid);
        let reached = progress.map_or(Step::Product, Progress::step);
        let path = Step::path(paid);
        let reached_at = path.iter().position(|step| *step == reached).unwrap_or(0);
        let linked = progress.is_some_and(|progress| progress.linked);
        let buttons = path.iter().copied().enumerate().map(|(at, step)| {
            let open_ok = match step {
                Step::Product => true,
                Step::Link => open.program.is_some(),
                Step::Promote | Step::Spend | Step::Result => linked,
            };
            let button = Button::new(("guided-step", at))
                .label(format!("{} {}", at + 1, step.name()))
                .small()
                .disabled(!open_ok);
            let button = if step == open.step {
                button.primary()
            } else if at < reached_at {
                button.outline().icon(IconName::Check)
            } else {
                button.ghost()
            };
            button.on_click(cx.listener(move |this, _, window, cx| this.go(step, window, cx)))
        });
        h_flex()
            .flex_wrap()
            .gap_1()
            .items_center()
            .children(buttons)
            .into_any_element()
    }

    /// "Próximo passo" and the way back to the list.
    fn render_moves(&self, open: &Open, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let progress = self.current().map(|current| current.progress);
        let path = Step::path(progress.is_some_and(|progress| progress.paid));
        let next = path
            .iter()
            .position(|step| *step == open.step)
            .and_then(|at| path.get(at + 1))
            .copied();
        let (ready, why) = match open.step {
            Step::Product => (open.program.is_some(), "Escolha ou cadastre a afiliação."),
            Step::Link => (
                progress.is_some_and(|progress| progress.linked),
                "Crie o link num canal para seguir.",
            ),
            Step::Promote | Step::Spend | Step::Result => (true, ""),
        };
        h_flex()
            .flex_wrap()
            .gap_2()
            .items_center()
            .children(next.map(|next| {
                Button::new("guided-next")
                    .label(format!("Próximo passo: {}", next.name()))
                    .primary()
                    .small()
                    .disabled(!ready)
                    .on_click(cx.listener(move |this, _, window, cx| this.go(next, window, cx)))
            }))
            .child(
                Button::new("guided-back")
                    .label("Voltar à lista")
                    .ghost()
                    .small()
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            )
            .when(next.is_some() && !ready, |moves| {
                moves.child(div().text_sm().text_color(t.text2).child(why))
            })
            .into_any_element()
    }

    fn render_product(&self, open: &Open, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        if let Some(program) = self.program(open.program) {
            let changeable = !self
                .current()
                .is_some_and(|current| current.progress.linked);
            return v_flex()
                .gap_3()
                .child(div().text_sm().text_color(t.text2).child(
                    "Confira as regras do produtor antes de divulgar: o que ele permite, \
                     proíbe ou ainda não foi conferido. O app avisa em cada link e em cada post \
                     que quebra uma delas.",
                ))
                .when(program.provisional, |step| {
                    step.child(div().text_sm().text_color(t.text2).child(
                        "Afiliação aguardando a primeira venda: quando ela chegar, Afiliação \
                         pergunta se é desta.",
                    ))
                })
                .child(self.rules.clone())
                .when(changeable, |step| {
                    step.child(
                        h_flex().child(
                            Button::new("guided-change-program")
                                .label("Trocar de afiliação")
                                .ghost()
                                .small()
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.start(window, cx)),
                                ),
                        ),
                    )
                })
                .into_any_element();
        }
        let context = preferences::work_context(cx);
        let programs: Vec<AnyElement> = self
            .held
            .programs
            .iter()
            .filter(|program| context.shows_platform(program.platform))
            .filter(|program| tagger(program.platform, cx).is_some())
            .enumerate()
            .map(|(index, program)| {
                let id = program.id;
                table_row(&t)
                    .flex_wrap()
                    .child(kit::tag(program.platform.name(), t.text2, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(220.))
                            .font_medium()
                            .truncate()
                            .child(program.name.clone()),
                    )
                    .child(
                        Button::new(("guided-pick-program", index))
                            .label("Usar esta afiliação")
                            .outline()
                            .small()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.pick(id, None, window, cx)
                            })),
                    )
                    .into_any_element()
            })
            .collect();
        let adding = self.links.read(cx).adding();
        v_flex()
            .gap_3()
            .when(!adding, |step| {
                step.child(div().text_sm().text_color(t.text2).child(
                    "Cadastre a afiliação que você acabou de pegar na plataforma, como aparece \
                     na página de afiliação, ou escolha uma que o app já conhece.",
                ))
                .child(
                    h_flex().child(
                        Button::new("guided-add-program")
                            .label("Nova afiliação")
                            .primary()
                            .small()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.links
                                    .update(cx, |links, cx| links.open_adding(window, cx));
                            })),
                    ),
                )
            })
            .child(self.links.clone())
            .when(!programs.is_empty(), |step| {
                step.child(
                    div()
                        .text_sm()
                        .font_medium()
                        .child("Ou use uma afiliação que você já tem"),
                )
                .children(programs)
            })
            .into_any_element()
    }

    fn render_link(&self, open: &Open, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let Some(program) = self.program(open.program) else {
            return div().into_any_element();
        };
        if tagger(program.platform, cx).is_none() {
            return div()
                .text_sm()
                .text_color(t.text2)
                .child(format!(
                    "O app ainda não marca links da {}: crie o link na plataforma e lance as \
                     comissões em Afiliação.",
                    program.platform.name()
                ))
                .into_any_element();
        }
        v_flex()
            .gap_3()
            .child(div().text_sm().text_color(t.text2).child(
                "Escolha o canal onde vai divulgar e cole o link de afiliado: o app acrescenta a \
                 marcação do canal, que volta em cada venda. Um canal por criativo deixa o \
                 resultado de cada anúncio separado. Com o Short.io conectado, \"Encurtar\" \
                 cria o link curto que conta os cliques, onde os termos permitem.",
            ))
            .child(self.rules.clone())
            .child(match &self.new_channel {
                Some(form) => self.render_new_channel(form, cx),
                None => {
                    h_flex()
                        .child(
                            Button::new("guided-new-channel")
                                .label("Novo canal")
                                .icon(IconName::Plus)
                                .ghost()
                                .small()
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_new_channel(window, cx)
                                })),
                        )
                        .into_any_element()
                }
            })
            .child(self.links.clone())
            .into_any_element()
    }

    fn render_new_channel(&self, form: &NewChannel, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let kinds = ChannelKind::ALL
            .into_iter()
            .enumerate()
            .map(|(index, kind)| {
                let chip = Button::new(("guided-channel-kind", index))
                    .label(kind.name())
                    .small();
                if kind == form.kind {
                    chip.primary()
                } else {
                    chip.ghost()
                }
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = this.new_channel.as_mut() {
                        form.kind = kind;
                    }
                    cx.notify();
                }))
            });
        let field = |label: &'static str, input: &Entity<InputState>| {
            v_flex()
                .gap_1()
                .child(div().text_sm().child(label))
                .child(div().w(px(360.)).child(Input::new(input).small()))
        };
        v_flex()
            .gap_3()
            .p_3()
            .rounded(t.radius_lg)
            .border(t.border_width)
            .border_color(t.border)
            .bg(t.raised)
            .child(div().font_medium().child("Novo canal"))
            .child(field("Nome", &form.name))
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().child("Tipo"))
                    .child(h_flex().flex_wrap().gap_1().children(kinds)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(t.text2)
                    .child(kind_note(form.kind)),
            )
            .when(form.kind == ChannelKind::PaidSearch, |fields| {
                fields.child(field("Palavras-chave", &form.keywords))
            })
            .when(form.kind == ChannelKind::TelegramChannel, |fields| {
                fields.child(field("Chat do Telegram", &form.chat))
            })
            .child(div().text_xs().text_color(t.text2).child(
                "A marcação sai do nome; mude-a, os horários e o modelo dos posts em Divulgação.",
            ))
            .children(form.outcome.as_ref().map(|outcome| notice(outcome, cx)))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("guided-create-channel")
                            .label("Criar canal")
                            .primary()
                            .small()
                            .loading(self.busy)
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.create_channel(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("guided-cancel-channel")
                            .label("Cancelar")
                            .ghost()
                            .small()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.new_channel = None;
                                cx.notify();
                            })),
                    ),
            )
            .into_any_element()
    }

    fn render_promote(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let Some(current) = self.current() else {
            return div().into_any_element();
        };
        let (paid_links, own_links): (Vec<&AffiliateLink>, Vec<&AffiliateLink>) =
            current.links.iter().partition(|link| {
                self.channel(link.channel)
                    .is_some_and(|channel| paid(channel.kind))
            });
        let mut step = v_flex().gap_3();
        if !own_links.is_empty() {
            // One post per link: a link with one opens it below.
            let rows: Vec<AnyElement> = own_links
                .iter()
                .filter(|link| {
                    !self
                        .held
                        .posts
                        .iter()
                        .any(|post| post.draft.facts.link_id == link.id)
                })
                .enumerate()
                .map(|(index, link)| {
                    let start = StartPost {
                        link: link.id,
                        platform: link.program.platform,
                        found: None,
                        image: None,
                    };
                    table_row(&t)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(format!("Post para {}", self.channel_name(link.channel))),
                        )
                        .child(
                            Button::new(("guided-start-post", index))
                                .label("Criar post")
                                .primary()
                                .small()
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.start_post(&start, window, cx)
                                })),
                        )
                        .into_any_element()
                })
                .collect();
            step = step
                .child(div().text_sm().text_color(t.text2).child(
                    "Um post de oferta com o modelo do canal: confira os avisos e copie para \
                     colar no canal, ou publique e agende num canal do Telegram, do Instagram ou \
                     do Pinterest. Nada sai sem você confirmar.",
                ))
                .children(rows)
                .child(self.posts.clone());
        }
        if !paid_links.is_empty() {
            let text = self.ad_text.read(cx).value().to_string();
            let rows: Vec<AnyElement> = paid_links
                .iter()
                .enumerate()
                .map(|(index, link)| {
                    let shared = link.short.clone().unwrap_or_else(|| link.tagged.clone());
                    let ad = if text.trim().is_empty() {
                        shared.clone()
                    } else {
                        format!("{}\n{shared}", text.trim_end())
                    };
                    table_row(&t)
                        .flex_wrap()
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w(px(260.))
                                .child(div().font_medium().child(self.channel_name(link.channel)))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(t.text2)
                                        .truncate()
                                        .child(shared.clone()),
                                ),
                        )
                        .child(
                            Button::new(("guided-copy-ad-link", index))
                                .label("Copiar link")
                                .outline()
                                .small()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.copy(shared.clone(), "Link", cx)
                                })),
                        )
                        .child(
                            Button::new(("guided-copy-ad", index))
                                .label("Copiar texto e link")
                                .outline()
                                .small()
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.copy(ad.clone(), "Texto com o link", cx)
                                })),
                        )
                        .into_any_element()
                })
                .collect();
            step = step
                .child(kit::section_heading("Texto para o anúncio"))
                .child(div().text_sm().text_color(t.text2).child(
                    "Escreva ou cole o texto do anúncio: o app avisa quando ele promete ganho de \
                     dinheiro, perda de peso, cura, resultado garantido ou antes e depois, que a \
                     Meta, o Google e o TikTok reprovam. Depois copie o texto com o link do canal \
                     e cole no Gerenciador de Anúncios. Nada é enviado daqui.",
                ))
                .child(Textarea::new(&self.ad_text).small())
                .children(findings_notice(&text, cx))
                .children(rows);
        }
        step.into_any_element()
    }

    fn render_spend(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        v_flex()
            .gap_3()
            .child(div().text_sm().text_color(t.text2).child(
                "Lance quanto o anúncio gastou, à mão ou pelo relatório por dia da Meta ou do \
                 Google Ads, para ver o lucro e o ROAS do produto no próximo passo.",
            ))
            .child(self.spends.clone())
            .into_any_element()
    }

    fn render_result(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = look(cx).tokens;
        let Some(current) = self.current() else {
            return div().into_any_element();
        };
        let body = match &self.results {
            None => div()
                .text_sm()
                .text_color(t.text2)
                .child("Lendo o resultado…")
                .into_any_element(),
            Some(Err(error)) => {
                kit::error_notice(format!("Não consegui ler o resultado: {error}"), cx)
                    .into_any_element()
            }
            Some(Ok(results)) => self.render_figures(current, results, &t),
        };
        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .flex_wrap()
                    .gap_3()
                    .items_center()
                    .child(div().text_sm().text_color(t.text2).child("Período"))
                    .child(self.span.clone()),
            )
            .child(body)
            .when(
                !current.links.iter().any(|link| link.short.is_some()),
                |step| {
                    step.child(div().text_sm().text_color(t.text2).child(
                        "Sem link curto, o app não conta os cliques: use \"Encurtar no Short.io\" \
                     no passo Link, onde os termos permitem.",
                    ))
                },
            )
            .child(div().text_xs().text_color(t.text2).child(
                "Ganho soma as comissões aprovadas e pagas do produto; lucro é o ganho menos a \
                 parte dele no gasto dos canais, e ROAS é o ganho dividido pelo gasto, como em \
                 Afiliação. Os cliques contam só no link curto. O app só sugere pausar: nada é \
                 pausado daqui.",
            ))
            .into_any_element()
    }

    fn render_figures(&self, current: &Underway, results: &Results, t: &Tokens) -> AnyElement {
        let ids: HashSet<RecordId> = current.links.iter().map(|link| link.id).collect();
        let clicks: u64 = ClickReport::of(&results.clicks, &results.commissions)
            .by_link
            .iter()
            .filter(|group| ids.contains(&group.of))
            .map(|group| group.clicks)
            .sum();
        let shortened = current.links.iter().any(|link| link.short.is_some());
        let mut blocks: Vec<AnyElement> = Vec::new();
        for report in &results.reports {
            let product = report
                .by_product
                .iter()
                .find(|group| group.of == current.product);
            let figures = match product {
                Some(group) => product_figures(group, clicks, shortened, t),
                None => vec![
                    figure("Cliques".into(), clicks_text(clicks, shortened), t.text, t),
                    figure("Vendas".into(), "0".into(), t.text, t),
                ],
            };
            let channels: Vec<AnyElement> = report
                .by_channel
                .iter()
                .filter(|group| {
                    group.of.is_some_and(|channel| {
                        current.links.iter().any(|link| link.channel == channel)
                            && self.channel(channel).is_some_and(|c| paid(c.kind))
                    })
                })
                .map(|group| self.channel_line(group, t))
                .collect();
            blocks.push(
                v_flex()
                    .gap_2()
                    .when(results.reports.len() > 1, |block| {
                        block.child(
                            div()
                                .text_sm()
                                .font_medium()
                                .child(format!("Em {}", report.currency.code())),
                        )
                    })
                    .when(product.is_none(), |block| {
                        block.child(
                            div()
                                .text_sm()
                                .text_color(t.text2)
                                .child("Nenhuma venda deste produto no período."),
                        )
                    })
                    .child(
                        h_flex()
                            .flex_wrap()
                            .items_stretch()
                            .gap_3()
                            .children(figures),
                    )
                    .children(channels)
                    .into_any_element(),
            );
        }
        if blocks.is_empty() {
            blocks.push(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text2)
                            .child("Nenhuma venda nem gasto no período."),
                    )
                    .child(h_flex().child(figure(
                        "Cliques".into(),
                        clicks_text(clicks, shortened),
                        t.text,
                        t,
                    )))
                    .into_any_element(),
            );
        }
        v_flex().gap_3().children(blocks).into_any_element()
    }

    /// A paid channel of the product: its spend, profit and ROAS, and the
    /// pause it suggests.
    fn channel_line(&self, group: &CommissionGroup<Option<RecordId>>, t: &Tokens) -> AnyElement {
        let channel = group.of.and_then(|id| self.channel(id));
        let name = channel.map_or_else(|| "canal apagado".to_owned(), |c| c.name.clone());
        let pause = group
            .suggests_pausing()
            .then(|| pause_text(channel.map(|channel| channel.kind)));
        table_row(t)
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        div()
                            .font_medium()
                            .truncate()
                            .child(format!("Canal {name}")),
                    )
                    .child(div().text_xs().text_color(t.text2).child(format!(
                        "Gasto {} · ganho {} · lucro {} · ROAS {}",
                        group.spent.to_pt_br(),
                        group.totals.earned().to_pt_br(),
                        money_text(group.profit()),
                        roas_text(group.roas())
                    )))
                    .children(pause.map(|text| div().text_xs().text_color(t.danger).child(text))),
            )
            .into_any_element()
    }
}

/// The figures of the product's own group.
fn product_figures(
    group: &CommissionGroup<String>,
    clicks: u64,
    shortened: bool,
    t: &Tokens,
) -> Vec<AnyElement> {
    let mut figures = vec![
        figure("Cliques".into(), clicks_text(clicks, shortened), t.text, t),
        figure("Vendas".into(), group.sales.to_string(), t.text, t),
        figure("Ganho".into(), group.totals.earned().to_pt_br(), t.text, t),
        figure(
            "Pendente".into(),
            group.totals.pending.to_pt_br(),
            t.text,
            t,
        ),
    ];
    if !group.spent.amount().is_zero() {
        figures.extend([
            figure("Gasto".into(), group.spent.to_pt_br(), t.text, t),
            figure(
                "Lucro".into(),
                money_text(group.profit()),
                profit_ink(group.profit(), t),
                t,
            ),
            figure("ROAS".into(), roas_text(group.roas()), t.text, t),
            figure(
                "Lucro se as pendentes aprovarem".into(),
                money_text(group.profit_if_pending_approve()),
                profit_ink(group.profit_if_pending_approve(), t),
                t,
            ),
        ]);
    }
    figures
}

/// The clicks, or a dash when no link of the product has a Short Link to
/// count them.
fn clicks_text(clicks: u64, shortened: bool) -> String {
    if shortened {
        clicks.to_string()
    } else {
        "–".into()
    }
}

impl Render for GuidedFlowScreen {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = look(cx).tokens;
        let mut parts = ScreenParts::new("Passo a passo");
        if affiliate(cx).is_none() {
            parts
                .notices
                .push(kit::error_notice(NO_DATABASE, cx).into_any_element());
            return layout::screen(parts, cx);
        }
        parts
            .notices
            .extend(self.outcome.as_ref().map(|outcome| notice(outcome, cx)));
        let Some(open) = self.open.clone() else {
            parts.actions.push(
                Button::new("guided-start")
                    .label("Começar com outro produto")
                    .primary()
                    .small()
                    .on_click(cx.listener(|this, _, window, cx| this.start(window, cx)))
                    .into_any_element(),
            );
            parts.content.push(self.render_list(cx));
            return layout::screen(parts, cx);
        };
        let heading = open
            .product
            .clone()
            .or_else(|| {
                self.program(open.program)
                    .map(|program| program.name.clone())
            })
            .unwrap_or_else(|| "Novo produto".into());
        let paid = self.current().is_some_and(|current| current.progress.paid);
        let body = match open.step {
            Step::Product => self.render_product(&open, cx),
            Step::Link => self.render_link(&open, cx),
            Step::Promote => self.render_promote(cx),
            Step::Spend => self.render_spend(cx),
            Step::Result => self.render_result(cx),
        };
        parts.content.push(
            v_flex()
                .gap_3()
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_3()
                        .items_center()
                        .child(kit::section_heading(heading))
                        .child(kit::tag(step_text(open.step, paid), t.accent_text, cx)),
                )
                .child(self.render_steps(&open, cx))
                .into_any_element(),
        );
        parts.content.push(body);
        parts.content.push(self.render_moves(&open, cx));
        layout::screen(parts, cx)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, TimeZone};
    use mascate_affiliate::{AffiliatePlatform, ProgramRef, PromotionRules};
    use mascate_kernel::{Currency, Money, Timestamp};
    use mascate_marketing::{OfferFacts, PostDraft, SpendOrigin};
    use rust_decimal::Decimal;

    use super::*;

    #[test]
    fn the_step_is_the_first_one_with_nothing_done() {
        use Step::*;
        // linked, posted, paid, spent, sold → step
        let table = [
            ((false, false, false, false, false), Link),
            ((false, true, false, false, true), Link),
            ((true, false, false, false, false), Promote),
            ((true, true, false, false, false), Result),
            ((true, false, false, false, true), Result),
            ((true, true, false, false, true), Result),
            ((true, false, true, false, false), Promote),
            ((true, true, true, false, false), Spend),
            ((true, false, true, false, true), Spend),
            ((true, true, true, false, true), Spend),
            ((true, false, true, true, false), Result),
            ((true, true, true, true, false), Result),
            ((true, false, true, true, true), Result),
            ((true, true, true, true, true), Result),
        ];
        for ((linked, posted, paid, spent, sold), step) in table {
            let progress = Progress {
                linked,
                posted,
                paid,
                spent,
                sold,
            };
            assert_eq!(progress.step(), step, "{progress:?}");
        }
    }

    #[test]
    fn an_organic_channel_skips_the_spend_step() {
        use Step::*;
        assert_eq!(Step::path(false), [Product, Link, Promote, Result]);
        assert_eq!(Step::path(true), [Product, Link, Promote, Spend, Result]);
        assert_eq!(step_text(Result, false), "Passo 4 de 4 · Resultado");
        assert_eq!(step_text(Spend, true), "Passo 4 de 5 · Gasto");
        let posted = Progress {
            linked: true,
            posted: true,
            ..Progress::default()
        };
        assert_eq!(posted.step(), Result);
    }

    fn at(day: u32) -> Timestamp {
        Utc.with_ymd_and_hms(2026, 10, day, 15, 0, 0).unwrap()
    }

    fn id(n: u128) -> RecordId {
        RecordId::from_u128(n)
    }

    fn program(n: u128, product: Option<&str>) -> Program {
        Program {
            id: id(n),
            platform: AffiliatePlatform::Hotmart,
            name: format!("Programa {n}"),
            rules: PromotionRules::defaults(AffiliatePlatform::Hotmart),
            provisional: product.is_some(),
            product: product.map(str::to_owned),
        }
    }

    fn channel(n: u128, kind: ChannelKind) -> PromotionChannel {
        PromotionChannel {
            id: id(n),
            name: format!("Canal {n}"),
            kind,
            default_tag: format!("canal{n}"),
            keywords: Vec::new(),
            telegram_chat: None,
            posting_times: Vec::new(),
            pinterest_board: None,
        }
    }

    fn link(n: u128, program: &Program, product: &str, channel: u128, day: u32) -> AffiliateLink {
        AffiliateLink {
            id: id(n),
            program: ProgramRef {
                id: program.id,
                platform: program.platform,
                name: program.name.clone(),
            },
            product: product.into(),
            channel: id(channel),
            tag: format!("canal{channel}"),
            pasted: "https://go.hotmart.com/X1".into(),
            tagged: format!("https://go.hotmart.com/X1?src=canal{channel}"),
            short: None,
            created_at: at(day),
        }
    }

    fn post(link: &AffiliateLink, status: PostStatus, copied: bool) -> OfferPost {
        OfferPost {
            id: id(900),
            draft: PostDraft {
                channel: link.channel,
                facts: OfferFacts {
                    link_id: link.id,
                    link: link.tagged.clone(),
                    product: link.product.clone(),
                    platform: "Hotmart".into(),
                    price: None,
                    image: None,
                    price_needs_time: false,
                    forbidden_contacts: Vec::new(),
                },
                text: "Curso".into(),
            },
            status,
            copied_at: copied.then(|| at(12)),
            scheduled_at: None,
            sent: None,
            failure: None,
            created_at: at(12),
            updated_at: at(12),
        }
    }

    fn spend(channel: u128, last_day: u32) -> PromotionSpend {
        PromotionSpend {
            id: id(800),
            channel: id(channel),
            first_day: NaiveDate::from_ymd_opt(2026, 10, 1).unwrap(),
            last_day: NaiveDate::from_ymd_opt(2026, 10, last_day).unwrap(),
            amount: Money::new(Decimal::from(50), Currency::Brl),
            origin: SpendOrigin::Typed,
            campaign: None,
        }
    }

    fn commission(
        program: &Program,
        product: &str,
        link: Option<RecordId>,
        day: u32,
    ) -> Commission {
        Commission {
            id: id(700),
            platform: program.platform,
            program: program.id,
            program_name: program.name.clone(),
            product: product.into(),
            sold_at: at(day),
            amount: Money::new(Decimal::from(30), Currency::Brl),
            status: CommissionStatus::Approved,
            platform_status: String::new(),
            status_since: at(day),
            tracking: None,
            manual: false,
            payout: None,
            linked_product: None,
            channel: None,
            link,
        }
    }

    #[test]
    fn a_program_added_before_its_link_waits_for_the_link() {
        let added = program(1, Some(" Curso de Fotografia "));
        let held = Held {
            programs: vec![added.clone(), program(2, None)],
            ..Held::default()
        };

        let products = underway(&held);

        assert_eq!(
            products.len(),
            1,
            "a Program without product or link is not listed"
        );
        assert_eq!(products[0].product, "Curso de Fotografia");
        assert_eq!(products[0].progress, Progress::default());
        assert_eq!(products[0].progress.step(), Step::Link);
    }

    #[test]
    fn each_product_of_a_program_goes_its_own_way() {
        let hotmart = program(1, Some("Curso A"));
        let organic = channel(10, ChannelKind::TelegramChannel);
        let meta = channel(11, ChannelKind::PaidSocial);
        let a_on_telegram = link(20, &hotmart, "Curso A", 10, 10);
        let b_on_meta = link(21, &hotmart, "Curso B", 11, 10);
        let mut held = Held {
            programs: vec![hotmart.clone()],
            links: vec![a_on_telegram.clone(), b_on_meta.clone()],
            channels: vec![organic, meta],
            ..Held::default()
        };
        let steps = |held: &Held| -> Vec<(String, Step)> {
            underway(held)
                .into_iter()
                .map(|underway| (underway.product, underway.progress.step()))
                .collect()
        };
        assert_eq!(
            steps(&held),
            [
                ("Curso A".into(), Step::Promote),
                ("Curso B".into(), Step::Promote)
            ]
        );

        // A draft is not divulged; a copied one is.
        held.posts = vec![post(&a_on_telegram, PostStatus::Draft, false)];
        assert_eq!(steps(&held)[0].1, Step::Promote);
        held.posts = vec![post(&a_on_telegram, PostStatus::Draft, true)];
        assert_eq!(steps(&held)[0].1, Step::Result);
        held.posts = vec![post(&a_on_telegram, PostStatus::Scheduled, false)];
        assert_eq!(steps(&held)[0].1, Step::Result);
        held.posts = vec![post(&a_on_telegram, PostStatus::Failed, false)];
        assert_eq!(steps(&held)[0].1, Step::Promote);

        // A sale through the paid link waits for its spend; a spend from
        // before the link was created is another product's.
        held.commissions = vec![commission(&hotmart, "Curso B", Some(b_on_meta.id), 11)];
        assert_eq!(steps(&held)[1].1, Step::Spend);
        held.spends = vec![spend(11, 9)];
        assert_eq!(steps(&held)[1].1, Step::Spend);
        held.spends = vec![spend(11, 10)];
        assert_eq!(steps(&held)[1].1, Step::Result);
    }

    #[test]
    fn a_sale_counts_only_once_the_link_exists_and_never_cancelled() {
        let hotmart = program(1, None);
        let telegram = channel(10, ChannelKind::TelegramChannel);
        let on_telegram = link(20, &hotmart, "Curso A", 10, 10);
        let mut held = Held {
            programs: vec![hotmart.clone()],
            links: vec![on_telegram],
            channels: vec![telegram],
            ..Held::default()
        };
        let step = |held: &Held| underway(held)[0].progress.step();

        held.commissions = vec![commission(&hotmart, "Curso A", None, 9)];
        assert_eq!(step(&held), Step::Promote, "sold before the link");
        let mut cancelled = commission(&hotmart, "Curso A", None, 11);
        cancelled.status = CommissionStatus::Cancelled;
        held.commissions = vec![cancelled];
        assert_eq!(step(&held), Step::Promote);
        // Matched by Program and product, as a sale without the marking.
        held.commissions = vec![commission(&hotmart, "Curso A", None, 11)];
        assert_eq!(step(&held), Step::Result);
    }
}
