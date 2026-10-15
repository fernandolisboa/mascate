//! Telegram as a `PostDestination` (#103) against the fake server serving
//! answers written from the Bot API's documentation (see the fixtures'
//! README), with marketing's `OfferPosts` end to end on a real temporary
//! database: what each kind of post sends, how each answer leaves the
//! post, and that a post whose fate is unknown is never sent twice.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_integrations::{Pause, Telegram};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{PlatformError, RecordId};
use mascate_marketing::{
    ChannelKind, DestinationStanding, MIGRATIONS, NewPromotionChannel, OfferFacts, OfferPost,
    OfferPosts, PostDestination, PostImage, PostStatus, PromotionChannels, PublishFailure,
};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore, Request};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use serde_json::{Value, json};

const TOKEN: &str = "7712345678:AAHk3xQ-mascate_TEST-token0123456789";
const PUBLIC: &str = "@ofertasdofernando";
const PRIVATE: &str = "-1001234567890";
const LINK: &str = "https://go.hotmart.com/X12345678Y?src=telegram";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/telegram/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

fn path(method: &str) -> String {
    format!("/bot{TOKEN}/{method}")
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
    clock: Arc<ManualClock>,
    server: FakeHttpServer,
    pauses: Arc<RecordedPauses>,
    telegram: Telegram,
    channels: PromotionChannels,
    posts: OfferPosts,
}

impl Fixture {
    async fn new() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        store
            .write("TELEGRAM_BOT_TOKEN", &Secret::new(TOKEN))
            .unwrap();
        Self::with(server, store).await
    }

    async fn with(server: FakeHttpServer, store: Arc<MemorySecretStore>) -> Self {
        Self::pointed_at(server.url().to_owned(), server, store).await
    }

    async fn pointed_at(
        api: String,
        server: FakeHttpServer,
        store: Arc<MemorySecretStore>,
    ) -> Self {
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
        let pauses = Arc::new(RecordedPauses::default());
        Self {
            _dir: dir,
            telegram: Telegram::new(&api, "Mascate/test", store)
                .with_pause(pauses.clone())
                .with_answer_timeout(Duration::from_millis(500)),
            channels: PromotionChannels::new(database.clone(), clock.clone(), ids.clone()),
            posts: OfferPosts::new(database, clock.clone(), ids),
            clock,
            server,
            pauses,
        }
    }

    /// A saved post on a Telegram channel at `chat`.
    async fn post(&self, chat: &str, image: Option<PostImage>) -> OfferPost {
        let channel = self
            .channels
            .create(&NewPromotionChannel {
                name: "Ofertas do Fernando".into(),
                kind: ChannelKind::TelegramChannel,
                default_tag: format!("telegram{}", self.channels.channels().await.unwrap().len()),
                keywords: Vec::new(),
                telegram_chat: Some(chat.into()),
            })
            .await
            .unwrap();
        let facts = OfferFacts {
            link_id: RecordId::from_u128(900),
            link: LINK.into(),
            product: "Curso de Fotografia".into(),
            platform: "Hotmart".into(),
            price: None,
            image,
            price_needs_time: false,
            forbidden_contacts: Vec::new(),
        };
        let draft = self.posts.draft(&facts, &channel).await.unwrap();
        self.posts.create(&draft).await.unwrap()
    }

    /// Prepares, confirms and publishes `post` through Telegram.
    async fn publish(&self, post: &OfferPost) -> OfferPost {
        self.clock.advance(TimeDelta::seconds(1));
        let request = self.posts.prepare(post.id).await.unwrap();
        self.posts
            .publish(&self.telegram, request.confirm())
            .await
            .unwrap()
    }

    fn sent(&self) -> Vec<Request> {
        self.server.received()
    }
}

fn json_body(request: &Request) -> Value {
    assert_eq!(
        header(request, "content-type").as_deref(),
        Some("application/json")
    );
    serde_json::from_str(&request.body).unwrap()
}

