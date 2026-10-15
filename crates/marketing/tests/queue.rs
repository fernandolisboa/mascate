//! The Telegram queue through the Marketing module's public interface, with
//! a real temporary database, an injected clock and pause and a recording
//! destination: a confirmed post goes out at its time once, a few seconds
//! apart from the last one in its chat, and nothing late, with an expired
//! price, edited since or paused goes out on its own.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{NaiveDate, NaiveTime, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, Currency, Money, Pause, PlatformError, RecordId, Timestamp, brasilia};
use mascate_marketing::{
    CHAT_SPACING, ChannelError, ChannelKind, ConfirmedPost, DestinationCheck, MIGRATIONS,
    NewPromotionChannel, OfferFacts, OfferPost, OfferPosts, OfferPrice, PostDestination, PostError,
    PostStatus, PostTime, PromotionChannel, PromotionChannels, PublishFailure, PublishedPost,
    default_posting_times, next_free_time,
};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;

const CHAT: &str = "@ofertasdofernando";
const OTHER_CHAT: &str = "@achadosdofernando";

/// October `day`, 2026, at `hour:minute` on Brasília's clock; the 14th is a
/// Wednesday.
fn brt(day: u32, hour: u32, minute: u32) -> Timestamp {
    brasilia()
        .with_ymd_and_hms(2026, 10, day, hour, minute, 0)
        .unwrap()
        .to_utc()
}

fn time(hour: u32, minute: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(hour, minute, 0).unwrap()
}

/// Publishes whatever it gets, answering each call with the next scripted
/// outcome or, past the script, a sent post.
#[derive(Default)]
struct Recorder {
    answers: Mutex<VecDeque<Result<PublishedPost, PublishFailure>>>,
    calls: Mutex<Vec<(String, String)>>,
}

impl Recorder {
    fn answering(answers: Vec<Result<PublishedPost, PublishFailure>>) -> Self {
        Self {
            answers: Mutex::new(answers.into()),
            ..Self::default()
        }
    }

    /// Each call's chat and text, in order.
    fn calls(&self) -> Vec<(String, String)> {
        self.calls.lock().unwrap().clone()
    }
}

impl PostDestination for Recorder {
    fn check(&self, _chat: &str) -> Result<DestinationCheck, PlatformError> {
        unreachable!("the queue never checks a chat")
    }

