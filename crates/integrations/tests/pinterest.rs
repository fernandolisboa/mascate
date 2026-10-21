//! Pinterest as a `PostDestination` (#155) against the fake server serving
//! answers written from API v5's documentation (see the fixtures' README),
//! with marketing's `OfferPosts` end to end on a real temporary database:
//! the login traded and renewed one at a time, the boards listed across
//! pages, what each Pin sends, how each answer leaves the post, and that a
//! Pin whose fate is unknown is never sent twice.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_integrations::{
    PINTEREST_REDIRECT, PINTEREST_SCOPES, Pause, Pinterest, PinterestAuthorization,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{PlatformError, RecordId};
use mascate_marketing::{
    ChannelKind, DestinationStanding, MIGRATIONS, NewPromotionChannel, OfferFacts, OfferPost,
    OfferPosts, PinterestBoard, PostDestination, PostImage, PostStatus, PromotionChannels,
    PublishFailure,
};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore, Request};
use mascate_platform::{Database, Secret, SecretStore, migrate};
use serde_json::{Value, json};

const APP_ID: &str = "1501234";
const APP_SECRET: &str = "3f0a9c-segredo-do-app";
const FIRST_REFRESH: &str = "pinr_MASCATE_primeiro_refresh_0123456789";
const SECOND_REFRESH: &str = "pinr_MASCATE_segundo_refresh_0123456789";
const FIRST_ACCESS: &str = "pina_MASCATE_primeiro_acesso_0123456789";
const SECOND_ACCESS: &str = "pina_MASCATE_segundo_acesso_0123456789";
const BOARD: &str = "549755885175";
const LINK: &str = "https://go.hotmart.com/X12345678Y?src=pinterest";

const TOKEN: &str = "/v5/oauth/token";
const BOARDS: &str = "/v5/boards";
const PINS: &str = "/v5/pins";

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/fixtures/pinterest/{name}",
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
    pauses: Arc<RecordedPauses>,
    pinterest: Arc<Pinterest>,
    channels: PromotionChannels,
    posts: OfferPosts,
}

impl Fixture {
    /// The app's keys and the owner's login stored, every renewal answered.
    async fn connected() -> Self {
        let f = Self::with_keys().await;
        f.store
            .write("PINTEREST_REFRESH_TOKEN", &Secret::new(FIRST_REFRESH))
            .unwrap();
        f
    }

    /// The app's keys pasted, with no login yet.
    async fn with_keys() -> Self {
        let server = FakeHttpServer::start();
        server.serve(TOKEN, 200, fixture("token.json"));
        let store = Arc::new(MemorySecretStore::default());
        store
            .write("PINTEREST_APP_ID", &Secret::new(APP_ID))
            .unwrap();
        store
            .write("PINTEREST_APP_SECRET", &Secret::new(APP_SECRET))
            .unwrap();
        Self::pointed_at(server.url().to_owned(), server, store).await
    }

    async fn pointed_at(
        api: String,
        server: FakeHttpServer,
        store: Arc<MemorySecretStore>,
    ) -> Self {
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
            pinterest: Arc::new(
                Pinterest::new(
                    &api,
                    &format!("{api}/oauth/"),
                    "Mascate/test",
                    store.clone(),
                    clock.clone(),
                )
                .with_pause(pauses.clone())
                .with_answer_timeout(Duration::from_millis(500)),
            ),
            channels: PromotionChannels::new(database.clone(), clock.clone(), ids.clone()),
            posts: OfferPosts::new(database, clock, ids),
            _dir: dir,
            server,
            store,
            pauses,
        }
    }

    fn stored(&self, name: &str) -> Option<String> {
        self.store
            .read(name)
            .unwrap()
            .map(|secret| secret.expose().to_owned())
    }

    fn requests_to(&self, path: &str) -> Vec<Request> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.path.split('?').next() == Some(path))
            .collect()
    }

    fn renewals(&self) -> usize {
        self.requests_to(TOKEN).len()
    }

    fn pins(&self) -> Vec<Value> {
        self.requests_to(PINS)
            .iter()
            .map(|request| serde_json::from_str(&request.body).unwrap())
            .collect()
    }

    /// A saved post with `image` on a Pinterest channel posting to the
    /// Ofertas board.
    async fn post(&self, image: Option<PostImage>) -> OfferPost {
        let channel = self
            .channels
            .create(&NewPromotionChannel {
                name: "Pinterest do Fernando".into(),
                kind: ChannelKind::Pinterest,
                default_tag: format!("pinterest{}", self.channels.channels().await.unwrap().len()),
                keywords: Vec::new(),
                telegram_chat: None,
            })
            .await
            .unwrap();
        let channel = self
            .channels
            .set_pinterest_board(
                channel.id,
                &PinterestBoard {
                    id: BOARD.into(),
                    name: "Ofertas".into(),
                },
            )
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
            .publish(self.pinterest.as_ref(), request.confirm())
            .await
            .unwrap()
    }
}

