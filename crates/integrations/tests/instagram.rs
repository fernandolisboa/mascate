//! Instagram as a `PostDestination` (#156) against the fake server serving
//! answers written from the Instagram Platform's documentation (see the
//! fixtures' README), with marketing's `OfferPosts` end to end on a real
//! temporary database: the pasted token and its renewal inside the window,
//! the quota read first, the container readied and then published, what
//! each answer leaves the post as, and a post whose answer never came
//! settled by the container's status, never sent twice on its own.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_integrations::{INSTAGRAM_TOKEN_DAYS, Instagram, InstagramAccount, Pause};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, PlatformError, RecordId, Timestamp};
use mascate_marketing::{
    ChannelKind, DestinationStanding, MIGRATIONS, NewPromotionChannel, OfferFacts, OfferPost,
    OfferPosts, PostDestination, PostImage, PostStatus, PromotionChannels, PublishFailure,
};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore, Request};
use mascate_platform::{Database, Secret, SecretStore, migrate, stored};
use serde_json::{Value, json};

const TOKEN: &str = "IGAAMASCATEcolado0123456789";
const RENEWED: &str = "IGAAMASCATErenovado0123456789";
const USER: &str = "17841405822304914";
const CONTAINER: &str = "17889615691921648";
const MEDIA_ID: &str = "17920238422030506";
const LINK: &str = "https://go.hotmart.com/X12345678Y?src=instagram";
const PHOTO: &str = "https://cf.shopee.com.br/file/abc.jpg";

const ME: &str = "/v25.0/me";
const LIMIT: &str = "/v25.0/17841405822304914/content_publishing_limit";
const MEDIA: &str = "/v25.0/17841405822304914/media";
const PUBLISH: &str = "/v25.0/17841405822304914/media_publish";
const CONTAINER_PATH: &str = "/v25.0/17889615691921648";
const MEDIA_PATH: &str = "/v25.0/17920238422030506";
const REFRESH: &str = "/refresh_access_token";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/instagram/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

/// Remembers each pause instead of sleeping.
#[derive(Default)]
struct RecordedPauses(Mutex<Vec<Duration>>);

impl Pause for RecordedPauses {
    fn pause(&self, duration: Duration) {
        self.0.lock().unwrap().push(duration);
    }
}

impl RecordedPauses {
    fn seconds(&self) -> Vec<u64> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(Duration::as_secs)
            .collect()
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    server: FakeHttpServer,
    store: Arc<MemorySecretStore>,
    clock: Arc<ManualClock>,
    pauses: Arc<RecordedPauses>,
    instagram: Arc<Instagram>,
    channels: PromotionChannels,
    posts: OfferPosts,
}

impl Fixture {
    /// The token pasted just now, and Instagram answering a publish that
    /// goes well, the photo readied on the second look.
    async fn connected() -> Self {
        let f = Self::bare().await;
        f.store
            .write("INSTAGRAM_ACCESS_TOKEN", &Secret::new(TOKEN))
            .unwrap();
        f.instagram.token_pasted().unwrap();
        f.server.serve_any_query(ME, 200, fixture("me.json"));
        f.server.serve_any_query(LIMIT, 200, fixture("limit.json"));
        f.server.serve(MEDIA, 200, fixture("container.json"));
        f.server
            .serve_any_query(CONTAINER_PATH, 200, fixture("container-in-progress.json"));
        f.server.then_serve(
            &any(CONTAINER_PATH),
            200,
            fixture("container-finished.json"),
        );
        f.server
            .serve(PUBLISH, 200, fixture("media-published.json"));
        f.server
            .serve_any_query(MEDIA_PATH, 200, fixture("media.json"));
        f.server
            .serve_any_query(REFRESH, 200, fixture("token-renewed.json"));
        f
    }