    fn publish(&self, post: &ConfirmedPost) -> Result<PublishedPost, PublishFailure> {
        let mut calls = self.calls.lock().unwrap();
        calls.push((post.chat().to_owned(), post.text().to_owned()));
        let number = calls.len();
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

impl ClockPause {
    fn pauses(&self) -> Vec<Duration> {
        self.pauses.lock().unwrap().clone()
    }
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
        Self {
            _dir: dir,
            channels: PromotionChannels::new(database.clone(), clock.clone(), ids.clone()),
            posts: OfferPosts::new(database, clock.clone(), ids),
            pause: ClockPause {
                clock: clock.clone(),
                pauses: Mutex::new(Vec::new()),
            },
            clock,
        }
    }

    async fn channel(&self, tag: &str, chat: &str) -> PromotionChannel {
        self.channels
            .create(&NewPromotionChannel {
                name: format!("Canal {tag}"),
                kind: ChannelKind::TelegramChannel,
                default_tag: tag.into(),
                keywords: Vec::new(),
                telegram_chat: Some(chat.into()),
            })
            .await
            .unwrap()
    }

    async fn post(&self, channel: &PromotionChannel, facts: &OfferFacts) -> OfferPost {
        let draft = self.posts.draft(facts, channel).await.unwrap();
        self.posts.create(&draft).await.unwrap()
    }

    /// The owner reads the post, confirms it and schedules it for `when`.
    async fn schedule(&self, post: RecordId, when: PostTime) -> Result<OfferPost, PostError> {
        let request = self.posts.prepare(post).await?;
        self.posts.schedule(request.confirm(), when).await
    }

    async fn status(&self, post: RecordId) -> PostStatus {
        self.posts.post(post).await.unwrap().status
    }

    fn set_clock(&self, at: Timestamp) {
        self.clock.advance(at - self.clock.now());
    }

    async fn send_due(&self, destination: &Recorder) -> mascate_marketing::QueueRound {
        self.posts.send_due(destination, &self.pause).await.unwrap()
    }
}

fn facts(product: &str) -> OfferFacts {
    OfferFacts {
        link_id: RecordId::from_u128(900),
        link: "https://go.hotmart.com/X12345678Y?src=telegram".into(),
        product: product.into(),
        platform: "Hotmart".into(),
        price: None,
        image: None,
        price_needs_time: false,
        forbidden_contacts: Vec::new(),
    }
}

/// Amazon's facts, with the price seen at `seen_at`.
fn amazon(seen_at: Timestamp) -> OfferFacts {
    OfferFacts {
        link: "https://www.amazon.com.br/dp/B0ABCDEFGH?tag=mascate-tg-20".into(),
        platform: "Amazon".into(),
        price: Some(OfferPrice {
            price: Money::new(rust_decimal::Decimal::new(39900, 2), Currency::Brl),
            before: None,
            seen_at,
        }),
        price_needs_time: true,
        ..facts("Kindle 11ª geração")
    }
}

#[test]
fn due_posts_go_out_in_schedule_order_once_each_a_few_seconds_apart_per_chat() {
    block_on(async {
        let f = Fixture::new().await;
        let ours = f.channel("telegram", CHAT).await;
        let other = f.channel("achados", OTHER_CHAT).await;
        let later = f.post(&ours, &facts("Curso de Fotografia")).await;
        let first = f.post(&ours, &facts("Fone Bluetooth")).await;
        let elsewhere = f.post(&other, &facts("Kit de Pincéis")).await;
        let future = f.post(&ours, &facts("Cafeteira")).await;
        f.schedule(later.id, PostTime::At(brt(14, 12, 10)))
            .await
            .unwrap();
        f.schedule(first.id, PostTime::At(brt(14, 12, 0)))
            .await
            .unwrap();
        f.schedule(elsewhere.id, PostTime::At(brt(14, 12, 5)))
            .await
            .unwrap();
        f.schedule(future.id, PostTime::At(brt(14, 19, 0)))
            .await
            .unwrap();
        let destination = Recorder::default();

        // Before its time, nothing goes out.
        f.set_clock(brt(14, 11, 59));
        let round = f.send_due(&destination).await;
        assert!(round.posts.is_empty());
        assert!(destination.calls().is_empty());

        f.set_clock(brt(14, 12, 15));
        assert_eq!(
            f.posts
                .due(f.clock.now())
                .await
                .unwrap()
                .iter()
                .map(|post| post.id)
                .collect::<Vec<_>>(),
            vec![first.id, elsewhere.id, later.id]
        );
        let round = f.send_due(&destination).await;
        let text = |post: &OfferPost| post.draft.text.clone();
        assert_eq!(
            destination.calls(),
            vec![
                (CHAT.to_owned(), text(&first)),
                (OTHER_CHAT.to_owned(), text(&elsewhere)),
                (CHAT.to_owned(), text(&later)),
            ]
        );
        // Only the second post to the same chat waited.
        assert_eq!(f.pause.pauses(), vec![CHAT_SPACING]);
        assert_eq!(round.posts.len(), 3);
        assert!(
            round
                .posts
                .iter()
                .all(|post| post.status == PostStatus::Sent)
        );
        assert_eq!(f.status(future.id).await, PostStatus::Scheduled);

        // Each goes out once.
        f.clock.advance(TimeDelta::minutes(1));
        let round = f.send_due(&destination).await;
        assert!(round.posts.is_empty());
        assert_eq!(destination.calls().len(), 3);
    });
}

#[test]
fn a_post_still_goes_out_thirty_minutes_late_and_waits_for_the_owner_after() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel("telegram", CHAT).await;
        let on_time = f.post(&channel, &facts("Fone Bluetooth")).await;
        let late = f.post(&channel, &facts("Curso de Fotografia")).await;
        f.schedule(on_time.id, PostTime::At(brt(14, 12, 0)))
            .await
            .unwrap();
        f.schedule(late.id, PostTime::At(brt(14, 11, 59)))
            .await
            .unwrap();
        let destination = Recorder::default();

