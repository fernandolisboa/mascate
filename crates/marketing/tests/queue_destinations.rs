//! The posting queue on every destination (#157, ADR 0035) through the
//! Marketing module's public interface, with a real temporary database, an
//! injected clock and pause and scripted destinations: a post scheduled on
//! a Telegram, Instagram or Pinterest channel goes out at its time through
//! its own destination, at that destination's pace; a destination at its
//! limit gets no request until it frees; and pausing, lateness and an
//! expired price hold on every kind.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeDelta, TimeZone};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, Currency, Money, Pause, PlatformError, RecordId, Timestamp, brasilia};
use mascate_marketing::{
    CHAT_SPACING, ChannelKind, ConfirmedPost, DestinationCheck, DestinationLimit,
    INSTAGRAM_POSTS_PER_DAY, MIGRATIONS, NewPromotionChannel, OfferFacts, OfferPost, OfferPosts,
    OfferPrice, PinterestBoard, PostDestination, PostDestinations, PostError, PostImage,
    PostStatus, PostTime, PromotionChannel, PromotionChannels, PublishFailure, PublishedPost,
    QUOTA_RECHECK_MINUTES, QueueRound, RATE_LIMIT_WAIT_MINUTES, default_posting_times,
};
use mascate_platform::{Database, migrate};

const CHAT: &str = "@ofertasdofernando";
const BOARD: &str = "549755885175";
const CONTAINER: &str = "17889455560051444";

/// October `day`, 2026, at `hour:minute` on Brasília's clock.
fn brt(day: u32, hour: u32, minute: u32) -> Timestamp {
    brasilia()
        .with_ymd_and_hms(2026, 10, day, hour, minute, 0)
        .unwrap()
        .to_utc()
}

/// What a destination received.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Sent {
    destination: &'static str,
    chat: String,
    title: String,
    staged: Option<String>,
}

fn sent(destination: &'static str, chat: &str, title: &str) -> Sent {
    Sent {
        destination,
        chat: chat.into(),
        title: title.into(),
        staged: None,
    }
}

/// Publishes whatever it gets, answering each call with the next scripted
/// outcome or, past the script, a sent post, and writes each call in a log
/// it may share with other destinations. One that publishes in two steps
/// readies [`CONTAINER`] first.
#[derive(Clone)]
struct Destination {
    name: &'static str,
    two_steps: bool,
    answers: Arc<Mutex<VecDeque<Result<PublishedPost, PublishFailure>>>>,
    log: Arc<Mutex<Vec<Sent>>>,
}

impl Destination {
    fn new(name: &'static str, log: &Arc<Mutex<Vec<Sent>>>) -> Self {
        Self {
            name,
            two_steps: name == "instagram",
            answers: Arc::default(),
            log: log.clone(),
        }
    }

    fn answering(self, answers: Vec<Result<PublishedPost, PublishFailure>>) -> Self {
        *self.answers.lock().unwrap() = answers.into();
        self
    }
}

impl PostDestination for Destination {
    fn check(&self, _chat: &str) -> Result<DestinationCheck, PlatformError> {
        unreachable!("the queue never checks a chat")
    }

    fn stage(&self, _post: &ConfirmedPost) -> Result<Option<String>, PublishFailure> {
        Ok(self.two_steps.then(|| CONTAINER.to_owned()))
    }

    fn publish(&self, post: &ConfirmedPost) -> Result<PublishedPost, PublishFailure> {
        let mut log = self.log.lock().unwrap();
        log.push(Sent {
            destination: self.name,
            chat: post.chat().to_owned(),
            title: post.title().to_owned(),
            staged: post.staged().map(str::to_owned),
        });
        let number = log.len();
        self.answers.lock().unwrap().pop_front().unwrap_or_else(|| {
            Ok(PublishedPost {
                message_id: number.to_string(),
                link: None,
            })
        })
    }
}

/// Moves the clock on by each pause, and remembers it.
struct ClockPause {
    clock: Arc<ManualClock>,
    pauses: Mutex<Vec<Duration>>,
}