    /// Nothing stored.
    async fn bare() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 21, 15, 0, 0).unwrap(),
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
        let pauses = Arc::new(RecordedPauses::default());
        Self {
            instagram: Arc::new(
                Instagram::new(server.url(), "Mascate/test", store.clone(), clock.clone())
                    .with_pause(pauses.clone())
                    .with_answer_timeout(Duration::from_millis(500)),
            ),
            channels: PromotionChannels::new(database.clone(), clock.clone(), ids.clone()),
            posts: OfferPosts::new(database, clock.clone(), ids),
            _dir: dir,
            server,
            store,
            clock,
            pauses,
        }
    }

    fn stored(&self, name: &str) -> Option<String> {
        self.store
            .read(name)
            .unwrap()
            .map(|secret| secret.expose().to_owned())
    }

    /// Stores the token's term as handed out `age` ago and lasting `left`.
    fn term(&self, age: TimeDelta, left: TimeDelta) {
        let now = self.clock.now();
        self.store
            .write(
                "INSTAGRAM_TOKEN_TERM",
                &Secret::new(format!("{} {}", stored(now - age), stored(now + left))),
            )
            .unwrap();
    }

    fn requests_to(&self, path: &str) -> Vec<Request> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.path.split('?').next() == Some(path))
            .collect()
    }

    fn containers_created(&self) -> Vec<Value> {
        self.requests_to(MEDIA)
            .iter()
            .map(|request| serde_json::from_str(&request.body).unwrap())
            .collect()
    }

    /// A saved post with `image` on an Instagram channel.
    async fn post(&self, image: Option<PostImage>) -> OfferPost {
        let channel = self
            .channels
            .create(&NewPromotionChannel {
                name: "Instagram da loja".into(),
                kind: ChannelKind::Instagram,
                default_tag: format!("insta{}", self.channels.channels().await.unwrap().len()),
                keywords: Vec::new(),
                telegram_chat: None,
            })
            .await
            .unwrap();
        let facts = OfferFacts {
            link_id: RecordId::from_u128(900),
            link: LINK.into(),
            product: "Curso de Fotografia com Celular".into(),
            platform: "Hotmart".into(),
            price: None,
            image,
            price_needs_time: false,
            forbidden_contacts: Vec::new(),
        };
        let draft = self.posts.draft(&facts, &channel).await.unwrap();
        self.posts.create(&draft).await.unwrap()
    }

    /// Prepares, confirms and publishes `post` as the owner would.
    async fn publish(&self, post: &OfferPost) -> OfferPost {
        let request = self.posts.prepare(post.id).await.unwrap();
        self.posts
            .publish(self.instagram.as_ref(), request.confirm())
            .await
            .unwrap()
    }
}

fn any(path: &str) -> String {
    format!("{path}?*")
}

fn platform_photo() -> Option<PostImage> {
    Some(PostImage::Url(PHOTO.into()))
}

#[test]
fn a_post_is_readied_in_a_container_then_published_keeping_the_media_and_its_link() {
    block_on(async {
        let f = Fixture::connected().await;
        let post = f.post(platform_photo()).await;

        let published = f.publish(&post).await;

        assert_eq!(published.status, PostStatus::Sent);
        let sent = published.sent.unwrap();
        assert_eq!(sent.message_id.as_deref(), Some(MEDIA_ID));
        assert_eq!(
            sent.link.as_deref(),
            Some("https://www.instagram.com/p/DAbc123xyZ_/")
        );
        let paths: Vec<String> = f
            .server
            .requests()
            .into_iter()
            .map(|path| path.split('?').next().unwrap().to_owned())
            .collect();
        assert_eq!(
            paths,
            [
                ME,
                LIMIT,
                MEDIA,
                ME,
                CONTAINER_PATH,
                CONTAINER_PATH,
                PUBLISH,
                MEDIA_PATH
            ]
        );
        assert_eq!(
            f.containers_created(),
            [json!({ "image_url": PHOTO, "caption": post.draft.text })]
        );
        let publish = &f.requests_to(PUBLISH)[0];
        assert_eq!(
            serde_json::from_str::<Value>(&publish.body).unwrap(),
            json!({ "creation_id": CONTAINER })
        );
        assert_eq!(
            publish.authorization.as_deref(),
            Some(format!("Bearer {TOKEN}").as_str())
        );
        // The token never goes in an address but the renewal's.
        assert!(f.server.requests().iter().all(|path| !path.contains(TOKEN)));
        // One wait for the photo, the first and shortest.
        assert_eq!(f.pauses.seconds(), [2]);
    });
}

#[test]
fn with_the_quota_used_the_post_waits_and_no_container_is_created() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server
            .serve_any_query(LIMIT, 200, fixture("limit-used.json"));
        let post = f.post(platform_photo()).await;

        let failed = f.publish(&post).await;

        assert_eq!(failed.status, PostStatus::Failed);
        assert_eq!(failed.failure, Some(PublishFailure::QuotaUsed));
        assert!(f.requests_to(MEDIA).is_empty());
        assert!(f.requests_to(PUBLISH).is_empty());
        // Still the owner's to publish later.
        assert!(failed.status.publishable());
    });
}

