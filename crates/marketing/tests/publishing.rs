//! Publishing Offer Posts through the Marketing module's public interface
//! with a real temporary database and a scripted destination: nothing
//! reaches the destination unless the owner confirmed it, each post goes out
//! at most once on its own, and every outcome leaves the post in the state
//! the owner acts on.

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, PlatformError, RecordId};
use mascate_marketing::{
    AD_DISCLOSURE, ChannelKind, ConfirmedPost, DestinationCheck, MIGRATIONS, NewPromotionChannel,
    OfferFacts, OfferPost, OfferPosts, PostDestination, PostError, PostImage, PostProblem,
    PostStatus, PromotionChannel, PromotionChannels, PublishFailure, PublishedPost, SentPost,
    TELEGRAM_LIMITS,
};
use mascate_platform::{Database, migrate};

const LINK: &str = "https://go.hotmart.com/X12345678Y?src=telegram";
const CHAT: &str = "@ofertasdofernando";

/// What the destination received.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    chat: String,
    text: String,
    image: Option<PostImage>,
    /// The post's status in the database while the destination had it.
    status: PostStatus,
}

/// Answers each publish with the next scripted outcome, and remembers it.
struct Scripted {
    answers: Mutex<VecDeque<Result<PublishedPost, PublishFailure>>>,
    calls: Mutex<Vec<Call>>,
    /// Reads the post's status while it is on its way.
    posts: Arc<OfferPosts>,
    post: RecordId,
    /// Stops mid-way, as an app that closes while sending.
    crash: bool,
}

impl Scripted {
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

impl PostDestination for Scripted {
    fn check(&self, _chat: &str) -> Result<DestinationCheck, PlatformError> {
        unreachable!("the Marketing never checks on its own")
    }

    fn publish(&self, post: &ConfirmedPost) -> Result<PublishedPost, PublishFailure> {
        let id = self.post;
        let posts = self.posts.clone();
        // The destination runs inside the caller's executor; read on another.
        let status = std::thread::spawn(move || block_on(posts.post(id)).unwrap().status)
            .join()
            .unwrap();
        self.calls.lock().unwrap().push(Call {
            chat: post.chat().to_owned(),
            text: post.text().to_owned(),
            image: post.image().cloned(),
            status,
        });
        if self.crash {
            panic!("the app closed");
        }
        self.answers
            .lock()
            .unwrap()
            .pop_front()
            .expect("an answer for every call")
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    channels: PromotionChannels,
    posts: Arc<OfferPosts>,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap(),
        ));
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
            posts: Arc::new(OfferPosts::new(database, clock.clone(), ids)),
            clock,
        }
    }

    async fn channel(&self, kind: ChannelKind, chat: Option<&str>) -> PromotionChannel {
        self.channels
            .create(&new_channel(kind, chat))
            .await
            .unwrap()
    }

    async fn post(&self, channel: &PromotionChannel, facts: &OfferFacts) -> OfferPost {
        let draft = self.posts.draft(facts, channel).await.unwrap();
        self.posts.create(&draft).await.unwrap()
    }

    fn destination(
        &self,
        post: &OfferPost,
        answers: Vec<Result<PublishedPost, PublishFailure>>,
    ) -> Scripted {
        Scripted {
            answers: Mutex::new(answers.into()),
            calls: Mutex::new(Vec::new()),
            posts: self.posts.clone(),
            post: post.id,
            crash: false,
        }
    }

    /// A moment later, so each change has its own time.
    fn later(&self) {
        self.clock.advance(TimeDelta::seconds(1));
    }
}

fn new_channel(kind: ChannelKind, chat: Option<&str>) -> NewPromotionChannel {
    NewPromotionChannel {
        name: "Ofertas do Fernando".into(),
        kind,
        default_tag: "telegram".into(),
        keywords: Vec::new(),
        telegram_chat: chat.map(str::to_owned),
    }
}

fn facts() -> OfferFacts {
    OfferFacts {
        link_id: RecordId::from_u128(900),
        link: LINK.into(),
        product: "Curso de Fotografia".into(),
        platform: "Hotmart".into(),
        price: None,
        image: Some(PostImage::Url("https://cf.shopee.com.br/file/abc".into())),
        price_needs_time: false,
        forbidden_contacts: Vec::new(),
    }
}