impl Pause for ClockPause {
    fn pause(&self, duration: Duration) {
        self.pauses.lock().unwrap().push(duration);
        self.clock
            .advance(TimeDelta::from_std(duration).expect("a short pause"));
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    channels: PromotionChannels,
    posts: OfferPosts,
    pause: ClockPause,
    log: Arc<Mutex<Vec<Sent>>>,
    telegram: Destination,
    instagram: Destination,
    pinterest: Destination,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(brt(14, 9, 0)));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        let ids = Arc::new(SequentialIds::default());
        let log = Arc::default();
        Self {
            _dir: dir,
            channels: PromotionChannels::new(database.clone(), clock.clone(), ids.clone()),
            posts: OfferPosts::new(database, clock.clone(), ids),
            pause: ClockPause {
                clock: clock.clone(),
                pauses: Mutex::new(Vec::new()),
            },
            clock,
            telegram: Destination::new("telegram", &log),
            instagram: Destination::new("instagram", &log),
            pinterest: Destination::new("pinterest", &log),
            log,
        }
    }

    async fn channel(&self, kind: ChannelKind, tag: &str) -> PromotionChannel {
        let channel = self
            .channels
            .create(&NewPromotionChannel {
                name: format!("Canal {tag}"),
                kind,
                default_tag: tag.into(),
                keywords: Vec::new(),
                telegram_chat: (kind == ChannelKind::TelegramChannel).then(|| CHAT.into()),
            })
            .await
            .unwrap();
        if kind != ChannelKind::Pinterest {
            return channel;
        }
        self.channels
            .set_pinterest_board(
                channel.id,
                &PinterestBoard {
                    id: BOARD.into(),
                    name: "Ofertas".into(),
                },
            )
            .await
            .unwrap()
    }

    async fn post(&self, channel: &PromotionChannel, facts: &OfferFacts) -> OfferPost {
        let draft = self.posts.draft(facts, channel).await.unwrap();
        self.posts.create(&draft).await.unwrap()
    }

    /// The owner reads the post, confirms it and schedules it for `at`.
    async fn schedule(&self, post: RecordId, at: Timestamp) -> Result<OfferPost, PostError> {
        let request = self.posts.prepare(post).await?;
        self.posts
            .schedule(request.confirm(), PostTime::At(at))
            .await
    }

    async fn status(&self, post: RecordId) -> PostStatus {
        self.posts.post(post).await.unwrap().status
    }

    fn set_clock(&self, at: Timestamp) {
        self.clock.advance(at - self.clock.now());
    }

    /// A turn of the queue with every destination connected.
    async fn send_due(&self) -> QueueRound {
        let destinations = PostDestinations::default()
            .with(
                ChannelKind::TelegramChannel,
                Arc::new(self.telegram.clone()),
            )
            .with(ChannelKind::Instagram, Arc::new(self.instagram.clone()))
            .with(ChannelKind::Pinterest, Arc::new(self.pinterest.clone()));
        self.send_through(&destinations).await
    }

    async fn send_through(&self, destinations: &PostDestinations) -> QueueRound {
        self.posts
            .send_due(destinations, &self.pause)
            .await
            .unwrap()
    }

    fn log(&self) -> Vec<Sent> {
        self.log.lock().unwrap().clone()
    }

    fn pauses(&self) -> Vec<Duration> {
        self.pause.pauses.lock().unwrap().clone()
    }
}

/// An offer with a photo on the web, which every destination takes.
fn facts(product: &str) -> OfferFacts {
    OfferFacts {
        link_id: RecordId::from_u128(900),
        link: "https://go.hotmart.com/X12345678Y?src=mascate".into(),
        product: product.into(),
        platform: "Hotmart".into(),
        price: None,
        image: Some(PostImage::Url("https://cf.shopee.com.br/file/abc".into())),
        price_needs_time: false,
        forbidden_contacts: Vec::new(),
    }
}

