//! Offer Posts on Instagram (#156, ADR 0035 decisions 5 and 6) through the
//! Marketing module's public interface with a real temporary database and
//! a scripted destination: a post needs a photo already on the internet and
//! a caption Instagram takes, what the destination readied is kept before it
//! publishes, and a post whose answer never came is settled only by what the
//! destination says, when the owner asks.

use std::sync::Mutex;

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{PlatformError, RecordId};
use mascate_marketing::{
    ChannelKind, ConfirmedPost, DestinationCheck, INSTAGRAM_LIMITS, MIGRATIONS,
    NewPromotionChannel, OfferFacts, OfferPost, OfferPosts, PostDestination, PostDraft, PostError,
    PostFate, PostImage, PostProblem, PostStatus, PromotionChannel, PromotionChannels,
    PublishFailure, PublishedPost, check_post, post_limits,
};
use mascate_platform::{Database, migrate};
use std::sync::Arc;

const LINK: &str = "https://go.hotmart.com/X12345678Y?src=insta#oferta";
const CONTAINER: &str = "17889615691921648";

/// Readies and publishes as scripted, and remembers what it was asked.
struct Account {
    staged: Result<Option<String>, PublishFailure>,
    published: Result<PublishedPost, PublishFailure>,
    fate: Mutex<Result<PostFate, PlatformError>>,
    /// What `publish` saw in `staged()`, and the references `fate` got.
    seen: Mutex<Vec<String>>,
    publishes: Mutex<usize>,
}

impl Account {
    fn new(
        staged: Result<Option<String>, PublishFailure>,
        published: Result<PublishedPost, PublishFailure>,
    ) -> Self {
        Self {
            staged,
            published,
            fate: Mutex::new(Ok(PostFate::Unknown)),
            seen: Mutex::new(Vec::new()),
            publishes: Mutex::new(0),
        }
    }

    fn answering_fate(&self, fate: Result<PostFate, PlatformError>) {
        *self.fate.lock().unwrap() = fate;
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }

    fn publishes(&self) -> usize {
        *self.publishes.lock().unwrap()
    }
}

impl PostDestination for Account {
    fn check(&self, _chat: &str) -> Result<DestinationCheck, PlatformError> {
        unreachable!("the Marketing never checks on its own")
    }

    fn stage(&self, _post: &ConfirmedPost) -> Result<Option<String>, PublishFailure> {
        self.staged.clone()
    }

    fn publish(&self, post: &ConfirmedPost) -> Result<PublishedPost, PublishFailure> {
        *self.publishes.lock().unwrap() += 1;
        self.seen
            .lock()
            .unwrap()
            .push(format!("publish {}", post.staged().unwrap_or("-")));
        self.published.clone()
    }

    fn fate(&self, reference: &str) -> Result<PostFate, PlatformError> {
        self.seen.lock().unwrap().push(format!("fate {reference}"));
        self.fate.lock().unwrap().clone()
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
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
            posts: OfferPosts::new(database, clock, ids),
        }
    }

    async fn instagram(&self) -> PromotionChannel {
        self.channels
            .create(&NewPromotionChannel {
                name: "Instagram da loja".into(),
                kind: ChannelKind::Instagram,
                default_tag: "insta".into(),
                keywords: Vec::new(),
                telegram_chat: None,
            })
            .await
            .unwrap()
    }

    async fn post(&self) -> OfferPost {
        let channel = self.instagram().await;
        let draft = self.posts.draft(&facts(), &channel).await.unwrap();
        self.posts.create(&draft).await.unwrap()
    }

    async fn publish(&self, account: &Account, post: &OfferPost) -> OfferPost {
        let request = self.posts.prepare(post.id).await.unwrap();
        self.posts
            .publish(account, request.confirm())
            .await
            .unwrap()
    }
}

fn facts() -> OfferFacts {
    OfferFacts {
        link_id: RecordId::from_u128(900),
        link: LINK.into(),
        product: "Curso de Fotografia com Celular".into(),
        platform: "Hotmart".into(),
        price: None,
        image: Some(PostImage::Url(
            "https://cf.shopee.com.br/file/abc.jpg".into(),
        )),
        price_needs_time: false,
        forbidden_contacts: Vec::new(),
    }
}