fn sent(message_id: &str) -> Result<PublishedPost, PublishFailure> {
    Ok(PublishedPost {
        message_id: message_id.into(),
        link: Some(format!("https://t.me/ofertasdofernando/{message_id}")),
    })
}

#[test]
fn a_confirmed_post_reaches_the_channel_once_and_keeps_its_link() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::TelegramChannel, Some(CHAT)).await;
        let post = f.post(&channel, &facts()).await;
        let destination = f.destination(&post, vec![sent("42")]);

        let request = f.posts.prepare(post.id).await.unwrap();
        assert_eq!(request.text(), post.draft.text);
        assert_eq!(request.image(), facts().image.as_ref());
        assert_eq!(request.chat(), CHAT);
        assert_eq!(request.channel(), "Ofertas do Fernando");
        // A request left unconfirmed changes nothing.
        let again = f.posts.prepare(post.id).await.unwrap();
        assert_eq!(
            f.posts.post(post.id).await.unwrap().status,
            PostStatus::Draft
        );
        assert!(destination.calls().is_empty());

        f.later();
        let published = f
            .posts
            .publish(&destination, request.confirm())
            .await
            .unwrap();
        assert_eq!(published.status, PostStatus::Sent);
        assert_eq!(
            published.sent,
            Some(SentPost {
                at: f.clock.now(),
                message_id: Some("42".into()),
                link: Some("https://t.me/ofertasdofernando/42".into()),
            })
        );
        assert_eq!(published.failure, None);
        assert_eq!(
            destination.calls(),
            vec![Call {
                chat: CHAT.into(),
                text: post.draft.text.clone(),
                image: facts().image,
                status: PostStatus::Sending,
            }]
        );

        // The second request of the same draft finds it gone out.
        f.later();
        assert!(matches!(
            f.posts.publish(&destination, again.confirm()).await,
            Err(PostError::Changed)
        ));
        assert!(matches!(
            f.posts.prepare(post.id).await,
            Err(PostError::WrongStatus(PostStatus::Sent))
        ));
        assert!(matches!(
            f.posts.update(post.id, &post.draft).await,
            Err(PostError::WrongStatus(PostStatus::Sent))
        ));
        assert_eq!(destination.calls().len(), 1);
    });
}

#[test]
fn only_a_telegram_channel_with_its_chat_has_a_destination() {
    block_on(async {
        let f = Fixture::new().await;
        let whatsapp = f.channel(ChannelKind::WhatsAppGroup, None).await;
        let post = f.post(&whatsapp, &facts()).await;
        assert!(matches!(
            f.posts.prepare(post.id).await,
            Err(PostError::NoDestination)
        ));

        f.channels
            .update(
                whatsapp.id,
                &new_channel(ChannelKind::TelegramChannel, None),
            )
            .await
            .unwrap();
        assert!(matches!(
            f.posts.prepare(post.id).await,
            Err(PostError::NoChat)
        ));

        f.channels
            .update(
                whatsapp.id,
                &new_channel(ChannelKind::TelegramChannel, Some("-1001234567890")),
            )
            .await
            .unwrap();
        assert_eq!(
            f.posts.prepare(post.id).await.unwrap().chat(),
            "-1001234567890"
        );
    });
}

