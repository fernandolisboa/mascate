//! Offer Posts on a Pinterest board (#155, ADR 0035 decision 7) through
//! the Marketing module's public interface with a real temporary database
//! and a scripted destination: the channel keeps the board picked from
//! Pinterest's list, a Pin needs a photo and a description Pinterest takes,
//! and it carries the post's whole Affiliate Link, once.

use std::sync::{Arc, Mutex};

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, PlatformError, RecordId};
use mascate_marketing::{
    ChannelError, ChannelKind, ConfirmedPost, DestinationCheck, MIGRATIONS, NewPromotionChannel,
    OfferFacts, OfferPosts, PINTEREST_LIMITS, PinterestBoard, PostDestination, PostError,
    PostImage, PostProblem, PostStatus, PromotionChannel, PromotionChannels, PublishFailure,
    PublishedPost, SentPost, check_post, post_limits,
};
use mascate_platform::{Database, migrate};

const LINK: &str = "https://go.hotmart.com/X12345678Y?src=pinterest";

/// What the destination received.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Call {
    board: String,
    title: String,
    link: String,
    text: String,
    image: Option<PostImage>,
}

/// Answers each publish with `answer`, and remembers what it got.
struct Board {
    answer: Result<PublishedPost, PublishFailure>,
    calls: Mutex<Vec<Call>>,
}

impl Board {
    fn answering(answer: Result<PublishedPost, PublishFailure>) -> Self {
        Self {
            answer,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

impl PostDestination for Board {
    fn check(&self, _board: &str) -> Result<DestinationCheck, PlatformError> {
        unreachable!("the Marketing never checks on its own")
    }

    fn publish(&self, post: &ConfirmedPost) -> Result<PublishedPost, PublishFailure> {
        self.calls.lock().unwrap().push(Call {
            board: post.chat().to_owned(),
            title: post.title().to_owned(),
            link: post.link().to_owned(),
            text: post.text().to_owned(),
            image: post.image().cloned(),
        });
        self.answer.clone()
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    channels: PromotionChannels,
    posts: OfferPosts,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 21, 12, 0, 0).unwrap(),
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
            posts: OfferPosts::new(database, clock.clone(), ids),
            clock,
        }
    }

    async fn pinterest(&self) -> PromotionChannel {
        self.channels.create(&pinterest()).await.unwrap()
    }

    /// A Pinterest channel posting to the Ofertas board.
    async fn on_board(&self) -> PromotionChannel {
        let channel = self.pinterest().await;
        self.channels
            .set_pinterest_board(channel.id, &ofertas())
            .await
            .unwrap()
    }

    fn later(&self) {
        self.clock.advance(TimeDelta::seconds(1));
    }
}

fn pinterest() -> NewPromotionChannel {
    NewPromotionChannel {
        name: "Pinterest do Fernando".into(),
        kind: ChannelKind::Pinterest,
        default_tag: "pinterest".into(),
        keywords: Vec::new(),
        telegram_chat: None,
    }
}

fn ofertas() -> PinterestBoard {
    PinterestBoard {
        id: "549755885175".into(),
        name: "Ofertas".into(),
    }
}

fn facts() -> OfferFacts {
    OfferFacts {
        link_id: RecordId::from_u128(900),
        link: LINK.into(),
        product: "Curso de Fotografia com Celular".into(),
        platform: "Hotmart".into(),
        price: None,
        image: Some(PostImage::Url("https://cf.shopee.com.br/file/abc".into())),
        price_needs_time: false,
        forbidden_contacts: Vec::new(),
    }
}

fn pinned(id: &str) -> Result<PublishedPost, PublishFailure> {
    Ok(PublishedPost {
        message_id: id.into(),
        link: Some(format!("https://www.pinterest.com/pin/{id}/")),
    })
}

#[test]
fn a_pinterest_channel_keeps_the_board_picked_from_pinterests_list() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.pinterest().await;
        assert_eq!(channel.pinterest_board, None);