#[test]
fn a_post_scheduled_on_each_kind_goes_out_at_its_time_through_its_own_destination() {
    block_on(async {
        let f = Fixture::new().await;
        let mut posts = Vec::new();
        for (kind, tag, product) in [
            (ChannelKind::TelegramChannel, "telegram", "Fone Bluetooth"),
            (ChannelKind::Instagram, "insta", "Cafeteira"),
            (ChannelKind::Pinterest, "pins", "Curso de Fotografia"),
        ] {
            let channel = f.channel(kind, tag).await;
            assert_eq!(channel.posting_times, default_posting_times(), "{kind:?}");
            let post = f.post(&channel, &facts(product)).await;
            let scheduled = f
                .posts
                .schedule(
                    f.posts.prepare(post.id).await.unwrap().confirm(),
                    PostTime::NextFree,
                )
                .await
                .unwrap();
            assert_eq!(scheduled.scheduled_at, Some(brt(14, 12, 0)), "{kind:?}");
            posts.push(post);
        }

        f.set_clock(brt(14, 11, 59));
        assert!(f.send_due().await.posts.is_empty());
        assert!(f.log().is_empty());

        f.set_clock(brt(14, 12, 0));
        let round = f.send_due().await;
        assert_eq!(round.posts.len(), 3);
        assert_eq!(
            f.log(),
            vec![
                sent("telegram", CHAT, "Fone Bluetooth"),
                // Instagram readied the post before publishing it, as on
                // "Publicar agora".
                Sent {
                    staged: Some(CONTAINER.into()),
                    ..sent("instagram", "", "Cafeteira")
                },
                sent("pinterest", BOARD, "Curso de Fotografia"),
            ]
        );
        for post in &posts {
            assert_eq!(f.status(post.id).await, PostStatus::Sent);
        }
        // Once each.
        f.clock.advance(TimeDelta::minutes(1));
        f.send_due().await;
        assert_eq!(f.log().len(), 3);
        // Different destinations do not wait for each other.
        assert!(f.pauses().is_empty());
    });
}

#[test]
fn posts_to_one_account_go_out_a_few_seconds_apart() {
    block_on(async {
        let f = Fixture::new().await;
        // Two Instagram channels post to the one connected account, and two
        // Pinterest channels through the one Pinterest login.
        for (kind, tags) in [
            (ChannelKind::Instagram, ["insta_a", "insta_b"]),
            (ChannelKind::Pinterest, ["pins_a", "pins_b"]),
        ] {
            for tag in tags {
                let channel = f.channel(kind, tag).await;
                let post = f.post(&channel, &facts(tag)).await;
                f.schedule(post.id, brt(14, 12, 0)).await.unwrap();
            }
        }

        f.set_clock(brt(14, 12, 0));
        let round = f.send_due().await;

        assert_eq!(round.posts.len(), 4);
        assert_eq!(f.pauses(), vec![CHAT_SPACING, CHAT_SPACING]);
    });
}

#[test]
fn a_destination_without_its_connection_keeps_its_posts_waiting_until_they_are_late() {
    block_on(async {
        let f = Fixture::new().await;
        let telegram = f.channel(ChannelKind::TelegramChannel, "telegram").await;
        let instagram = f.channel(ChannelKind::Instagram, "insta").await;
        let on_telegram = f.post(&telegram, &facts("Fone Bluetooth")).await;
        let on_instagram = f.post(&instagram, &facts("Cafeteira")).await;
        f.schedule(on_telegram.id, brt(14, 12, 0)).await.unwrap();
        f.schedule(on_instagram.id, brt(14, 12, 0)).await.unwrap();
        let only_telegram = PostDestinations::default()
            .with(ChannelKind::TelegramChannel, Arc::new(f.telegram.clone()));

        f.set_clock(brt(14, 12, 0));
        f.send_through(&only_telegram).await;
        assert_eq!(f.log(), vec![sent("telegram", CHAT, "Fone Bluetooth")]);
        assert_eq!(f.status(on_instagram.id).await, PostStatus::Scheduled);

        f.set_clock(brt(14, 12, 31));
        let round = f.send_through(&only_telegram).await;
        assert_eq!(round.held, 1);
        assert_eq!(f.status(on_instagram.id).await, PostStatus::Late);
    });
}