#[test]
fn quota_reached_at_media_publish_publishes_nothing() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.serve(PUBLISH, 400, fixture("error-quota.json"));
        let post = f.post(platform_photo()).await;

        let failed = f.publish(&post).await;

        assert_eq!(failed.status, PostStatus::Failed);
        assert_eq!(failed.failure, Some(PublishFailure::QuotaUsed));
    });
}

#[test]
fn a_timeout_on_media_publish_is_unconfirmed_and_a_published_container_settles_it() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.serve_late(
            PUBLISH,
            Duration::from_secs(2),
            200,
            fixture("media-published.json"),
        );
        let post = f.post(platform_photo()).await;

        let unconfirmed = f.publish(&post).await;

        assert_eq!(unconfirmed.status, PostStatus::Unconfirmed);
        assert!(matches!(
            unconfirmed.failure,
            Some(PublishFailure::Unconfirmed(_))
        ));
        f.server
            .serve_any_query(CONTAINER_PATH, 200, fixture("container-published.json"));

        let settled = f
            .posts
            .check_fate(f.instagram.as_ref(), post.id)
            .await
            .unwrap();

        assert_eq!(settled.status, PostStatus::Sent);
        assert_eq!(settled.failure, None);
        let sent = settled.sent.unwrap();
        assert_eq!(sent.message_id.as_deref(), Some(CONTAINER));
        // The container does not name its media: the profile shows it.
        assert_eq!(
            sent.link.as_deref(),
            Some("https://www.instagram.com/mascate.ofertas/")
        );
        // Checking sent nothing.
        assert_eq!(f.requests_to(PUBLISH).len(), 1);
        assert_eq!(f.requests_to(MEDIA).len(), 1);
    });
}

#[test]
fn an_unanswered_post_whose_container_expired_can_be_sent_again_by_the_owner() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.hang_up(PUBLISH);
        let post = f.post(platform_photo()).await;
        let unconfirmed = f.publish(&post).await;
        assert_eq!(unconfirmed.status, PostStatus::Unconfirmed);
        f.server
            .serve_any_query(CONTAINER_PATH, 200, fixture("container-expired.json"));

        let settled = f
            .posts
            .check_fate(f.instagram.as_ref(), post.id)
            .await
            .unwrap();

        assert_eq!(settled.status, PostStatus::Failed);
        assert_eq!(settled.failure, Some(PublishFailure::NotPublished));
        assert_eq!(f.requests_to(PUBLISH).len(), 1);

        // The owner sends it again: a new container, published this time.
        f.server
            .serve_any_query(CONTAINER_PATH, 200, fixture("container-finished.json"));
        f.server
            .serve(PUBLISH, 200, fixture("media-published.json"));
        let again = f.publish(&settled).await;
        assert_eq!(again.status, PostStatus::Sent);
        assert_eq!(f.requests_to(MEDIA).len(), 2);
    });
}

#[test]
fn a_container_still_readying_leaves_the_post_unconfirmed() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.hang_up(PUBLISH);
        let post = f.post(platform_photo()).await;
        f.publish(&post).await;
        f.server
            .serve_any_query(CONTAINER_PATH, 200, fixture("container-in-progress.json"));

        let unsettled = f
            .posts
            .check_fate(f.instagram.as_ref(), post.id)
            .await
            .unwrap();

        assert_eq!(unsettled.status, PostStatus::Unconfirmed);
    });
}

#[test]
fn a_photo_instagram_cannot_ready_publishes_nothing_and_says_why() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server
            .serve_any_query(CONTAINER_PATH, 200, fixture("container-error.json"));
        let post = f.post(platform_photo()).await;

        let failed = f.publish(&post).await;

        assert_eq!(failed.status, PostStatus::Failed);
        assert_eq!(
            failed.failure,
            Some(PublishFailure::Refused(
                "o Instagram não conseguiu baixar a foto do endereço dela".into()
            ))
        );
        assert!(f.requests_to(PUBLISH).is_empty());
    });
}