        // The app was closed from before noon until 12:30.
        f.set_clock(brt(14, 12, 30));
        let round = f.send_due(&destination).await;
        assert_eq!(round.held, 1);
        assert_eq!(destination.calls().len(), 1);
        assert_eq!(f.status(on_time.id).await, PostStatus::Sent);
        assert_eq!(f.status(late.id).await, PostStatus::Late);

        // Late, it never goes out on its own.
        f.clock.advance(TimeDelta::hours(2));
        let round = f.send_due(&destination).await;
        assert!(round.posts.is_empty());
        assert_eq!(destination.calls().len(), 1);

        // The owner reschedules it for the next free time, 19:00.
        let rescheduled = f.schedule(late.id, PostTime::NextFree).await.unwrap();
        assert_eq!(rescheduled.status, PostStatus::Scheduled);
        assert_eq!(rescheduled.scheduled_at, Some(brt(14, 19, 0)));
        f.set_clock(brt(14, 19, 0));
        f.send_due(&destination).await;
        assert_eq!(f.status(late.id).await, PostStatus::Sent);
        assert_eq!(destination.calls().len(), 2);
    });
}

#[test]
fn a_late_post_goes_out_when_the_owner_publishes_it_now() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel("telegram", CHAT).await;
        let post = f.post(&channel, &facts("Fone Bluetooth")).await;
        f.schedule(post.id, PostTime::At(brt(14, 12, 0)))
            .await
            .unwrap();
        f.set_clock(brt(14, 20, 0));
        f.posts.hold_back().await.unwrap();
        assert_eq!(f.status(post.id).await, PostStatus::Late);

        let destination = Recorder::default();
        let request = f.posts.prepare(post.id).await.unwrap();
        let published = f
            .posts
            .publish(&destination, request.confirm())
            .await
            .unwrap();
        assert_eq!(published.status, PostStatus::Sent);
        assert_eq!(destination.calls().len(), 1);
    });
}

#[test]
fn an_amazon_price_seen_more_than_a_day_before_never_goes_out_on_its_own() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel("telegram", CHAT).await;
        let seen_at = brt(14, 9, 0);
        let post = f.post(&channel, &amazon(seen_at)).await;

        // A time past the price's day is refused up front.
        assert!(matches!(
            f.schedule(post.id, PostTime::At(brt(15, 9, 1))).await,
            Err(PostError::OldPrice)
        ));
        f.schedule(post.id, PostTime::At(brt(15, 8, 50)))
            .await
            .unwrap();

        // The app opens 20 minutes after its time: not late yet, but the
        // price was seen 24 hours and 10 minutes before.
        let destination = Recorder::default();
        f.set_clock(brt(15, 9, 10));
        assert!(f.posts.due(f.clock.now()).await.unwrap().is_empty());
        let round = f.send_due(&destination).await;
        assert_eq!(round.held, 1);
        assert!(destination.calls().is_empty());
        let held = f.posts.post(post.id).await.unwrap();
        assert_eq!(held.status, PostStatus::PriceExpired);
        assert!(!held.status.publishable());
        assert!(held.status.editable());

        // Not even "Publicar agora" takes the old price.
        assert!(matches!(
            f.posts.prepare(post.id).await,
            Err(PostError::WrongStatus(PostStatus::PriceExpired))
        ));

        // The owner sees the price again: the post is a draft to confirm.
        let mut draft = held.draft.clone();
        draft.facts = amazon(f.clock.now());
        let updated = f.posts.update(post.id, &draft).await.unwrap();
        assert_eq!(updated.status, PostStatus::Draft);
        let scheduled = f.schedule(post.id, PostTime::NextFree).await.unwrap();
        assert_eq!(scheduled.scheduled_at, Some(brt(15, 12, 0)));
        f.set_clock(brt(15, 12, 0));
        f.send_due(&destination).await;
        assert_eq!(f.status(post.id).await, PostStatus::Sent);
    });
}