#[test]
fn an_instagram_post_at_quota_waits_for_the_next_window_without_a_request() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::Instagram, "insta").await;
        let telegram = f.channel(ChannelKind::TelegramChannel, "telegram").await;
        // The day's quota goes on posts published now, a second apart.
        for number in 0..INSTAGRAM_POSTS_PER_DAY {
            let post = f.post(&channel, &facts(&format!("Oferta {number}"))).await;
            let request = f.posts.prepare(post.id).await.unwrap();
            let published = f
                .posts
                .publish(&f.instagram, request.confirm())
                .await
                .unwrap();
            assert_eq!(published.status, PostStatus::Sent);
            f.clock.advance(TimeDelta::seconds(1));
        }
        let waiting = f.post(&channel, &facts("Cafeteira")).await;
        f.schedule(waiting.id, brt(15, 8, 50)).await.unwrap();
        let on_telegram = f.post(&telegram, &facts("Fone Bluetooth")).await;
        f.schedule(on_telegram.id, brt(15, 8, 50)).await.unwrap();
        let frees = brt(15, 9, 0);

        f.set_clock(brt(15, 8, 50));
        let round = f.send_due().await;
        assert_eq!(
            round.limited,
            vec![DestinationLimit {
                kind: ChannelKind::Instagram,
                until: frees,
            }]
        );
        assert_eq!(f.log().len(), INSTAGRAM_POSTS_PER_DAY + 1);
        assert_eq!(f.log().last().unwrap().destination, "telegram");
        assert_eq!(f.status(waiting.id).await, PostStatus::Scheduled);
        let queue = f.posts.queue().await.unwrap();
        assert_eq!(queue.limited, round.limited);
        assert!(queue.scheduled.iter().any(|post| post.id == waiting.id));

        f.set_clock(brt(15, 8, 59));
        f.send_due().await;
        assert_eq!(f.log().len(), INSTAGRAM_POSTS_PER_DAY + 1);

        // The first post of the full day turns a day old: room for one.
        f.set_clock(frees);
        let round = f.send_due().await;
        assert!(round.limited.is_empty());
        assert_eq!(f.log().last().unwrap().title, "Cafeteira");
        assert_eq!(f.status(waiting.id).await, PostStatus::Sent);
        // That post fills the quota again, until the next oldest turns a
        // day old.
        assert_eq!(
            f.posts.queue().await.unwrap().limited,
            vec![DestinationLimit {
                kind: ChannelKind::Instagram,
                until: frees + TimeDelta::seconds(1),
            }]
        );
    });
}

#[test]
fn a_quota_instagram_reports_holds_the_account_for_a_while_and_nothing_goes_out_twice() {
    block_on(async {
        let f = Fixture::new().await;
        let first = f.channel(ChannelKind::Instagram, "insta_a").await;
        let second = f.channel(ChannelKind::Instagram, "insta_b").await;
        let telegram = f.channel(ChannelKind::TelegramChannel, "telegram").await;
        let turned_away = f.post(&first, &facts("Cafeteira")).await;
        let after = f.post(&second, &facts("Liquidificador")).await;
        let on_telegram = f.post(&telegram, &facts("Fone Bluetooth")).await;
        f.schedule(turned_away.id, brt(14, 12, 0)).await.unwrap();
        f.schedule(after.id, brt(14, 12, 10)).await.unwrap();
        f.schedule(on_telegram.id, brt(14, 12, 10)).await.unwrap();
        let instagram = f
            .instagram
            .clone()
            .answering(vec![Err(PublishFailure::QuotaUsed)]);
        let destinations = PostDestinations::default()
            .with(ChannelKind::Instagram, Arc::new(instagram))
            .with(ChannelKind::TelegramChannel, Arc::new(f.telegram.clone()));
        let held_until = brt(14, 12, 0) + TimeDelta::minutes(QUOTA_RECHECK_MINUTES);

        f.set_clock(brt(14, 12, 0));
        let round = f.send_through(&destinations).await;
        assert_eq!(
            round.limited,
            vec![DestinationLimit {
                kind: ChannelKind::Instagram,
                until: held_until,
            }]
        );
        // Turned away, nothing went out: back in the queue at its time.
        let requeued = f.posts.post(turned_away.id).await.unwrap();
        assert_eq!(requeued.status, PostStatus::Scheduled);
        assert_eq!(requeued.scheduled_at, Some(brt(14, 12, 0)));
        assert_eq!(requeued.failure, None);
        assert_eq!(f.log().len(), 1);

        f.set_clock(brt(14, 12, 10));
        f.send_through(&destinations).await;
        assert_eq!(
            f.log()[1..],
            [sent("telegram", CHAT, "Fone Bluetooth")],
            "only Telegram was asked"
        );
        assert_eq!(f.status(after.id).await, PostStatus::Scheduled);

        // Past the late limit they wait for the owner, as on Telegram.
        f.set_clock(brt(14, 12, 41));
        f.send_through(&destinations).await;
        assert_eq!(f.status(turned_away.id).await, PostStatus::Late);
        assert_eq!(f.status(after.id).await, PostStatus::Late);
        assert_eq!(f.log().len(), 2);

        // Once the hold is over, a post due goes out.
        let next = f.post(&first, &facts("Panela de pressão")).await;
        f.schedule(next.id, held_until).await.unwrap();
        f.set_clock(held_until);
        f.send_through(&destinations).await;
        assert_eq!(f.status(next.id).await, PostStatus::Sent);
    });
}