fn header(request: &Request, name: &str) -> Option<String> {
    request
        .headers
        .iter()
        .find(|(header, _)| header == name)
        .map(|(_, value)| value.clone())
}

#[test]
fn a_text_post_goes_as_a_message_without_parse_mode_and_keeps_its_link() {
    block_on(async {
        let f = Fixture::new().await;
        f.server
            .serve(&path("sendMessage"), 200, fixture("send-message.json"));
        let post = f.post(PUBLIC, None).await;

        let published = f.publish(&post).await;
        assert_eq!(published.status, PostStatus::Sent);
        let sent = published.sent.unwrap();
        assert_eq!(sent.message_id.as_deref(), Some("128"));
        assert_eq!(
            sent.link.as_deref(),
            Some("https://t.me/ofertasdofernando/128")
        );

        let requests = f.sent();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "POST");
        assert_eq!(requests[0].path, path("sendMessage"));
        assert_eq!(
            json_body(&requests[0]),
            json!({ "chat_id": PUBLIC, "text": post.draft.text })
        );
    });
}

#[test]
fn a_private_channel_takes_its_id_as_a_number_and_links_through_c() {
    block_on(async {
        let f = Fixture::new().await;
        f.server.serve(
            &path("sendMessage"),
            200,
            fixture("send-message-private.json"),
        );
        let post = f.post(PRIVATE, None).await;

        let published = f.publish(&post).await;
        assert_eq!(
            published.sent.unwrap().link.as_deref(),
            Some("https://t.me/c/1234567890/57")
        );
        assert_eq!(
            json_body(&f.sent()[0])["chat_id"],
            json!(-1001234567890_i64)
        );
    });
}

#[test]
fn a_platform_picture_goes_by_its_url_with_the_text_as_caption() {
    block_on(async {
        let f = Fixture::new().await;
        f.server
            .serve(&path("sendPhoto"), 200, fixture("send-photo.json"));
        let url = "https://cf.shopee.com.br/file/br-11134207-abc";
        let post = f.post(PUBLIC, Some(PostImage::Url(url.into()))).await;

        let published = f.publish(&post).await;
        assert_eq!(published.status, PostStatus::Sent);
        assert_eq!(published.sent.unwrap().message_id.as_deref(), Some("129"));
        assert_eq!(
            json_body(&f.sent()[0]),
            json!({ "chat_id": PUBLIC, "photo": url, "caption": post.draft.text })
        );
    });
}

/// A photo name with a quote, which the multipart header must not carry
/// as is; Windows refuses quotes in file names, so there it is plain.
#[cfg(not(windows))]
const PHOTO_NAME: &str = "fone \"azul\".JPG";
#[cfg(not(windows))]
const SENT_NAME: &str = "fone _azul_.JPG";
#[cfg(windows)]
const PHOTO_NAME: &str = "fone azul.JPG";
#[cfg(windows)]
const SENT_NAME: &str = PHOTO_NAME;

#[test]
fn a_product_photo_is_uploaded_as_multipart_with_its_bytes_name_and_type() {
    block_on(async {
        let f = Fixture::new().await;
        f.server
            .serve(&path("sendPhoto"), 200, fixture("send-photo.json"));
        let folder = tempfile::tempdir().unwrap();
        let photo = folder.path().join(PHOTO_NAME);
        // A JPEG's first bytes, which are no UTF-8, and a CRLF inside.
        let bytes: Vec<u8> = [
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, b'\r', b'\n', 0x80, 0xFF, 0xD9,
        ]
        .repeat(50);
        std::fs::write(&photo, &bytes).unwrap();
        let post = f.post(PRIVATE, Some(PostImage::File(photo))).await;

        let published = f.publish(&post).await;
        assert_eq!(published.status, PostStatus::Sent);

        let request = &f.sent()[0];
        assert_eq!(request.path, path("sendPhoto"));
        let content_type = header(request, "content-type").unwrap();
        let boundary = content_type
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap();
        let parts = multipart_parts(&request.bytes, boundary);
        assert_eq!(parts.len(), 3);
        assert_eq!(
            parts[0],
            (
                "Content-Disposition: form-data; name=\"chat_id\"".to_owned(),
                PRIVATE.as_bytes().to_vec()
            )
        );
        assert_eq!(
            parts[1],
            (
                "Content-Disposition: form-data; name=\"caption\"".to_owned(),
                post.draft.text.as_bytes().to_vec()
            )
        );
        assert_eq!(
            parts[2],
            (
                format!(
                    "Content-Disposition: form-data; name=\"photo\"; filename=\"{SENT_NAME}\"\r\n\
                     Content-Type: image/jpeg"
                ),
                bytes
            )
        );
    });
}