#[test]
fn a_refused_container_keeps_instagrams_reason() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server
            .serve(MEDIA, 400, fixture("error-aspect-ratio.json"));
        let post = f.post(platform_photo()).await;

        let failed = f.publish(&post).await;

        assert_eq!(failed.status, PostStatus::Failed);
        assert_eq!(
            failed.failure,
            Some(PublishFailure::Refused(
                "a proporção da foto fica fora do que o Instagram aceita, de 4:5 a 1,91:1".into()
            ))
        );
        f.server.serve(MEDIA, 400, fixture("error-caption.json"));
        let other = f.post(platform_photo()).await;
        assert_eq!(
            f.publish(&other).await.failure,
            Some(PublishFailure::Refused(
                "Cannot use more than 20 tags per created media.".into()
            ))
        );
        assert!(f.requests_to(PUBLISH).is_empty());
    });
}

#[test]
fn a_photo_readying_past_five_minutes_publishes_nothing() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server
            .serve_any_query(CONTAINER_PATH, 200, fixture("container-in-progress.json"));
        let post = f.post(platform_photo()).await;

        let failed = f.publish(&post).await;

        assert_eq!(failed.status, PostStatus::Failed);
        assert!(matches!(failed.failure, Some(PublishFailure::NotSent(_))));
        assert!(f.requests_to(PUBLISH).is_empty());
        let waits = f.pauses.seconds();
        assert_eq!(waits, [2, 4, 8, 16, 32, 60, 60, 60, 60]);
        assert!(waits.iter().sum::<u64>() >= 5 * 60);
    });
}

#[test]
fn a_throttled_container_waits_and_is_created_once() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.serve(MEDIA, 400, fixture("error-throttled.json"));
        f.server.then_serve(MEDIA, 200, fixture("container.json"));
        let post = f.post(platform_photo()).await;

        let published = f.publish(&post).await;

        assert_eq!(published.status, PostStatus::Sent);
        assert_eq!(f.requests_to(MEDIA).len(), 2);
        assert_eq!(f.pauses.seconds(), [5, 2]);
    });
}

#[test]
fn instagram_failing_at_media_publish_leaves_the_post_unconfirmed() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.serve(PUBLISH, 500, fixture("error-500.json"));
        let post = f.post(platform_photo()).await;

        let unconfirmed = f.publish(&post).await;

        assert_eq!(unconfirmed.status, PostStatus::Unconfirmed);
        assert_eq!(f.requests_to(PUBLISH).len(), 1);
    });
}

#[test]
fn instagram_failing_at_the_container_publishes_nothing() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.serve(MEDIA, 500, fixture("error-500.json"));
        let post = f.post(platform_photo()).await;

        let failed = f.publish(&post).await;

        assert_eq!(failed.status, PostStatus::Failed);
        assert!(matches!(failed.failure, Some(PublishFailure::NotSent(_))));
    });
}

#[test]
fn a_token_instagram_refuses_with_190_is_expired_and_nothing_is_created() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.serve_any_query(ME, 400, fixture("error-190.json"));
        let post = f.post(platform_photo()).await;

        let failed = f.publish(&post).await;

        assert_eq!(failed.status, PostStatus::Failed);
        assert_eq!(failed.failure, Some(PublishFailure::Expired));
        assert!(f.requests_to(MEDIA).is_empty());
        assert_eq!(f.instagram.account(), Err(PlatformError::Expired));
    });
}

#[test]
fn a_fresh_token_is_not_renewed_and_counts_sixty_days_from_when_it_was_pasted() {
    block_on(async {
        let f = Fixture::connected().await;

        f.instagram.account().unwrap();

        assert!(f.requests_to(REFRESH).is_empty());
        assert_eq!(
            f.instagram.valid_until().unwrap(),
            Some(f.clock.now() + TimeDelta::days(INSTAGRAM_TOKEN_DAYS))
        );
    });
}

#[test]
fn the_token_is_renewed_only_inside_the_window_and_kept_before_it_is_used() {
    block_on(async {
        let f = Fixture::connected().await;
        // A day old but far from running out: not yet.
        f.term(TimeDelta::days(2), TimeDelta::days(58));
        f.instagram.account().unwrap();
        // Close to running out but not a day old: Instagram would refuse.
        f.term(TimeDelta::hours(23), TimeDelta::days(10));
        f.instagram.account().unwrap();
        assert!(f.requests_to(REFRESH).is_empty());

        // A day old and less than 30 days left.
        f.term(TimeDelta::days(31), TimeDelta::days(29));
        let account = f.instagram.account().unwrap();

        assert_eq!(account.username, "mascate.ofertas");
        let renewal = &f.requests_to(REFRESH)[0];
        assert!(renewal.path.contains("grant_type=ig_refresh_token"));
        assert!(renewal.path.contains(&format!("access_token={TOKEN}")));
        assert_eq!(f.stored("INSTAGRAM_ACCESS_TOKEN").as_deref(), Some(RENEWED));
        let until: Timestamp = f.clock.now() + TimeDelta::seconds(5_183_944);
        assert_eq!(f.instagram.valid_until().unwrap(), Some(until));
        let me = f.requests_to(ME);
        assert_eq!(
            me.last().unwrap().authorization.as_deref(),
            Some(format!("Bearer {RENEWED}").as_str())
        );
        // Renewed, the next call does not renew again.
        f.instagram.account().unwrap();
        assert_eq!(f.requests_to(REFRESH).len(), 1);
    });
}