        let kept = f
            .channels
            .set_pinterest_board(
                channel.id,
                &PinterestBoard {
                    id: " 549755885175 ".into(),
                    name: " Ofertas ".into(),
                },
            )
            .await
            .unwrap();

        assert_eq!(kept.pinterest_board, Some(ofertas()));
        assert_eq!(f.channels.channel(channel.id).await.unwrap(), kept);
        // Editing the rest of the channel keeps its board.
        let renamed = f
            .channels
            .update(
                channel.id,
                &NewPromotionChannel {
                    name: "Pins de oferta".into(),
                    ..pinterest()
                },
            )
            .await
            .unwrap();
        assert_eq!(renamed.pinterest_board, Some(ofertas()));
    });
}

#[test]
fn only_a_pinterest_channel_takes_a_board_and_one_that_stops_being_forgets_it() {
    block_on(async {
        let f = Fixture::new().await;
        let telegram = f
            .channels
            .create(&NewPromotionChannel {
                name: "Telegram".into(),
                kind: ChannelKind::TelegramChannel,
                default_tag: "telegram".into(),
                keywords: Vec::new(),
                telegram_chat: Some("@ofertasdofernando".into()),
            })
            .await
            .unwrap();
        assert!(matches!(
            f.channels
                .set_pinterest_board(telegram.id, &ofertas())
                .await,
            Err(ChannelError::NotPinterest)
        ));
        let channel = f.pinterest().await;
        for board in [
            PinterestBoard {
                id: " ".into(),
                ..ofertas()
            },
            PinterestBoard {
                name: "".into(),
                ..ofertas()
            },
        ] {
            assert!(matches!(
                f.channels.set_pinterest_board(channel.id, &board).await,
                Err(ChannelError::BadBoard)
            ));
        }

        f.channels
            .set_pinterest_board(channel.id, &ofertas())
            .await
            .unwrap();
        let other = NewPromotionChannel {
            kind: ChannelKind::Other,
            ..pinterest()
        };
        assert_eq!(
            f.channels
                .update(channel.id, &other)
                .await
                .unwrap()
                .pinterest_board,
            None
        );
        // Back on Pinterest, the board is picked again.
        assert_eq!(
            f.channels
                .update(channel.id, &pinterest())
                .await
                .unwrap()
                .pinterest_board,
            None
        );
    });
}

#[test]
fn a_pin_needs_a_photo_and_a_description_of_up_to_800() {
    let draft = |text: String, image: Option<PostImage>| mascate_marketing::PostDraft {
        channel: RecordId::from_u128(1),
        facts: OfferFacts { image, ..facts() },
        text,
    };
    let limits = post_limits(ChannelKind::Pinterest);
    assert_eq!(limits, Some(PINTEREST_LIMITS));
    let fits = format!("{LINK} #publi {}", "a".repeat(800));
    let fits: String = fits.chars().take(800).collect();
    assert_eq!(check_post(&draft(fits.clone(), facts().image), limits), []);

    let over = format!("{fits}b");
    assert_eq!(
        check_post(&draft(over, facts().image), limits),
        [PostProblem::TooLong {
            units: 801,
            limit: 800,
            // The photo does not shorten a Pin's description.
            caption: false,
        }]
    );
    let without = check_post(&draft(fits, None), limits);
    assert_eq!(without, [PostProblem::NoImage]);
    assert!(without[0].blocks());
}

#[test]
fn a_post_without_a_photo_or_over_the_limit_is_not_saved_on_pinterest() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.on_board().await;
        let photoless = OfferFacts {
            image: None,
            ..facts()
        };
        let draft = f.posts.draft(&photoless, &channel).await.unwrap();
        assert!(matches!(
            f.posts.create(&draft).await,
            Err(PostError::Blocked(problems)) if problems == [PostProblem::NoImage]
        ));

        let mut long = f.posts.draft(&facts(), &channel).await.unwrap();
        long.text = format!("{}\n{}", long.text, "a".repeat(800));
        assert!(matches!(
            f.posts.create(&long).await,
            Err(PostError::Blocked(problems))
                if matches!(problems[..], [PostProblem::TooLong { limit: 800, .. }])
        ));
    });
}