#[test]
fn publishing_now_refuses_a_price_its_program_no_longer_keeps() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel("telegram", CHAT).await;
        let fresh = f.post(&channel, &amazon(brt(13, 9, 0))).await;
        let old = f.post(&channel, &amazon(brt(13, 8, 59))).await;
        // Seen exactly a day ago, the price still goes out.
        assert!(f.posts.prepare(fresh.id).await.is_ok());
        assert!(matches!(
            f.posts.prepare(old.id).await,
            Err(PostError::OldPrice)
        ));
        // A price without its time asked for keeps.
        let typed = OfferFacts {
            price_needs_time: false,
            ..amazon(brt(1, 9, 0))
        };
        let shopee = f.post(&channel, &typed).await;
        assert!(f.posts.prepare(shopee.id).await.is_ok());
    });
}

#[test]
fn a_paused_queue_sends_nothing_and_resuming_sends_only_what_is_not_late() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel("telegram", CHAT).await;
        let early = f.post(&channel, &facts("Fone Bluetooth")).await;
        let recent = f.post(&channel, &facts("Curso de Fotografia")).await;
        f.schedule(early.id, PostTime::At(brt(14, 12, 0)))
            .await
            .unwrap();
        f.schedule(recent.id, PostTime::At(brt(14, 12, 20)))
            .await
            .unwrap();
        let destination = Recorder::default();

        assert!(!f.posts.queue_paused().await.unwrap());
        f.posts.set_queue_paused(true).await.unwrap();
        assert!(f.posts.queue_paused().await.unwrap());
        f.set_clock(brt(14, 12, 20));
        let round = f.send_due(&destination).await;
        assert!(round.paused);
        assert!(round.posts.is_empty());
        f.set_clock(brt(14, 12, 40));
        f.send_due(&destination).await;
        assert!(destination.calls().is_empty());
        assert_eq!(f.status(early.id).await, PostStatus::Late);
        assert_eq!(f.status(recent.id).await, PostStatus::Scheduled);

        f.posts.set_queue_paused(false).await.unwrap();
        let round = f.send_due(&destination).await;
        assert!(!round.paused);
        assert_eq!(
            destination.calls(),
            vec![(CHAT.to_owned(), recent.draft.text.clone())]
        );
        assert_eq!(f.status(early.id).await, PostStatus::Late);
    });
}

#[test]
fn editing_a_scheduled_post_takes_it_out_of_the_queue_until_confirmed_again() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel("telegram", CHAT).await;
        let post = f.post(&channel, &facts("Fone Bluetooth")).await;
        f.clock.advance(TimeDelta::seconds(1));
        let confirmed_before = f.posts.prepare(post.id).await.unwrap().confirm();
        let scheduled = f
            .schedule(post.id, PostTime::At(brt(14, 12, 0)))
            .await
            .unwrap();
        assert_eq!(scheduled.status, PostStatus::Scheduled);

        // A confirmation of the text before it was scheduled is spent.
        assert!(matches!(
            f.posts
                .schedule(confirmed_before, PostTime::At(brt(14, 19, 0)))
                .await,
            Err(PostError::Changed)
        ));

        f.clock.advance(TimeDelta::seconds(1));
        let mut draft = scheduled.draft.clone();
        draft.text = draft.text.replace("Fone Bluetooth", "Fone Bluetooth JBL");
        let edited = f.posts.update(post.id, &draft).await.unwrap();
        assert_eq!(edited.status, PostStatus::Draft);
        assert_eq!(edited.scheduled_at, None);

        let destination = Recorder::default();
        f.set_clock(brt(14, 12, 0));
        let round = f.send_due(&destination).await;
        assert!(round.posts.is_empty());
        assert!(destination.calls().is_empty());

        // Confirmed again, the new text goes out at its time.
        f.schedule(post.id, PostTime::At(brt(14, 12, 5)))
            .await
            .unwrap();
        f.set_clock(brt(14, 12, 5));
        f.send_due(&destination).await;
        assert_eq!(
            destination.calls(),
            vec![(CHAT.to_owned(), edited.draft.text.clone())]
        );
    });
}