#[test]
fn a_renewal_instagram_refuses_means_the_token_expired() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server
            .serve_any_query(REFRESH, 400, fixture("error-190.json"));
        f.term(TimeDelta::days(50), TimeDelta::days(10));

        assert_eq!(f.instagram.account(), Err(PlatformError::Expired));
        assert_eq!(f.stored("INSTAGRAM_ACCESS_TOKEN").as_deref(), Some(TOKEN));
    });
}

#[test]
fn a_renewal_that_fails_for_a_moment_keeps_the_valid_token() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server
            .serve_any_query(REFRESH, 500, fixture("error-500.json"));
        f.term(TimeDelta::days(50), TimeDelta::days(10));

        let account = f.instagram.account().unwrap();

        assert_eq!(account.user_id, USER);
        assert_eq!(f.stored("INSTAGRAM_ACCESS_TOKEN").as_deref(), Some(TOKEN));
        assert_eq!(
            f.requests_to(ME)[0].authorization.as_deref(),
            Some(format!("Bearer {TOKEN}").as_str())
        );
    });
}

#[test]
fn a_photo_from_the_folder_or_none_never_reaches_instagram() {
    block_on(async {
        let f = Fixture::connected().await;
        let channel = f
            .channels
            .create(&NewPromotionChannel {
                name: "Telegram".into(),
                kind: ChannelKind::TelegramChannel,
                default_tag: "telegram".into(),
                keywords: Vec::new(),
                telegram_chat: Some("@ofertas".into()),
            })
            .await
            .unwrap();
        // The Marketing blocks these on an Instagram channel; the adapter
        // refuses them anyway, before any request.
        for image in [Some(PostImage::File("foto.jpg".into())), None] {
            let facts = OfferFacts {
                link_id: RecordId::from_u128(900),
                link: LINK.into(),
                product: "Curso".into(),
                platform: "Hotmart".into(),
                price: None,
                image,
                price_needs_time: false,
                forbidden_contacts: Vec::new(),
            };
            let draft = f.posts.draft(&facts, &channel).await.unwrap();
            let post = f.posts.create(&draft).await.unwrap();
            let request = f.posts.prepare(post.id).await.unwrap();
            let failed = f
                .posts
                .publish(f.instagram.as_ref(), request.confirm())
                .await
                .unwrap();
            assert_eq!(failed.status, PostStatus::Failed);
            assert!(matches!(failed.failure, Some(PublishFailure::NotSent(_))));
        }
        assert!(f.server.requests().is_empty());
    });
}

#[test]
fn without_a_token_nothing_is_asked() {
    block_on(async {
        let f = Fixture::bare().await;

        assert_eq!(f.instagram.account(), Err(PlatformError::NotConnected));
        assert_eq!(f.instagram.valid_until(), Ok(None));
        assert!(f.server.requests().is_empty());
    });
}

#[test]
fn checking_names_the_account_the_token_reaches() {
    block_on(async {
        let f = Fixture::connected().await;

        let check = f.instagram.check("").unwrap();

        assert_eq!(check.title, "@mascate.ofertas");
        assert_eq!(check.standing, DestinationStanding::CanPost);
        assert_eq!(
            f.instagram.account().unwrap(),
            InstagramAccount {
                user_id: USER.into(),
                username: "mascate.ofertas".into()
            }
        );
    });
}

#[test]
fn a_published_post_is_never_sent_again() {
    block_on(async {
        let f = Fixture::connected().await;
        let post = f.post(platform_photo()).await;
        f.publish(&post).await;

        assert!(f.posts.prepare(post.id).await.is_err());
        assert_eq!(f.requests_to(PUBLISH).len(), 1);
    });
}