/// Each part of a multipart body: its headers and its content.
fn multipart_parts(body: &[u8], boundary: &str) -> Vec<(String, Vec<u8>)> {
    let delimiter = format!("--{boundary}");
    let closing = format!("\r\n--{boundary}--\r\n");
    assert!(
        body.starts_with(delimiter.as_bytes()),
        "opens with the boundary"
    );
    assert!(
        body.ends_with(closing.as_bytes()),
        "closes with the boundary"
    );
    let inner = &body[delimiter.len()..body.len() - closing.len()];
    let separator = format!("\r\n--{boundary}");
    split(inner, separator.as_bytes())
        .into_iter()
        .map(|part| {
            let part = part.strip_prefix(b"\r\n").expect("each part starts a line");
            let end = find(part, b"\r\n\r\n").expect("headers end with a blank line");
            (
                String::from_utf8(part[..end].to_vec()).unwrap(),
                part[end + 4..].to_vec(),
            )
        })
        .collect()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn split<'a>(mut bytes: &'a [u8], separator: &[u8]) -> Vec<&'a [u8]> {
    let mut parts = Vec::new();
    while let Some(at) = find(bytes, separator) {
        parts.push(&bytes[..at]);
        bytes = &bytes[at + separator.len()..];
    }
    parts.push(bytes);
    parts
}

#[test]
fn a_photo_telegram_does_not_take_fails_before_anything_is_sent() {
    block_on(async {
        let f = Fixture::new().await;
        let folder = tempfile::tempdir().unwrap();
        let gif = folder.path().join("fone.gif");
        std::fs::write(&gif, b"GIF89a").unwrap();
        let big = folder.path().join("fone.png");
        std::fs::write(&big, vec![0u8; 10 * 1024 * 1024 + 1]).unwrap();
        let missing = folder.path().join("sumiu.jpg");

        for photo in [gif, big, missing] {
            let post = f.post(PUBLIC, Some(PostImage::File(photo.clone()))).await;
            let failed = f.publish(&post).await;
            assert_eq!(failed.status, PostStatus::Failed, "{photo:?}");
            assert!(
                matches!(failed.failure, Some(PublishFailure::NotSent(_))),
                "{photo:?}: {:?}",
                failed.failure
            );
        }
        assert!(f.sent().is_empty());
    });
}

#[test]
fn a_429_waits_what_telegram_asks_and_tries_again() {
    block_on(async {
        let f = Fixture::new().await;
        f.server
            .serve(&path("sendMessage"), 429, fixture("error-429.json"));
        f.server
            .then_serve(&path("sendMessage"), 200, fixture("send-message.json"));
        let post = f.post(PUBLIC, None).await;

        let published = f.publish(&post).await;
        assert_eq!(published.status, PostStatus::Sent);
        assert_eq!(f.pauses.seconds(), vec![3]);
        assert_eq!(f.sent().len(), 2);
    });
}