#[test]
fn pinterest_asking_to_wait_holds_it_and_the_pin_goes_out_after() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::Pinterest, "pins").await;
        let post = f.post(&channel, &facts("Curso de Fotografia")).await;
        f.schedule(post.id, brt(14, 12, 0)).await.unwrap();
        let pinterest = f
            .pinterest
            .clone()
            .answering(vec![Err(PublishFailure::RateLimited)]);
        let destinations =
            PostDestinations::default().with(ChannelKind::Pinterest, Arc::new(pinterest));
        let waits_until = brt(14, 12, 0) + TimeDelta::minutes(RATE_LIMIT_WAIT_MINUTES);

        f.set_clock(brt(14, 12, 0));
        let round = f.send_through(&destinations).await;
        assert_eq!(
            round.limited,
            vec![DestinationLimit {
                kind: ChannelKind::Pinterest,
                until: waits_until,
            }]
        );
        assert_eq!(f.status(post.id).await, PostStatus::Scheduled);

        f.set_clock(waits_until - TimeDelta::minutes(1));
        f.send_through(&destinations).await;
        assert_eq!(f.log().len(), 1);

        f.set_clock(waits_until);
        f.send_through(&destinations).await;
        assert_eq!(f.log().len(), 2);
        assert_eq!(f.status(post.id).await, PostStatus::Sent);
    });
}

#[test]
fn a_refused_login_stops_only_its_own_destination() {
    block_on(async {
        let f = Fixture::new().await;
        let pins = f.channel(ChannelKind::Pinterest, "pins").await;
        let more_pins = f.channel(ChannelKind::Pinterest, "pins_b").await;
        let instagram = f.channel(ChannelKind::Instagram, "insta").await;
        let refused = f.post(&pins, &facts("Curso de Fotografia")).await;
        let left = f.post(&more_pins, &facts("Tripé")).await;
        let on_instagram = f.post(&instagram, &facts("Cafeteira")).await;
        for post in [&refused, &left, &on_instagram] {
            f.schedule(post.id, brt(14, 12, 0)).await.unwrap();
        }
        let pinterest = f
            .pinterest
            .clone()
            .answering(vec![Err(PublishFailure::Expired)]);
        let destinations = PostDestinations::default()
            .with(ChannelKind::Pinterest, Arc::new(pinterest))
            .with(ChannelKind::Instagram, Arc::new(f.instagram.clone()));

        f.set_clock(brt(14, 12, 0));
        let round = f.send_through(&destinations).await;

        assert_eq!(round.expired, vec![ChannelKind::Pinterest]);
        assert_eq!(f.status(refused.id).await, PostStatus::Failed);
        assert_eq!(f.status(left.id).await, PostStatus::Scheduled);
        assert_eq!(f.status(on_instagram.id).await, PostStatus::Sent);
    });
}