#[test]
fn cancelling_takes_a_post_out_of_the_queue_as_a_draft() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel("telegram", CHAT).await;
        let post = f.post(&channel, &facts("Fone Bluetooth")).await;
        assert!(matches!(
            f.posts.unschedule(post.id).await,
            Err(PostError::WrongStatus(PostStatus::Draft))
        ));
        f.schedule(post.id, PostTime::At(brt(14, 12, 0)))
            .await
            .unwrap();
        let cancelled = f.posts.unschedule(post.id).await.unwrap();
        assert_eq!(cancelled.status, PostStatus::Draft);
        assert_eq!(cancelled.scheduled_at, None);

        f.set_clock(brt(14, 12, 0));
        let destination = Recorder::default();
        f.send_due(&destination).await;
        assert!(destination.calls().is_empty());
    });
}

#[test]
fn a_new_chat_or_a_deleted_channel_takes_its_scheduled_posts_out_of_the_queue() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel("telegram", CHAT).await;
        let moved = f.post(&channel, &facts("Fone Bluetooth")).await;
        f.schedule(moved.id, PostTime::At(brt(14, 12, 0)))
            .await
            .unwrap();

        // Renaming keeps it; another chat is not where the owner confirmed.
        let renamed = NewPromotionChannel {
            name: "Ofertas".into(),
            kind: ChannelKind::TelegramChannel,
            default_tag: "telegram".into(),
            keywords: Vec::new(),
            telegram_chat: Some(CHAT.into()),
        };
        f.channels.update(channel.id, &renamed).await.unwrap();
        assert_eq!(f.status(moved.id).await, PostStatus::Scheduled);
        f.channels
            .update(
                channel.id,
                &NewPromotionChannel {
                    telegram_chat: Some(OTHER_CHAT.into()),
                    ..renamed
                },
            )
            .await
            .unwrap();
        assert_eq!(f.status(moved.id).await, PostStatus::Draft);

        let other = f.channel("achados", "@outrocanal").await;
        let orphan = f.post(&other, &facts("Cafeteira")).await;
        f.schedule(orphan.id, PostTime::At(brt(14, 12, 0)))
            .await
            .unwrap();
        f.channels.delete(other.id).await.unwrap();
        assert_eq!(f.status(orphan.id).await, PostStatus::Draft);
    });
}

#[test]
fn what_may_have_gone_out_or_the_login_refused_stops_there() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel("telegram", CHAT).await;
        let unsure = f.post(&channel, &facts("Fone Bluetooth")).await;
        let refused = f.post(&channel, &facts("Curso de Fotografia")).await;
        let waiting = f.post(&channel, &facts("Cafeteira")).await;
        for (post, minute) in [(&unsure, 0), (&refused, 1), (&waiting, 2)] {
            f.schedule(post.id, PostTime::At(brt(14, 12, minute)))
                .await
                .unwrap();
        }
        let destination = Recorder::answering(vec![
            Err(PublishFailure::Unconfirmed("timed out".into())),
            Err(PublishFailure::Expired),
        ]);

        f.set_clock(brt(14, 12, 2));
        let round = f.send_due(&destination).await;
        assert!(round.expired);
        assert_eq!(f.status(unsure.id).await, PostStatus::Unconfirmed);
        assert_eq!(f.status(refused.id).await, PostStatus::Failed);
        assert_eq!(f.status(waiting.id).await, PostStatus::Scheduled);

        // The next turn sends the one left, and never the unconfirmed one.
        f.clock.advance(TimeDelta::minutes(1));
        let round = f.send_due(&destination).await;
        assert_eq!(round.posts.len(), 1);
        assert_eq!(f.status(waiting.id).await, PostStatus::Sent);
        assert_eq!(f.status(unsure.id).await, PostStatus::Unconfirmed);
        assert_eq!(destination.calls().len(), 3);
    });
}