fn platform_photo() -> Option<PostImage> {
    Some(PostImage::Url(
        "https://cf.shopee.com.br/file/abc.jpg".into(),
    ))
}

fn form(request: &Request) -> Vec<(String, String)> {
    request
        .body
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

#[test]
fn the_login_page_asks_for_the_boards_and_pins_and_returns_to_localhost() {
    block_on(async {
        let f = Fixture::with_keys().await;

        let page = f.pinterest.authorization_url().unwrap();

        assert_eq!(
            page,
            format!(
                "{}/oauth/?client_id={APP_ID}&redirect_uri=http%3A%2F%2Flocalhost%2F&\
                 response_type=code&scope=boards%3Aread%2Cboards%3Awrite%2Cpins%3Aread%2C\
                 pins%3Awrite",
                f.server.url()
            )
        );
        assert_eq!(PINTEREST_REDIRECT, "http://localhost/");
        assert_eq!(
            PINTEREST_SCOPES,
            "boards:read,boards:write,pins:read,pins:write"
        );
        let empty = Fixture::pointed_at(
            "http://127.0.0.1:9".into(),
            FakeHttpServer::start(),
            Arc::new(MemorySecretStore::default()),
        )
        .await;
        assert_eq!(
            empty.pinterest.authorization_url(),
            Err(PlatformError::NotConnected)
        );
    });
}

#[test]
fn the_code_comes_from_the_address_the_browser_stopped_at() {
    assert_eq!(
        PinterestAuthorization::from_landing(" http://localhost/?code=c0de-1.2_3~&state=x "),
        Some(PinterestAuthorization {
            code: "c0de-1.2_3~".into()
        })
    );
    assert_eq!(
        PinterestAuthorization::from_landing("code=abc"),
        Some(PinterestAuthorization { code: "abc".into() })
    );
    for wrong in [
        "http://localhost/",
        "http://localhost/?error=access_denied",
        "http://localhost/?code=",
        "http://localhost/?code=a%20b",
    ] {
        assert_eq!(PinterestAuthorization::from_landing(wrong), None, "{wrong}");
    }
}

#[test]
fn the_code_is_traded_once_with_the_apps_keys_and_its_refresh_token_kept_first() {
    block_on(async {
        let f = Fixture::with_keys().await;
        f.server
            .serve_any_query(BOARDS, 200, fixture("boards-page-2.json"));

        f.pinterest
            .authorize(&PinterestAuthorization {
                code: "c0de".into(),
            })
            .unwrap();

        assert_eq!(
            f.stored("PINTEREST_REFRESH_TOKEN").as_deref(),
            Some(FIRST_REFRESH)
        );
        let trade = &f.requests_to(TOKEN)[0];
        // base64("1501234:3f0a9c-segredo-do-app")
        assert_eq!(
            trade.authorization.as_deref(),
            Some("Basic MTUwMTIzNDozZjBhOWMtc2VncmVkby1kby1hcHA=")
        );
        assert_eq!(
            form(trade),
            [
                ("grant_type".into(), "authorization_code".into()),
                ("code".into(), "c0de".into()),
                ("redirect_uri".into(), "http%3A%2F%2Flocalhost%2F".into()),
            ]
        );
        // The login just traded serves the next call: no renewal.
        assert_eq!(f.pinterest.boards().unwrap().len(), 1);
        assert_eq!(f.renewals(), 1);
        assert_eq!(
            f.requests_to(BOARDS)[0].authorization.as_deref(),
            Some(format!("Bearer {FIRST_ACCESS}").as_str())
        );
    });
}

#[test]
fn a_refused_code_keeps_no_login_and_says_why() {
    block_on(async {
        let f = Fixture::with_keys().await;
        f.server.serve(TOKEN, 400, fixture("token-refused.json"));

        let refused = f.pinterest.authorize(&PinterestAuthorization {
            code: "velho".into(),
        });

        assert_eq!(
            refused,
            Err(PlatformError::Refused("Invalid authorization code.".into()))
        );
        assert_eq!(f.stored("PINTEREST_REFRESH_TOKEN"), None);
        assert!(f.pauses.seconds().is_empty());
    });
}

#[test]
fn the_boards_are_listed_across_pages_by_their_bookmark() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.serve(
            "/v5/boards?page_size=250",
            200,
            fixture("boards-page-1.json"),
        );
        f.server.serve(
            "/v5/boards?page_size=250&bookmark=Y2Fmw6kgZSBjb3F1ZXRlbA",
            200,
            fixture("boards-page-2.json"),
        );

        let boards = f.pinterest.boards().unwrap();

        let board = |id: &str, name: &str| PinterestBoard {
            id: id.into(),
            name: name.into(),
        };
        assert_eq!(
            boards,
            [
                board("549755885175", "Ofertas"),
                board("549755885176", "Cozinha"),
                board("549755885177", "Café"),
            ]
        );
        assert_eq!(f.requests_to(BOARDS).len(), 2);
    });
}