#[test]
fn pause_holds_posts_of_every_kind_and_resume_sends_them_in_order() {
    block_on(async {
        let f = Fixture::new().await;
        let mut posts = Vec::new();
        for (minute, kind, tag) in [
            (2, ChannelKind::TelegramChannel, "telegram"),
            (0, ChannelKind::Pinterest, "pins"),
            (1, ChannelKind::Instagram, "insta"),
        ] {
            let channel = f.channel(kind, tag).await;
            let post = f.post(&channel, &facts(tag)).await;
            f.schedule(post.id, brt(14, 12, minute)).await.unwrap();
            posts.push(post);
        }
        f.posts.set_queue_paused(true).await.unwrap();

        f.set_clock(brt(14, 12, 5));
        let round = f.send_due().await;
        assert!(round.paused);
        assert!(f.log().is_empty());
        for post in &posts {
            assert_eq!(f.status(post.id).await, PostStatus::Scheduled);
        }

        f.posts.set_queue_paused(false).await.unwrap();
        f.send_due().await;
        let order: Vec<&str> = f.log().iter().map(|sent| sent.destination).collect();
        assert_eq!(order, ["pinterest", "instagram", "telegram"]);
    });
}

#[test]
fn late_and_expired_price_rules_hold_on_instagram_and_pinterest() {
    block_on(async {
        let f = Fixture::new().await;
        let instagram = f.channel(ChannelKind::Instagram, "insta").await;
        let pins = f.channel(ChannelKind::Pinterest, "pins").await;
        let late = f.post(&instagram, &facts("Cafeteira")).await;
        f.schedule(late.id, brt(14, 12, 0)).await.unwrap();
        f.set_clock(brt(14, 12, 31));
        let round = f.send_due().await;
        assert_eq!(round.held, 1);
        assert_eq!(f.status(late.id).await, PostStatus::Late);

        let seen_at = f.clock.now();
        let amazon = OfferFacts {
            link: "https://www.amazon.com.br/dp/B0ABCDEFGH?tag=mascate-pin-20".into(),
            platform: "Amazon".into(),
            price: Some(OfferPrice {
                price: Money::new(rust_decimal::Decimal::new(39900, 2), Currency::Brl),
                before: None,
                seen_at,
            }),
            price_needs_time: true,
            ..facts("Kindle 11ª geração")
        };
        let priced = f.post(&pins, &amazon).await;
        assert!(matches!(
            f.schedule(priced.id, seen_at + TimeDelta::hours(25)).await,
            Err(PostError::OldPrice)
        ));
        let at = seen_at + TimeDelta::hours(23) + TimeDelta::minutes(59);
        f.schedule(priced.id, at).await.unwrap();
        f.set_clock(seen_at + TimeDelta::hours(24) + TimeDelta::minutes(1));
        f.send_due().await;
        assert_eq!(f.status(priced.id).await, PostStatus::PriceExpired);
        assert!(f.log().is_empty());
    });
}

#[test]
fn instagram_and_pinterest_channels_keep_posting_times_and_a_new_kind_unschedules() {
    block_on(async {
        let f = Fixture::new().await;
        let pins = f.channel(ChannelKind::Pinterest, "pins").await;
        let times = [
            chrono::NaiveTime::from_hms_opt(10, 30, 0).unwrap(),
            chrono::NaiveTime::from_hms_opt(20, 0, 0).unwrap(),
        ];
        let kept = f.channels.set_posting_times(pins.id, &times).await.unwrap();
        assert_eq!(kept.posting_times, times);
        assert_eq!(
            f.posts.next_free(pins.id, None).await.unwrap(),
            Some(brt(14, 10, 30))
        );

        let instagram = f.channel(ChannelKind::Instagram, "insta").await;
        let post = f.post(&instagram, &facts("Cafeteira")).await;
        f.schedule(post.id, brt(14, 12, 0)).await.unwrap();
        f.channels
            .update(
                instagram.id,
                &NewPromotionChannel {
                    name: instagram.name.clone(),
                    kind: ChannelKind::TikTok,
                    default_tag: instagram.default_tag.clone(),
                    keywords: Vec::new(),
                    telegram_chat: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(f.status(post.id).await, PostStatus::Draft);
    });
}