#[test]
fn a_429_that_keeps_coming_or_asks_too_long_fails_without_publishing() {
    block_on(async {
        let f = Fixture::new().await;
        f.server
            .serve(&path("sendMessage"), 429, fixture("error-429.json"));
        let post = f.post(PUBLIC, None).await;
        let failed = f.publish(&post).await;
        assert_eq!(failed.status, PostStatus::Failed);
        assert_eq!(failed.failure, Some(PublishFailure::RateLimited));
        assert_eq!(f.pauses.seconds(), vec![3, 3, 3]);
        assert_eq!(f.sent().len(), 4);

        let f = Fixture::new().await;
        f.server
            .serve(&path("sendMessage"), 429, fixture("error-429-long.json"));
        let post = f.post(PUBLIC, None).await;
        let failed = f.publish(&post).await;
        assert_eq!(failed.failure, Some(PublishFailure::RateLimited));
        assert!(f.pauses.seconds().is_empty());
        assert_eq!(f.sent().len(), 1);
    });
}

#[test]
fn a_refusal_fails_with_telegrams_reason_and_a_rejected_token_expires() {
    block_on(async {
        for (status, file, expected) in [
            (
                400,
                "error-400-chat-not-found.json",
                PublishFailure::Refused("Bad Request: chat not found".into()),
            ),
            (
                403,
                "error-403-not-a-member.json",
                PublishFailure::Refused(
                    "Forbidden: bot is not a member of the channel chat".into(),
                ),
            ),
            (401, "error-401.json", PublishFailure::Expired),
            (404, "error-404.json", PublishFailure::Expired),
        ] {
            let f = Fixture::new().await;
            f.server.serve(&path("sendMessage"), status, fixture(file));
            let post = f.post(PUBLIC, None).await;
            let failed = f.publish(&post).await;
            assert_eq!(failed.status, PostStatus::Failed, "{file}");
            assert_eq!(failed.failure, Some(expected), "{file}");
            assert_eq!(f.sent().len(), 1, "{file}");
            // Nothing went out: the owner can publish again.
            assert!(f.posts.prepare(post.id).await.is_ok());
        }
    });
}

#[test]
fn a_post_whose_fate_is_unknown_is_unconfirmed_and_never_sent_twice() {
    block_on(async {
        let late = FakeHttpServer::start();
        late.serve_late(
            &path("sendMessage"),
            Duration::from_secs(3),
            200,
            fixture("send-message.json"),
        );
        let hung_up = FakeHttpServer::start();
        hung_up.hang_up(&path("sendMessage"));
        let failing = FakeHttpServer::start();
        failing.serve(&path("sendMessage"), 500, fixture("error-500.json"));
        let proxied = FakeHttpServer::start();
        proxied.serve(&path("sendMessage"), 502, fixture("error-502.html"));

        for (case, server) in [
            ("timeout", late),
            ("dropped", hung_up),
            ("500", failing),
            ("502", proxied),
        ] {
            let store = Arc::new(MemorySecretStore::default());
            store
                .write("TELEGRAM_BOT_TOKEN", &Secret::new(TOKEN))
                .unwrap();
            let f = Fixture::with(server, store).await;
            let post = f.post(PUBLIC, None).await;
            let unknown = f.publish(&post).await;
            assert_eq!(unknown.status, PostStatus::Unconfirmed, "{case}");
            let failure = unknown.failure.unwrap();
            assert!(
                matches!(failure, PublishFailure::Unconfirmed(_)),
                "{case}: {failure:?}"
            );
            assert!(!failure.to_string().contains(TOKEN), "{case}: {failure}");
            assert_eq!(f.sent().len(), 1, "{case}");
            assert!(f.pauses.seconds().is_empty(), "{case}");
        }
    });
}

#[test]
fn a_telegram_out_of_reach_publishes_nothing_and_says_so() {
    block_on(async {
        // A port nobody listens on.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        let store = Arc::new(MemorySecretStore::default());
        store
            .write("TELEGRAM_BOT_TOKEN", &Secret::new(TOKEN))
            .unwrap();
        let f = Fixture::pointed_at(api, FakeHttpServer::start(), store).await;
        let post = f.post(PUBLIC, None).await;
        let failed = f.publish(&post).await;
        assert_eq!(failed.status, PostStatus::Failed);
        let failure = failed.failure.unwrap();
        assert!(matches!(failure, PublishFailure::NotSent(_)), "{failure:?}");
        assert!(!failure.to_string().contains(TOKEN), "{failure}");
    });
}