#[test]
fn a_renewal_keeps_the_next_refresh_token_before_using_the_new_access() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.serve(TOKEN, 200, fixture("token-renewed.json"));
        f.server
            .serve_any_query(BOARDS, 200, fixture("boards-page-2.json"));

        f.pinterest.boards().unwrap();

        let renewal = &f.requests_to(TOKEN)[0];
        assert_eq!(
            form(renewal),
            [
                ("grant_type".into(), "refresh_token".into()),
                ("refresh_token".into(), FIRST_REFRESH.into()),
            ]
        );
        assert_eq!(
            f.stored("PINTEREST_REFRESH_TOKEN").as_deref(),
            Some(SECOND_REFRESH)
        );
        assert_eq!(
            f.requests_to(BOARDS)[0].authorization.as_deref(),
            Some(format!("Bearer {SECOND_ACCESS}").as_str())
        );
    });
}

#[test]
fn two_calls_racing_for_a_login_renew_it_once() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.serve_late(
            TOKEN,
            Duration::from_millis(300),
            200,
            fixture("token-renewed.json"),
        );
        f.server
            .serve_any_query(BOARDS, 200, fixture("boards-page-2.json"));

        let threads: Vec<_> = (0..2)
            .map(|_| {
                let pinterest = f.pinterest.clone();
                std::thread::spawn(move || pinterest.boards())
            })
            .collect();
        for thread in threads {
            assert_eq!(thread.join().unwrap().unwrap().len(), 1);
        }

        // A second renewal would spend the refresh token the first one got.
        assert_eq!(f.renewals(), 1);
    });
}

#[test]
fn a_refused_renewal_means_the_login_expired() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.serve(TOKEN, 400, fixture("token-refused.json"));

        assert_eq!(f.pinterest.boards(), Err(PlatformError::Expired));
        let post = f.post(platform_photo()).await;
        let failed = f.publish(&post).await;
        assert_eq!(failed.status, PostStatus::Failed);
        assert_eq!(failed.failure, Some(PublishFailure::Expired));
        assert!(f.pins().is_empty());
    });
}

#[test]
fn a_pin_with_the_platforms_photo_goes_as_its_address_with_the_whole_link() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.serve(PINS, 201, fixture("pin.json"));
        let post = f.post(platform_photo()).await;

        let published = f.publish(&post).await;

        assert_eq!(
            f.pins(),
            [json!({
                "board_id": BOARD,
                "title": "Curso de Fotografia com Celular",
                "description": post.draft.text,
                "alt_text": "Curso de Fotografia com Celular",
                "link": LINK,
                "media_source": {
                    "source_type": "image_url",
                    "url": "https://cf.shopee.com.br/file/abc.jpg",
                },
            })]
        );
        assert_eq!(
            f.requests_to(PINS)[0].authorization.as_deref(),
            Some(format!("Bearer {FIRST_ACCESS}").as_str())
        );
        assert_eq!(published.status, PostStatus::Sent);
        let sent = published.sent.unwrap();
        assert_eq!(sent.message_id.as_deref(), Some("813744226420795884"));
        assert_eq!(
            sent.link.as_deref(),
            Some("https://www.pinterest.com/pin/813744226420795884/")
        );
    });
}