#[test]
fn a_text_or_caption_over_telegrams_limits_never_reaches_it() {
    block_on(async {
        let f = Fixture::new().await;
        // Saved where no limit applies, then the channel turns into Telegram.
        let other = f.channel(ChannelKind::Other, None).await;
        let mut long = f.posts.draft(&facts(), &other).await.unwrap();
        long.text = format!("{}\n{}", long.text, "a".repeat(TELEGRAM_LIMITS.caption));
        let captioned = f.posts.create(&long).await.unwrap();
        let mut longer = f
            .posts
            .draft(
                &OfferFacts {
                    image: None,
                    ..facts()
                },
                &other,
            )
            .await
            .unwrap();
        longer.text = format!("{}\n{}", longer.text, "a".repeat(TELEGRAM_LIMITS.text));
        let plain = f.posts.create(&longer).await.unwrap();
        f.channels
            .update(
                other.id,
                &new_channel(ChannelKind::TelegramChannel, Some(CHAT)),
            )
            .await
            .unwrap();

        for (post, limit, caption) in [
            (&captioned, TELEGRAM_LIMITS.caption, true),
            (&plain, TELEGRAM_LIMITS.text, false),
        ] {
            assert!(matches!(
                f.posts.prepare(post.id).await,
                Err(PostError::Blocked(problems)) if problems == vec![PostProblem::TooLong {
                    units: post.draft.text.encode_utf16().count(),
                    limit,
                    caption,
                }]
            ));
        }
    });
}

#[test]
fn a_post_changed_after_its_confirmation_is_not_published() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::TelegramChannel, Some(CHAT)).await;
        let post = f.post(&channel, &facts()).await;
        let destination = f.destination(&post, vec![sent("42")]);
        let request = f.posts.prepare(post.id).await.unwrap();

        f.later();
        let mut edited = post.draft.clone();
        edited.text = format!("{} Corre!", edited.text);
        f.posts.update(post.id, &edited).await.unwrap();
        assert!(matches!(
            f.posts.publish(&destination, request.confirm()).await,
            Err(PostError::Changed)
        ));
        assert!(destination.calls().is_empty());
        assert_eq!(
            f.posts.post(post.id).await.unwrap().status,
            PostStatus::Draft
        );
    });
}

#[test]
fn a_post_nothing_published_fails_with_why_and_can_be_edited_and_sent_again() {
    block_on(async {
        for failure in [
            PublishFailure::NotConnected,
            PublishFailure::Expired,
            PublishFailure::Refused("Bad Request: chat not found".into()),
            PublishFailure::RateLimited,
            PublishFailure::NotSent("connection refused".into()),
        ] {
            let f = Fixture::new().await;
            let channel = f.channel(ChannelKind::TelegramChannel, Some(CHAT)).await;
            let post = f.post(&channel, &facts()).await;
            let destination = f.destination(&post, vec![Err(failure.clone()), sent("7")]);

            f.later();
            let request = f.posts.prepare(post.id).await.unwrap();
            let failed = f
                .posts
                .publish(&destination, request.confirm())
                .await
                .unwrap();
            assert_eq!(failed.status, PostStatus::Failed, "{failure:?}");
            assert_eq!(failed.failure, Some(failure.clone()));
            assert_eq!(failed.sent, None);

            f.later();
            let mut edited = failed.draft.clone();
            edited.text = format!("{} Corre!", edited.text);
            let edited = f.posts.update(post.id, &edited).await.unwrap();
            assert_eq!(edited.status, PostStatus::Failed);
            let request = f.posts.prepare(post.id).await.unwrap();
            let published = f
                .posts
                .publish(&destination, request.confirm())
                .await
                .unwrap();
            assert_eq!(published.status, PostStatus::Sent, "{failure:?}");
            assert_eq!(published.failure, None);
            assert_eq!(destination.calls().len(), 2);
        }
    });
}