fn draft(text: &str, image: Option<PostImage>) -> PostDraft {
    PostDraft {
        channel: RecordId::from_u128(1),
        facts: OfferFacts { image, ..facts() },
        text: text.into(),
    }
}

fn blocking(draft: &PostDraft) -> Vec<PostProblem> {
    check_post(draft, post_limits(ChannelKind::Instagram))
        .into_iter()
        .filter(PostProblem::blocks_publishing)
        .collect()
}

fn media() -> Result<PublishedPost, PublishFailure> {
    Ok(PublishedPost {
        message_id: "17920238422030506".into(),
        link: Some("https://www.instagram.com/p/DAbc123xyZ_/".into()),
    })
}

fn web_photo() -> Option<PostImage> {
    Some(PostImage::Url(
        "https://cf.shopee.com.br/file/abc.jpg".into(),
    ))
}

#[test]
fn instagram_takes_a_photo_post_with_a_caption_of_2200_30_hashtags_and_20_mentions() {
    assert_eq!(post_limits(ChannelKind::Instagram), Some(INSTAGRAM_LIMITS));
    assert_eq!(INSTAGRAM_LIMITS.caption, 2200);
    assert_eq!(INSTAGRAM_LIMITS.hashtags, Some(30));
    assert_eq!(INSTAGRAM_LIMITS.mentions, Some(20));
    let text = |body: String| format!("{body}\n{LINK}\n#publi");

    let fits = text("a".repeat(2200 - LINK.len() - "\n\n#publi".len()));
    assert_eq!(fits.chars().count(), 2200);
    assert!(blocking(&draft(&fits, web_photo())).is_empty());
    let over = format!("a{fits}");
    assert_eq!(
        blocking(&draft(&over, web_photo())),
        [PostProblem::TooLong {
            units: 2201,
            limit: 2200,
            caption: false
        }]
    );

    // #publi is one of the 30.
    let tags = |n: usize| {
        (0..n)
            .map(|i| format!("#oferta{i}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert!(blocking(&draft(&text(tags(29)), web_photo())).is_empty());
    assert_eq!(
        blocking(&draft(&text(tags(30)), web_photo())),
        [PostProblem::TooManyHashtags {
            count: 31,
            limit: 30
        }]
    );

    let mentions = |n: usize| {
        (0..n)
            .map(|i| format!("@loja{i}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    assert!(blocking(&draft(&text(mentions(20)), web_photo())).is_empty());
    assert_eq!(
        blocking(&draft(&text(mentions(21)), web_photo())),
        [PostProblem::TooManyMentions {
            count: 21,
            limit: 20
        }]
    );
}

#[test]
fn a_mark_inside_the_link_a_word_or_an_email_is_no_hashtag_or_mention() {
    // The link carries `#oferta`; `C#`, an e-mail and a lone `#` or `@`
    // are no tags either.
    let tags = (0..29)
        .map(|i| format!("#t{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mentions = (0..20)
        .map(|i| format!("@m{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let text = format!("{tags} {mentions} C# loja@exemplo.com # @ \n{LINK}\n#publi");

    assert!(blocking(&draft(&text, web_photo())).is_empty());
}

#[test]
fn a_folder_photo_or_none_blocks_an_instagram_post() {
    let text = format!("Oferta\n{LINK}\n#publi");

    assert_eq!(
        blocking(&draft(&text, Some(PostImage::File("foto.jpg".into())))),
        [PostProblem::FolderPhoto]
    );
    assert_eq!(blocking(&draft(&text, None)), [PostProblem::NoImage]);
    // On Telegram the same photo goes.
    assert!(
        check_post(
            &draft(&text, Some(PostImage::File("foto.jpg".into()))),
            post_limits(ChannelKind::TelegramChannel)
        )
        .iter()
        .all(|problem| !problem.blocks())
    );
}

#[test]
fn a_post_with_a_folder_photo_is_kept_to_copy_but_never_prepared_for_instagram() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.instagram().await;
        let mut draft = f.posts.draft(&facts(), &channel).await.unwrap();
        draft.facts.image = Some(PostImage::File("foto.jpg".into()));

        let saved = f.posts.create(&draft).await.unwrap();
        assert!(!PostProblem::FolderPhoto.blocks());
        let copied = f.posts.copied(saved.id).await.unwrap();
        assert!(copied.copied_at.is_some());

        let refused = f.posts.prepare(saved.id).await;

        assert!(matches!(
            refused,
            Err(PostError::Blocked(problems)) if problems == [PostProblem::FolderPhoto]
        ));
    });
}

#[test]
fn an_instagram_post_goes_to_the_connected_account_with_the_channels_name() {
    block_on(async {
        let f = Fixture::new().await;
        let post = f.post().await;

        let request = f.posts.prepare(post.id).await.unwrap();

        assert_eq!(request.kind(), ChannelKind::Instagram);
        assert_eq!(request.chat(), "");
        assert_eq!(request.place(), "Instagram da loja");
        assert_eq!(request.text(), post.draft.text);
    });
}

#[test]
fn what_the_destination_readied_is_kept_and_handed_to_its_publish() {
    block_on(async {
        let f = Fixture::new().await;
        let account = Account::new(Ok(Some(CONTAINER.into())), media());
        let post = f.post().await;

        let published = f.publish(&account, &post).await;

        assert_eq!(account.seen(), [format!("publish {CONTAINER}")]);
        assert_eq!(published.status, PostStatus::Sent);
        let sent = published.sent.unwrap();
        assert_eq!(sent.message_id.as_deref(), Some("17920238422030506"));
        assert_eq!(
            sent.link.as_deref(),
            Some("https://www.instagram.com/p/DAbc123xyZ_/")
        );
    });
}

#[test]
fn a_destination_that_could_not_ready_the_post_is_never_asked_to_publish_it() {
    block_on(async {
        let f = Fixture::new().await;
        let account = Account::new(Err(PublishFailure::QuotaUsed), media());
        let post = f.post().await;

        let failed = f.publish(&account, &post).await;

        assert_eq!(failed.status, PostStatus::Failed);
        assert_eq!(failed.failure, Some(PublishFailure::QuotaUsed));
        assert_eq!(account.publishes(), 0);
        // Nothing went out: the owner may try again.
        assert!(failed.status.publishable());
        assert!(f.posts.prepare(post.id).await.is_ok());
    });
}

#[test]
fn an_unconfirmed_post_the_destination_says_it_published_becomes_sent_with_its_link() {
    block_on(async {
        let f = Fixture::new().await;
        let account = Account::new(
            Ok(Some(CONTAINER.into())),
            Err(PublishFailure::Unconfirmed("timeout".into())),
        );
        let post = f.post().await;
        let unconfirmed = f.publish(&account, &post).await;
        assert_eq!(unconfirmed.status, PostStatus::Unconfirmed);
        account.answering_fate(Ok(PostFate::Published(PublishedPost {
            message_id: CONTAINER.into(),
            link: Some("https://www.instagram.com/mascate.ofertas/".into()),
        })));

        let settled = f.posts.check_fate(&account, post.id).await.unwrap();

        assert_eq!(
            account.seen(),
            [format!("publish {CONTAINER}"), format!("fate {CONTAINER}")]
        );
        assert_eq!(settled.status, PostStatus::Sent);
        assert_eq!(settled.failure, None);
        assert_eq!(
            settled.sent.unwrap().link.as_deref(),
            Some("https://www.instagram.com/mascate.ofertas/")
        );
        assert_eq!(account.publishes(), 1);
    });
}

#[test]
fn an_unconfirmed_post_that_never_went_out_waits_for_the_owner_to_send_it_again() {
    block_on(async {
        let f = Fixture::new().await;
        let account = Account::new(Ok(Some(CONTAINER.into())), Err(PublishFailure::Interrupted));
        let post = f.post().await;
        f.publish(&account, &post).await;
        account.answering_fate(Ok(PostFate::NotPublished));

        let settled = f.posts.check_fate(&account, post.id).await.unwrap();

        assert_eq!(settled.status, PostStatus::Failed);
        assert_eq!(settled.failure, Some(PublishFailure::NotPublished));
        assert!(!settled.failure.as_ref().unwrap().may_have_gone_out());
        // Nothing resends on its own.
        assert_eq!(account.publishes(), 1);
        // A post that did not go out is no longer one to check.
        assert!(matches!(
            f.posts.check_fate(&account, post.id).await,
            Err(PostError::WrongStatus(PostStatus::Failed))
        ));
    });
}

#[test]
fn a_destination_that_cannot_say_leaves_the_post_unconfirmed() {
    block_on(async {
        let f = Fixture::new().await;
        let account = Account::new(
            Ok(Some(CONTAINER.into())),
            Err(PublishFailure::Unconfirmed("timeout".into())),
        );
        let post = f.post().await;
        let unconfirmed = f.publish(&account, &post).await;

        let still = f.posts.check_fate(&account, post.id).await.unwrap();
        assert_eq!(still, unconfirmed);

        account.answering_fate(Err(PlatformError::Expired));
        assert!(matches!(
            f.posts.check_fate(&account, post.id).await,
            Err(PostError::FateUnread(PlatformError::Expired))
        ));
        assert_eq!(
            f.posts.post(post.id).await.unwrap().status,
            PostStatus::Unconfirmed
        );
    });
}

#[test]
fn a_post_the_destination_readied_nothing_for_is_not_asked_about() {
    block_on(async {
        let f = Fixture::new().await;
        let account = Account::new(Ok(None), Err(PublishFailure::Unconfirmed("timeout".into())));
        let post = f.post().await;
        let unconfirmed = f.publish(&account, &post).await;

        let still = f.posts.check_fate(&account, post.id).await.unwrap();

        assert_eq!(still, unconfirmed);
        assert_eq!(account.seen(), ["publish -"]);
    });
}

#[test]
fn a_new_attempt_forgets_what_the_last_one_readied() {
    block_on(async {
        let f = Fixture::new().await;
        let readied = Account::new(
            Ok(Some(CONTAINER.into())),
            Err(PublishFailure::Unconfirmed("timeout".into())),
        );
        let post = f.post().await;
        let unconfirmed = f.publish(&readied, &post).await;
        // The owner sends it again, and this time nothing gets readied.
        let nothing = Account::new(Ok(None), Err(PublishFailure::Unconfirmed("timeout".into())));
        f.publish(&nothing, &unconfirmed).await;

        f.posts.check_fate(&readied, post.id).await.unwrap();

        assert_eq!(readied.seen(), [format!("publish {CONTAINER}")]);
    });
}

#[test]
fn only_an_unconfirmed_post_is_checked() {
    block_on(async {
        let f = Fixture::new().await;
        let account = Account::new(Ok(Some(CONTAINER.into())), media());
        let post = f.post().await;

        assert!(matches!(
            f.posts.check_fate(&account, post.id).await,
            Err(PostError::WrongStatus(PostStatus::Draft))
        ));
        f.publish(&account, &post).await;
        assert!(matches!(
            f.posts.check_fate(&account, post.id).await,
            Err(PostError::WrongStatus(PostStatus::Sent))
        ));
        assert_eq!(account.seen(), [format!("publish {CONTAINER}")]);
    });
}

#[test]
fn a_post_left_sending_keeps_what_was_readied_for_the_check() {
    block_on(async {
        let f = Fixture::new().await;
        // The app closed after the container: on the next start the post is
        // unconfirmed, and the container still answers for it.
        let account = Account::new(Ok(Some(CONTAINER.into())), Err(PublishFailure::Interrupted));
        let post = f.post().await;
        f.publish(&account, &post).await;
        f.posts.settle_interrupted().await.unwrap();
        account.answering_fate(Ok(PostFate::NotPublished));

        let settled = f.posts.check_fate(&account, post.id).await.unwrap();

        assert_eq!(settled.status, PostStatus::Failed);
        assert_eq!(
            account.seen().last().map(String::as_str),
            Some(format!("fate {CONTAINER}").as_str())
        );
    });
}