#[test]
fn a_pin_with_a_folder_photo_goes_as_its_bytes_in_base64() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.serve(PINS, 201, fixture("pin.json"));
        let folder = tempfile::tempdir().unwrap();
        let photo: PathBuf = folder.path().join("caneca.PNG");
        std::fs::write(&photo, b"foobar").unwrap();
        let post = f.post(Some(PostImage::File(photo))).await;

        assert_eq!(f.publish(&post).await.status, PostStatus::Sent);

        assert_eq!(
            f.pins()[0]["media_source"],
            json!({
                "source_type": "image_base64",
                "content_type": "image/png",
                "data": "Zm9vYmFy",
            })
        );
    });
}

#[test]
fn a_photo_pinterest_cannot_take_sends_nothing() {
    block_on(async {
        let f = Fixture::connected().await;
        let folder = tempfile::tempdir().unwrap();
        let gif = folder.path().join("caneca.gif");
        std::fs::write(&gif, b"GIF89a").unwrap();
        for image in [
            PostImage::File(gif),
            PostImage::File(folder.path().join("sumiu.jpg")),
            PostImage::Url("http://cf.shopee.com.br/file/abc.jpg".into()),
        ] {
            let post = f.post(Some(image.clone())).await;
            let failed = f.publish(&post).await;
            assert_eq!(failed.status, PostStatus::Failed, "{image:?}");
            assert!(
                matches!(failed.failure, Some(PublishFailure::NotSent(_))),
                "{image:?}: {:?}",
                failed.failure
            );
        }
        assert!(f.pins().is_empty());
    });
}

#[test]
fn a_401_renews_the_login_once_and_a_second_one_means_it_expired() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.serve(PINS, 401, fixture("error-401.json"));
        f.server.then_serve(PINS, 201, fixture("pin.json"));
        f.server
            .then_serve(TOKEN, 200, fixture("token-renewed.json"));
        let post = f.post(platform_photo()).await;

        assert_eq!(f.publish(&post).await.status, PostStatus::Sent);
        assert_eq!(f.renewals(), 2);
        assert_eq!(f.pins().len(), 2);

        let again = Fixture::connected().await;
        again.server.serve(PINS, 401, fixture("error-401.json"));
        let post = again.post(platform_photo()).await;
        let expired = again.publish(&post).await;
        assert_eq!(expired.status, PostStatus::Failed);
        assert_eq!(expired.failure, Some(PublishFailure::Expired));
        assert_eq!(again.renewals(), 2);
        assert_eq!(again.pins().len(), 2);
    });
}

#[test]
fn a_429_waits_and_tries_again_and_one_that_keeps_on_gives_up() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.serve(PINS, 429, fixture("error-429.json"));
        f.server.then_serve(PINS, 201, fixture("pin.json"));
        let post = f.post(platform_photo()).await;
        assert_eq!(f.publish(&post).await.status, PostStatus::Sent);
        assert_eq!(f.pauses.seconds(), [5]);

        let busy = Fixture::connected().await;
        busy.server.serve(PINS, 429, fixture("error-429.json"));
        let post = busy.post(platform_photo()).await;
        let limited = busy.publish(&post).await;
        assert_eq!(limited.status, PostStatus::Failed);
        assert_eq!(limited.failure, Some(PublishFailure::RateLimited));
        assert_eq!(busy.pauses.seconds(), [5, 10, 20, 40]);
        assert_eq!(busy.pins().len(), 5);
    });
}