#[test]
fn a_post_that_may_have_gone_out_waits_for_the_owner() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::TelegramChannel, Some(CHAT)).await;
        let found = f.post(&channel, &facts()).await;
        let resent = f.post(&channel, &facts()).await;
        let timeout = PublishFailure::Unconfirmed("timed out reading the answer".into());

        for post in [&found, &resent] {
            let destination = f.destination(post, vec![Err(timeout.clone()), sent("9")]);
            f.later();
            let request = f.posts.prepare(post.id).await.unwrap();
            let unknown = f
                .posts
                .publish(&destination, request.confirm())
                .await
                .unwrap();
            assert_eq!(unknown.status, PostStatus::Unconfirmed);
            assert_eq!(unknown.failure, Some(timeout.clone()));
            assert!(matches!(
                f.posts.update(post.id, &post.draft).await,
                Err(PostError::WrongStatus(PostStatus::Unconfirmed))
            ));
            assert_eq!(destination.calls().len(), 1);

            if post.id == found.id {
                // The owner found it in the channel.
                f.later();
                let marked = f.posts.mark_published(post.id).await.unwrap();
                assert_eq!(marked.status, PostStatus::Sent);
                assert_eq!(
                    marked.sent,
                    Some(SentPost {
                        at: f.clock.now(),
                        message_id: None,
                        link: None,
                    })
                );
                assert_eq!(marked.failure, None);
            } else {
                // It was not there: "Publicar de novo".
                f.later();
                let request = f.posts.prepare(post.id).await.unwrap();
                let published = f
                    .posts
                    .publish(&destination, request.confirm())
                    .await
                    .unwrap();
                assert_eq!(published.status, PostStatus::Sent);
                assert_eq!(destination.calls().len(), 2);
            }
        }
        assert!(matches!(
            f.posts.mark_published(found.id).await,
            Err(PostError::WrongStatus(PostStatus::Sent))
        ));
    });
}

#[test]
fn a_post_left_sending_when_the_app_closed_is_unconfirmed_on_the_next_start() {
    let f = block_on(Fixture::new());
    let channel = block_on(f.channel(ChannelKind::TelegramChannel, Some(CHAT)));
    let post = block_on(f.post(&channel, &facts()));
    let other = block_on(f.post(&channel, &facts()));
    let destination = Scripted {
        crash: true,
        ..f.destination(&post, Vec::new())
    };
    let request = block_on(f.posts.prepare(post.id)).unwrap();
    let crashed = catch_unwind(AssertUnwindSafe(|| {
        block_on(f.posts.publish(&destination, request.confirm()))
    }));
    assert!(crashed.is_err());
    assert_eq!(destination.calls().len(), 1);
    assert_eq!(
        block_on(f.posts.post(post.id)).unwrap().status,
        PostStatus::Sending
    );

    f.later();
    assert_eq!(block_on(f.posts.settle_interrupted()).unwrap(), 1);
    let settled = block_on(f.posts.post(post.id)).unwrap();
    assert_eq!(settled.status, PostStatus::Unconfirmed);
    assert_eq!(settled.failure, Some(PublishFailure::Interrupted));
    assert!(settled.failure.as_ref().unwrap().may_have_gone_out());
    assert_eq!(
        block_on(f.posts.post(other.id)).unwrap().status,
        PostStatus::Draft
    );
    assert_eq!(block_on(f.posts.settle_interrupted()).unwrap(), 0);
}

#[test]
fn a_post_without_the_disclosure_raises_the_reminder_once_it_may_be_out() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::TelegramChannel, Some(CHAT)).await;
        let mut bare = f.posts.draft(&facts(), &channel).await.unwrap();
        bare.text = format!("Curso de Fotografia {LINK}");

        // Nothing went out: no Reminder.
        let refused = f.posts.create(&bare).await.unwrap();
        let destination = f.destination(
            &refused,
            vec![Err(PublishFailure::Refused("Forbidden".into()))],
        );
        let request = f.posts.prepare(refused.id).await.unwrap();
        f.posts
            .publish(&destination, request.confirm())
            .await
            .unwrap();
        assert!(f.posts.raised_reminders().await.unwrap().is_empty());

        for outcome in [sent("1"), Err(PublishFailure::Unconfirmed("502".into()))] {
            let fresh = Fixture::new().await;
            let channel = fresh
                .channel(ChannelKind::TelegramChannel, Some(CHAT))
                .await;
            let post = fresh
                .posts
                .create(&mascate_marketing::PostDraft {
                    channel: channel.id,
                    ..bare.clone()
                })
                .await
                .unwrap();
            let destination = fresh.destination(&post, vec![outcome]);
            let request = fresh.posts.prepare(post.id).await.unwrap();
            fresh
                .posts
                .publish(&destination, request.confirm())
                .await
                .unwrap();
            assert_eq!(
                fresh.posts.raised_reminders().await.unwrap(),
                vec![AD_DISCLOSURE.key]
            );
        }
    });
}