#[test]
fn a_pinterest_post_waits_for_its_board() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.pinterest().await;
        let draft = f.posts.draft(&facts(), &channel).await.unwrap();
        let post = f.posts.create(&draft).await.unwrap();
        assert!(matches!(
            f.posts.prepare(post.id).await,
            Err(PostError::NoBoard)
        ));

        f.channels
            .set_pinterest_board(channel.id, &ofertas())
            .await
            .unwrap();
        let request = f.posts.prepare(post.id).await.unwrap();
        assert_eq!(request.kind(), ChannelKind::Pinterest);
        assert_eq!(request.chat(), "549755885175");
        assert_eq!(request.place(), "Ofertas");
    });
}

#[test]
fn a_confirmed_pin_goes_once_with_the_whole_affiliate_link_and_keeps_its_address() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.on_board().await;
        let mut draft = f.posts.draft(&facts(), &channel).await.unwrap();
        // The text may carry the link any way the owner likes; the Pin's
        // link is the Affiliate Link itself.
        draft.text = draft.text.replace(LINK, &format!("{LINK} (link na bio)"));
        let post = f.posts.create(&draft).await.unwrap();
        let board = Board::answering(pinned("813744226420795884"));

        f.later();
        let request = f.posts.prepare(post.id).await.unwrap();
        let published = f.posts.publish(&board, request.confirm()).await.unwrap();

        assert_eq!(
            board.calls(),
            [Call {
                board: "549755885175".into(),
                title: "Curso de Fotografia com Celular".into(),
                link: LINK.into(),
                text: post.draft.text.clone(),
                image: facts().image,
            }]
        );
        assert_eq!(published.status, PostStatus::Sent);
        assert_eq!(
            published.sent,
            Some(SentPost {
                at: f.clock.now(),
                message_id: Some("813744226420795884".into()),
                link: Some("https://www.pinterest.com/pin/813744226420795884/".into()),
            })
        );
        assert!(matches!(
            f.posts.prepare(post.id).await,
            Err(PostError::WrongStatus(PostStatus::Sent))
        ));
    });
}

#[test]
fn a_long_product_name_is_cut_to_a_title_pinterest_takes() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.on_board().await;
        let product = format!("Kit {} completo", "Fotografia Profissional ".repeat(6));
        let draft = f
            .posts
            .draft(
                &OfferFacts {
                    product: product.clone(),
                    ..facts()
                },
                &channel,
            )
            .await
            .unwrap();
        let post = f.posts.create(&draft).await.unwrap();

        let title = f.posts.prepare(post.id).await.unwrap().title().to_owned();

        assert!(title.chars().count() <= 100, "{title}");
        assert!(title.ends_with('…'), "{title}");
        let kept = title.trim_end_matches('…');
        assert!(product.starts_with(kept), "{title}");
        // Cut between words, never inside one.
        assert!(product[kept.len()..].starts_with(' '), "{title}");
    });
}

#[test]
fn a_pin_that_may_have_gone_out_is_unconfirmed_and_never_sent_again_alone() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.on_board().await;
        let draft = f.posts.draft(&facts(), &channel).await.unwrap();
        let post = f.posts.create(&draft).await.unwrap();
        let board = Board::answering(Err(PublishFailure::Unconfirmed("timeout".into())));

        let request = f.posts.prepare(post.id).await.unwrap();
        let after = f.posts.publish(&board, request.confirm()).await.unwrap();

        assert_eq!(after.status, PostStatus::Unconfirmed);
        assert_eq!(
            after.failure,
            Some(PublishFailure::Unconfirmed("timeout".into()))
        );
        assert_eq!(board.calls().len(), 1);
    });
}