#[test]
fn a_refusal_publishes_nothing_and_keeps_pinterests_words() {
    block_on(async {
        for (status, file, why) in [
            (
                400,
                "error-400.json",
                "Invalid parameters: the link is not a valid URL.",
            ),
            (
                403,
                "error-403.json",
                "Not authorized to access board or Pin.",
            ),
            (404, "error-404.json", "Board not found."),
        ] {
            let f = Fixture::connected().await;
            f.server.serve(PINS, status, fixture(file));
            let post = f.post(platform_photo()).await;
            let failed = f.publish(&post).await;
            assert_eq!(failed.status, PostStatus::Failed, "{file}");
            assert_eq!(
                failed.failure,
                Some(PublishFailure::Refused(why.into())),
                "{file}"
            );
            assert_eq!(f.pins().len(), 1, "{file}");
            // Nothing went out: the owner can publish again.
            assert!(f.posts.prepare(post.id).await.is_ok(), "{file}");
        }
    });
}

#[test]
fn a_pin_whose_fate_is_unknown_is_unconfirmed_and_never_sent_twice() {
    block_on(async {
        for case in ["timeout", "dropped", "500"] {
            let f = Fixture::connected().await;
            match case {
                "timeout" => {
                    f.server
                        .serve_late(PINS, Duration::from_secs(3), 201, fixture("pin.json"))
                }
                "dropped" => f.server.hang_up(PINS),
                _ => f.server.serve(PINS, 500, fixture("error-500.json")),
            }
            let post = f.post(platform_photo()).await;
            let unknown = f.publish(&post).await;
            assert_eq!(unknown.status, PostStatus::Unconfirmed, "{case}");
            let failure = unknown.failure.unwrap();
            assert!(
                matches!(failure, PublishFailure::Unconfirmed(_)),
                "{case}: {failure:?}"
            );
            assert!(!failure.to_string().contains(FIRST_ACCESS), "{case}");
            assert_eq!(f.pins().len(), 1, "{case}");
            assert!(f.pauses.seconds().is_empty(), "{case}");
        }
    });
}

#[test]
fn pinterest_out_of_reach_publishes_nothing_and_says_so() {
    block_on(async {
        // A port nobody listens on.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let api = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        let store = Arc::new(MemorySecretStore::default());
        for (name, value) in [
            ("PINTEREST_APP_ID", APP_ID),
            ("PINTEREST_APP_SECRET", APP_SECRET),
            ("PINTEREST_REFRESH_TOKEN", FIRST_REFRESH),
        ] {
            store.write(name, &Secret::new(value)).unwrap();
        }
        let f = Fixture::pointed_at(api, FakeHttpServer::start(), store).await;
        let post = f.post(platform_photo()).await;
        let failed = f.publish(&post).await;
        assert_eq!(failed.status, PostStatus::Failed);
        let failure = failed.failure.unwrap();
        assert!(matches!(failure, PublishFailure::NotSent(_)), "{failure:?}");
        assert!(!failure.to_string().contains(FIRST_REFRESH), "{failure}");
    });
}

#[test]
fn without_the_login_nothing_is_asked() {
    block_on(async {
        let f = Fixture::with_keys().await;
        let post = f.post(platform_photo()).await;
        let failed = f.publish(&post).await;
        assert_eq!(failed.failure, Some(PublishFailure::NotConnected));
        assert!(f.server.received().is_empty());
    });
}

#[test]
fn checking_a_board_reads_its_name_and_a_strange_id_asks_nothing() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server
            .serve(&format!("{BOARDS}/{BOARD}"), 200, fixture("board.json"));

        let check = f.pinterest.check(BOARD).unwrap();
        assert_eq!(check.title, "Ofertas");
        assert_eq!(check.standing, DestinationStanding::CanPost);

        assert_eq!(
            f.pinterest.check("549755885199"),
            Err(PlatformError::NotFound)
        );
        let asked = f.server.received().len();
        assert_eq!(f.pinterest.check("../pins"), Err(PlatformError::NotFound));
        assert_eq!(f.server.received().len(), asked);
    });
}

#[test]
fn a_published_pin_is_never_sent_again() {
    block_on(async {
        let f = Fixture::connected().await;
        f.server.serve(PINS, 201, fixture("pin.json"));
        let post = f.post(platform_photo()).await;
        let request = f.posts.prepare(post.id).await.unwrap();
        let twin = f.posts.prepare(post.id).await.unwrap();

        f.posts
            .publish(f.pinterest.as_ref(), request.confirm())
            .await
            .unwrap();
        assert!(
            f.posts
                .publish(f.pinterest.as_ref(), twin.confirm())
                .await
                .is_err()
        );

        assert_eq!(f.pins().len(), 1);
    });
}