#[test]
fn without_a_token_nothing_is_asked() {
    block_on(async {
        let f = Fixture::with(
            FakeHttpServer::start(),
            Arc::new(MemorySecretStore::default()),
        )
        .await;
        let post = f.post(PUBLIC, None).await;
        let failed = f.publish(&post).await;
        assert_eq!(failed.failure, Some(PublishFailure::NotConnected));
        assert_eq!(f.telegram.check(PUBLIC), Err(PlatformError::NotConnected));
        assert!(f.sent().is_empty());
    });
}

#[test]
fn checking_a_channel_says_whether_the_bot_can_post_there() {
    for (chat_file, member_file, expected) in [
        (
            "get-chat-public.json",
            Some("chat-member-administrator.json"),
            DestinationStanding::CanPost,
        ),
        (
            "get-chat-private.json",
            Some("chat-member-administrator-without-posting.json"),
            DestinationStanding::CannotPost,
        ),
        (
            "get-chat-public.json",
            Some("chat-member-left.json"),
            DestinationStanding::NotAdministrator,
        ),
        (
            "get-chat-group.json",
            None,
            DestinationStanding::NotAChannel,
        ),
    ] {
        let f = block_on(Fixture::new());
        f.server.serve(&path("getMe"), 200, fixture("get-me.json"));
        f.server.serve(&path("getChat"), 200, fixture(chat_file));
        if let Some(member) = member_file {
            f.server.serve(&path("getChatMember"), 200, fixture(member));
        }
        let check = f.telegram.check(PUBLIC).unwrap();
        assert_eq!(check.standing, expected, "{chat_file} {member_file:?}");
        let requests = f.sent();
        assert_eq!(json_body(&requests[1]), json!({ "chat_id": PUBLIC }));
        if member_file.is_some() {
            assert_eq!(
                json_body(&requests[2]),
                json!({ "chat_id": PUBLIC, "user_id": 7712345678_i64 })
            );
        } else {
            assert_eq!(requests.len(), 2);
        }
    }

    let f = block_on(Fixture::new());
    f.server.serve(&path("getMe"), 200, fixture("get-me.json"));
    f.server
        .serve(&path("getChat"), 200, fixture("get-chat-public.json"));
    f.server.serve(
        &path("getChatMember"),
        200,
        fixture("chat-member-administrator.json"),
    );
    assert_eq!(
        f.telegram.check(PUBLIC).unwrap().title,
        "Ofertas do Fernando"
    );

    let f = block_on(Fixture::new());
    f.server
        .serve(&path("getMe"), 401, fixture("error-401.json"));
    assert_eq!(f.telegram.check(PUBLIC), Err(PlatformError::Expired));

    let f = block_on(Fixture::new());
    f.server.serve(&path("getMe"), 200, fixture("get-me.json"));
    f.server.serve(
        &path("getChat"),
        400,
        fixture("error-400-chat-not-found.json"),
    );
    assert_eq!(
        f.telegram.check(PRIVATE),
        Err(PlatformError::Refused("Bad Request: chat not found".into()))
    );
    assert_eq!(
        json_body(&f.sent()[1]),
        json!({ "chat_id": -1001234567890_i64 })
    );
}

#[test]
fn a_photo_url_telegram_cannot_fetch_fails_with_its_reason() {
    block_on(async {
        let f = Fixture::new().await;
        f.server.serve(
            &path("sendPhoto"),
            400,
            fixture("error-400-wrong-photo-url.json"),
        );
        let post = f
            .post(
                PUBLIC,
                Some(PostImage::Url("https://example.com/x.jpg".into())),
            )
            .await;
        let failed = f.publish(&post).await;
        assert_eq!(
            failed.failure,
            Some(PublishFailure::Refused(
                "Bad Request: wrong HTTP URL specified".into()
            ))
        );
    });
}