#[test]
fn the_next_free_time_skips_taken_times_and_rolls_to_the_next_day() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel("telegram", CHAT).await;
        assert_eq!(channel.posting_times, default_posting_times());
        assert_eq!(
            f.posts.next_free(channel.id, None).await.unwrap(),
            Some(brt(14, 12, 0))
        );
        let mut times = Vec::new();
        for product in ["A", "B", "C", "D"] {
            let post = f.post(&channel, &facts(product)).await;
            times.push(
                f.schedule(post.id, PostTime::NextFree)
                    .await
                    .unwrap()
                    .scheduled_at
                    .unwrap(),
            );
        }
        assert_eq!(
            times,
            vec![
                brt(14, 12, 0),
                brt(14, 19, 0),
                brt(14, 21, 0),
                brt(15, 8, 0)
            ]
        );

        // A time the owner picks must be free and ahead.
        let picked = f.post(&channel, &facts("E")).await;
        assert!(matches!(
            f.schedule(picked.id, PostTime::At(brt(14, 19, 0))).await,
            Err(PostError::TimeTaken)
        ));
        assert!(matches!(
            f.schedule(picked.id, PostTime::At(brt(14, 8, 59))).await,
            Err(PostError::PastTime)
        ));
        let at = brt(14, 19, 30) + TimeDelta::seconds(42);
        let scheduled = f.schedule(picked.id, PostTime::At(at)).await.unwrap();
        assert_eq!(scheduled.scheduled_at, Some(brt(14, 19, 30)));
    });
}

#[test]
fn a_telegram_channel_keeps_its_own_posting_times() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel("telegram", CHAT).await;
        let kept = f
            .channels
            .set_posting_times(
                channel.id,
                &[
                    time(18, 30),
                    NaiveTime::from_hms_opt(7, 15, 40).unwrap(),
                    time(18, 30),
                ],
            )
            .await
            .unwrap();
        assert_eq!(kept.posting_times, vec![time(7, 15), time(18, 30)]);
        assert_eq!(
            f.channels.channel(channel.id).await.unwrap().posting_times,
            kept.posting_times
        );
        assert_eq!(
            f.posts.next_free(channel.id, None).await.unwrap(),
            Some(brt(14, 18, 30))
        );

        // Without times, only a picked time schedules.
        f.channels.set_posting_times(channel.id, &[]).await.unwrap();
        let post = f.post(&channel, &facts("Fone Bluetooth")).await;
        assert!(matches!(
            f.schedule(post.id, PostTime::NextFree).await,
            Err(PostError::NoPostingTimes)
        ));

        let whatsapp = f
            .channels
            .create(&NewPromotionChannel {
                name: "Grupo VIP".into(),
                kind: ChannelKind::WhatsAppGroup,
                default_tag: "grupo_vip".into(),
                keywords: Vec::new(),
                telegram_chat: None,
            })
            .await
            .unwrap();
        assert!(whatsapp.posting_times.is_empty());
        assert!(matches!(
            f.channels
                .set_posting_times(whatsapp.id, &[time(8, 0)])
                .await,
            Err(ChannelError::NotTelegram)
        ));
    });
}

#[test]
fn the_queue_lists_what_waits_what_failed_and_what_went_out_today() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel("telegram", CHAT).await;
        let posts: Vec<OfferPost> = {
            let mut posts = Vec::new();
            for product in ["sent yesterday", "sent", "unsure", "failed", "late", "next"] {
                posts.push(f.post(&channel, &facts(product)).await);
            }
            posts
        };
        let [yesterday, sent, unsure, failed, late, next] = &posts[..] else {
            unreachable!()
        };
        f.schedule(yesterday.id, PostTime::At(brt(14, 23, 50)))
            .await
            .unwrap();
        f.set_clock(brt(14, 23, 50));
        f.send_due(&Recorder::default()).await;

        f.set_clock(brt(15, 8, 0));
        for (post, minute) in [(sent, 1), (unsure, 2), (failed, 3), (late, 4)] {
            f.schedule(post.id, PostTime::At(brt(15, 9, minute)))
                .await
                .unwrap();
        }
        f.schedule(next.id, PostTime::At(brt(15, 12, 0)))
            .await
            .unwrap();
        f.set_clock(brt(15, 9, 3));
        f.send_due(&Recorder::answering(vec![
            Ok(PublishedPost {
                message_id: "7".into(),
                link: None,
            }),
            Err(PublishFailure::Unconfirmed("timed out".into())),
            Err(PublishFailure::Refused("chat not found".into())),
        ]))
        .await;
        f.set_clock(brt(15, 9, 40));
        f.posts.hold_back().await.unwrap();
        f.posts.set_queue_paused(true).await.unwrap();

        let queue = f.posts.queue().await.unwrap();
        let ids = |list: &[OfferPost]| list.iter().map(|post| post.id).collect::<Vec<_>>();
        assert!(queue.paused);
        assert_eq!(ids(&queue.scheduled), vec![next.id]);
        assert_eq!(ids(&queue.waiting), vec![late.id]);
        assert_eq!(ids(&queue.unconfirmed), vec![unsure.id]);
        assert_eq!(ids(&queue.failed), vec![failed.id]);
        // Yesterday 23:50 in Brasília is another day, though the same in UTC.
        assert_eq!(ids(&queue.published_today), vec![sent.id]);
        assert!(!queue.holds(yesterday.id));
        assert!(queue.holds(late.id));
    });
}

/// `day` at `time` on Brasília's clock.
fn at_brasilia(day: NaiveDate, time: NaiveTime) -> Timestamp {
    day.and_time(time)
        .and_local_timezone(brasilia())
        .single()
        .unwrap()
        .to_utc()
}

proptest! {
    #[test]
    fn the_next_free_time_is_ahead_free_and_the_first_such(
        minutes in prop::collection::vec(0u32..24 * 60, 1..6),
        taken_offsets in prop::collection::vec(0i64..5 * 24 * 60, 0..20),
        now_offset in 0i64..3 * 24 * 60 * 60,
    ) {
        let times: Vec<NaiveTime> = minutes
            .iter()
            .map(|minute| time(minute / 60, minute % 60))
            .collect();
        let now = Utc.with_ymd_and_hms(2026, 10, 14, 0, 0, 0).unwrap()
            + TimeDelta::seconds(now_offset);
        let start = now.with_timezone(&brasilia()).date_naive();
        // Taken minutes land on posting times, as scheduled posts do.
        let taken: Vec<Timestamp> = taken_offsets
            .iter()
            .map(|offset| {
                let day = start + TimeDelta::days(offset / (24 * 60) % 5);
                at_brasilia(day, times[*offset as usize % times.len()])
            })
            .collect();

        let next = next_free_time(&times, &taken, now).expect("times to pick from");
        prop_assert!(next > now, "never in the past");
        prop_assert!(!taken.contains(&next), "never on a taken time");
        let local = next.with_timezone(&brasilia());
        prop_assert!(times.contains(&local.time()), "one of the posting times");
        // No posting time between now and it is free.
        let mut day = start;
        while day <= local.date_naive() {
            for time in &times {
                let candidate = at_brasilia(day, *time);
                if candidate > now && candidate < next {
                    prop_assert!(taken.contains(&candidate));
                }
            }
            day += TimeDelta::days(1);
        }
    }
}

#[test]
fn without_posting_times_there_is_no_next_free_time() {
    assert_eq!(next_free_time(&[], &[], Utc::now()), None);
}
