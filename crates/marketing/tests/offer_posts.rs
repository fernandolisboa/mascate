//! Offer Posts through the Marketing module's public interface with a real
//! temporary database: drafted from the caller's facts with the channel's
//! template, checked against the destination and the Program's rules,
//! saved, copied, and the disclosure Reminder raised while a post without
//! it went out in the last 30 days.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, Currency, Money, RecordId};
use mascate_marketing::{
    AD_DISCLOSURE, ChannelKind, Contact, DISCLOSURE, MIGRATIONS, NewPromotionChannel, OfferFacts,
    OfferPosts, OfferPrice, POST_VARIABLES, PRICE_NOTICE, PostDraft, PostError, PostImage,
    PostProblem, PromotionChannel, PromotionChannels, TELEGRAM_LIMITS, check_post,
    default_template, discount, post_limits, price_time,
};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;
use rust_decimal::Decimal;

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
            Utc.with_ymd_and_hms(2026, 10, 13, 12, 0, 0).unwrap(),
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

    async fn channel(&self, kind: ChannelKind, tag: &str) -> PromotionChannel {
        self.channels
            .create(&NewPromotionChannel {
                name: format!("Canal {tag}"),
                kind,
                default_tag: tag.into(),
                keywords: Vec::new(),
                telegram_chat: None,
            })
            .await
            .unwrap()
    }
}

fn brl(cents: i64) -> Money {
    Money::new(Decimal::new(cents, 2), Currency::Brl)
}

const LINK: &str = "https://go.hotmart.com/X12345678Y?src=telegram";

fn facts() -> OfferFacts {
    OfferFacts {
        link_id: RecordId::from_u128(900),
        link: LINK.into(),
        product: "Curso de Fotografia".into(),
        platform: "Hotmart".into(),
        price: Some(OfferPrice {
            price: brl(14990),
            before: Some(brl(19990)),
            seen_at: Utc.with_ymd_and_hms(2026, 10, 13, 11, 30, 0).unwrap(),
        }),
        image: None,
        price_needs_time: false,
        forbidden_contacts: Vec::new(),
    }
}

/// Amazon's facts with the price the owner typed and confirmed.
fn amazon_facts() -> OfferFacts {
    OfferFacts {
        link: "https://www.amazon.com.br/dp/B0ABCDEFGH?tag=mascate-tg-20".into(),
        product: "Kindle 11ª geração".into(),
        platform: "Amazon".into(),
        price_needs_time: true,
        ..facts()
    }
}

#[test]
fn a_telegram_post_fills_every_variable_and_ends_with_the_disclosure() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::TelegramChannel, "telegram").await;
        let draft = f.posts.draft(&facts(), &channel).await.unwrap();
        assert_eq!(
            draft.text,
            format!(
                "🔥 Curso de Fotografia\n\nDe R$ 199,90\nPor R$ 149,90\n📉 25% de desconto\n\n\
                 👉 {LINK}\n\n{DISCLOSURE}"
            )
        );
        assert_eq!(draft.channel, channel.id);
        assert_eq!(draft.facts, facts());
        assert!(f.posts.check(&draft).await.unwrap().is_empty());
    });
}

#[test]
fn a_missing_value_takes_its_line_out_and_leaves_no_dangling_price() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::WhatsAppGroup, "zap").await;
        let no_price = OfferFacts {
            price: None,
            ..facts()
        };
        assert_eq!(
            f.posts.draft(&no_price, &channel).await.unwrap().text,
            format!("🔥 Curso de Fotografia\n\n👉 {LINK}\n\n{DISCLOSURE}")
        );
        // A "before" price no higher than the price is no discount at all.
        for before in [None, Some(brl(14990)), Some(brl(9990))] {
            let facts = OfferFacts {
                price: Some(OfferPrice {
                    before,
                    ..facts().price.unwrap()
                }),
                ..facts()
            };
            assert_eq!(
                f.posts.draft(&facts, &channel).await.unwrap().text,
                format!("🔥 Curso de Fotografia\n\nPor R$ 149,90\n\n👉 {LINK}\n\n{DISCLOSURE}")
            );
        }
    });
}

#[test]
fn an_amazon_price_carries_its_time_in_brasilia_and_the_notice() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::TelegramChannel, "telegram").await;
        let draft = f.posts.draft(&amazon_facts(), &channel).await.unwrap();
        assert_eq!(
            draft.text,
            "🔥 Kindle 11ª geração\n\nDe R$ 199,90\nPor R$ 149,90\nPreço em 13/10/2026 às 08:30, \
             horário de Brasília\n📉 25% de desconto\n\n\
             👉 https://www.amazon.com.br/dp/B0ABCDEFGH?tag=mascate-tg-20\n\n\
             Os preços e a disponibilidade dos produtos estão corretos na data/horário indicados \
             e poderão sofrer alterações.\n\n#publi · link de afiliado"
        );
        assert!(f.posts.check(&draft).await.unwrap().is_empty());

        // A template without the time still gets it, with the notice.
        f.posts
            .set_template(channel.id, "{produto} por {preco}\n{link}")
            .await
            .unwrap();
        let draft = f.posts.draft(&amazon_facts(), &channel).await.unwrap();
        assert!(draft.text.contains(&format!(
            "Preço {}. {PRICE_NOTICE}.",
            price_time(amazon_facts().price.unwrap().seen_at)
        )));
        assert!(f.posts.check(&draft).await.unwrap().is_empty());
    });
}

#[test]
fn an_amazon_post_without_a_price_needs_no_time() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::TelegramChannel, "telegram").await;
        let facts = OfferFacts {
            price: None,
            ..amazon_facts()
        };
        let draft = f.posts.draft(&facts, &channel).await.unwrap();
        assert_eq!(
            draft.text,
            "🔥 Kindle 11ª geração\n\n\
             👉 https://www.amazon.com.br/dp/B0ABCDEFGH?tag=mascate-tg-20\n\n\
             #publi · link de afiliado"
        );
        assert!(f.posts.check(&draft).await.unwrap().is_empty());
    });
}

#[test]
fn editing_out_the_time_the_notice_or_the_disclosure_warns_and_the_link_blocks() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::TelegramChannel, "telegram").await;
        let draft = f.posts.draft(&amazon_facts(), &channel).await.unwrap();
        let time = price_time(amazon_facts().price.unwrap().seen_at);
        let edited = |from: &str, to: &str| {
            let mut edited = draft.clone();
            edited.text = edited.text.replace(from, to);
            edited
        };
        let cases = [
            (edited(&time, ""), vec![PostProblem::NoPriceTime]),
            (edited(PRICE_NOTICE, ""), vec![PostProblem::NoPriceNotice]),
            (
                edited(DISCLOSURE, "link de afiliado"),
                vec![PostProblem::NoDisclosure],
            ),
            (
                edited(
                    "https://www.amazon.com.br/dp/B0ABCDEFGH?tag=mascate-tg-20",
                    "",
                ),
                vec![PostProblem::NoLink],
            ),
            // Taking the price out takes its rules with it.
            (edited("Por R$ 149,90", "Em oferta"), Vec::new()),
            (edited(DISCLOSURE, "#publicidade"), Vec::new()),
        ];
        for (draft, expected) in cases {
            assert_eq!(
                f.posts.check(&draft).await.unwrap(),
                expected,
                "{}",
                draft.text
            );
            let saved = f.posts.create(&draft).await;
            if expected.iter().any(PostProblem::blocks) {
                assert!(
                    matches!(saved, Err(PostError::Blocked(ref problems)) if *problems == expected)
                );
            } else {
                assert_eq!(saved.unwrap().draft.text, draft.text);
            }
        }
    });
}

#[test]
fn telegram_counts_utf16_units_and_a_photo_caption_takes_less() {
    block_on(async {
        let f = Fixture::new().await;
        let telegram = f.channel(ChannelKind::TelegramChannel, "telegram").await;
        let whatsapp = f.channel(ChannelKind::WhatsAppGroup, "zap").await;
        let mut draft = f.posts.draft(&facts(), &telegram).await.unwrap();
        // Each 🔥 is two UTF-16 units though one character.
        let pad = |text: &str, units: usize| {
            let gap = units - text.encode_utf16().count();
            format!("{text}{}{}", "a".repeat(gap % 2), "🔥".repeat(gap / 2))
        };
        draft.text = pad(&draft.text, TELEGRAM_LIMITS.text);
        assert_eq!(draft.text.encode_utf16().count(), TELEGRAM_LIMITS.text);
        assert!(f.posts.check(&draft).await.unwrap().is_empty());

        draft.text.push('🔥');
        let too_long = PostProblem::TooLong {
            units: TELEGRAM_LIMITS.text + 2,
            limit: TELEGRAM_LIMITS.text,
            caption: false,
        };
        assert_eq!(f.posts.check(&draft).await.unwrap(), vec![too_long.clone()]);
        assert!(matches!(
            f.posts.create(&draft).await,
            Err(PostError::Blocked(problems)) if problems == vec![too_long]
        ));
        // The same text saves for a channel without a known limit.
        let elsewhere = PostDraft {
            channel: whatsapp.id,
            ..draft.clone()
        };
        assert!(f.posts.create(&elsewhere).await.is_ok());

        let mut photo = f.posts.draft(&facts(), &telegram).await.unwrap();
        photo.facts.image = Some(PostImage::Url("https://cf.shopee.com.br/file/abc".into()));
        photo.text = pad(&photo.text, TELEGRAM_LIMITS.caption + 2);
        assert_eq!(
            check_post(&photo, post_limits(ChannelKind::TelegramChannel)),
            vec![PostProblem::TooLong {
                units: photo.text.encode_utf16().count(),
                limit: TELEGRAM_LIMITS.caption,
                caption: true,
            }]
        );
    });
}

#[test]
fn a_contact_the_program_forbids_warns_but_the_link_is_no_contact() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::TelegramChannel, "telegram").await;
        let forbidding = OfferFacts {
            forbidden_contacts: vec![Contact::Phone, Contact::Email],
            ..facts()
        };
        let mut draft = f.posts.draft(&forbidding, &channel).await.unwrap();
        // The link's 8 digits read as no phone number.
        assert!(f.posts.check(&draft).await.unwrap().is_empty());
        draft.text = format!("{}\nDúvidas: (11) 98765-4321 ou eu@exemplo.com", draft.text);
        assert_eq!(
            f.posts.check(&draft).await.unwrap(),
            vec![
                PostProblem::Contact(Contact::Phone),
                PostProblem::Contact(Contact::Email)
            ]
        );
        // A Program that allows them says nothing.
        draft.facts.forbidden_contacts.clear();
        assert!(f.posts.check(&draft).await.unwrap().is_empty());
    });
}

#[test]
fn a_channel_keeps_its_own_template_until_put_back_on_the_default() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::Instagram, "insta").await;
        assert_eq!(
            f.posts.template(&channel).await.unwrap(),
            default_template(ChannelKind::Instagram)
        );
        assert!(matches!(
            f.posts.set_template(channel.id, "{produto} {preço}\n{link}").await,
            Err(PostError::UnknownVariable(variable)) if variable == "{preço}"
        ));
        assert!(matches!(
            f.posts
                .set_template(channel.id, "{produto} por {preco}")
                .await,
            Err(PostError::TemplateWithoutLink)
        ));
        assert!(matches!(
            f.posts
                .set_template(RecordId::from_u128(404), "{link}")
                .await,
            Err(PostError::NoSuchChannel)
        ));
        f.posts
            .set_template(channel.id, "  {produto}: {preco}\n{link}\n")
            .await
            .unwrap();
        f.posts
            .set_template(channel.id, "{produto} só {preco}\n{link}")
            .await
            .unwrap();
        assert_eq!(
            f.posts.draft(&facts(), &channel).await.unwrap().text,
            format!("Curso de Fotografia só R$ 149,90\n{LINK}\n\n{DISCLOSURE}")
        );
        f.posts.reset_template(channel.id).await.unwrap();
        assert_eq!(
            f.posts.template(&channel).await.unwrap(),
            default_template(ChannelKind::Instagram)
        );
    });
}

#[test]
fn a_post_saves_edits_and_deletes_with_every_fact() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::TelegramChannel, "telegram").await;
        let photo = OfferFacts {
            image: Some(PostImage::File(PathBuf::from("/fotos/KINDLE-001/capa.jpg"))),
            forbidden_contacts: vec![Contact::Email, Contact::Phone],
            ..amazon_facts()
        };
        let mut draft = f.posts.draft(&photo, &channel).await.unwrap();
        draft.text = format!("  {}\n", draft.text);
        let saved = f.posts.create(&draft).await.unwrap();
        assert_eq!(saved.draft.facts, photo);
        assert_eq!(saved.draft.text, draft.text.trim());
        assert_eq!(saved.copied_at, None);
        assert_eq!(f.posts.post(saved.id).await.unwrap(), saved);

        f.clock.advance(TimeDelta::minutes(5));
        let first = f
            .posts
            .create(&f.posts.draft(&facts(), &channel).await.unwrap())
            .await
            .unwrap();
        f.clock.advance(TimeDelta::minutes(5));
        let mut edited = saved.draft.clone();
        edited.facts.price = None;
        edited.facts.image = Some(PostImage::Url("https://m.media-amazon.com/x.jpg".into()));
        edited.text = format!("Kindle\n{}\n{DISCLOSURE}", edited.facts.link);
        let updated = f.posts.update(saved.id, &edited).await.unwrap();
        assert_eq!(updated.draft, edited);
        assert_eq!(
            f.posts
                .posts()
                .await
                .unwrap()
                .iter()
                .map(|post| post.id)
                .collect::<Vec<_>>(),
            vec![saved.id, first.id]
        );

        f.posts.delete(saved.id).await.unwrap();
        assert!(matches!(
            f.posts.post(saved.id).await,
            Err(PostError::NotFound)
        ));
        assert!(matches!(
            f.posts.update(saved.id, &edited).await,
            Err(PostError::NotFound)
        ));
        assert_eq!(f.posts.posts().await.unwrap(), vec![first]);
    });
}

#[test]
fn the_disclosure_reminder_shows_only_while_an_undisclosed_post_went_out_in_30_days() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channel(ChannelKind::WhatsAppGroup, "zap").await;
        let disclosed = f
            .posts
            .create(&f.posts.draft(&facts(), &channel).await.unwrap())
            .await
            .unwrap();
        let copied = f.posts.copied(disclosed.id).await.unwrap();
        assert_eq!(copied.copied_at, Some(f.clock.now()));
        assert!(f.posts.raised_reminders().await.unwrap().is_empty());

        // Saved without the disclosure but never copied: nothing went out.
        let mut bare = f.posts.draft(&facts(), &channel).await.unwrap();
        bare.text = bare.text.replace(DISCLOSURE, "");
        let bare = f.posts.create(&bare).await.unwrap();
        assert!(f.posts.raised_reminders().await.unwrap().is_empty());

        f.posts.copied(bare.id).await.unwrap();
        assert_eq!(
            f.posts.raised_reminders().await.unwrap(),
            vec![AD_DISCLOSURE.key]
        );
        // Putting the disclosure back, or deleting the post, does not call
        // back what went out.
        f.clock.advance(TimeDelta::days(1));
        let mut fixed = bare.draft.clone();
        fixed.text = format!("{}\n\n{DISCLOSURE}", fixed.text);
        f.posts.update(bare.id, &fixed).await.unwrap();
        f.posts.copied(bare.id).await.unwrap();
        f.posts.delete(bare.id).await.unwrap();
        assert!(matches!(
            f.posts.copied(bare.id).await,
            Err(PostError::NotFound)
        ));
        f.clock.advance(TimeDelta::days(29));
        assert_eq!(
            f.posts.raised_reminders().await.unwrap(),
            vec![AD_DISCLOSURE.key]
        );
        f.clock.advance(TimeDelta::seconds(1));
        assert!(f.posts.raised_reminders().await.unwrap().is_empty());
    });
}

#[test]
fn a_discount_rounds_only_to_show() {
    // 50,00 off 199,90 is 25.0125…%: kept whole, shown as 25%.
    let cut = discount(brl(14990), brl(19990)).unwrap();
    assert_eq!(
        cut.percent(),
        Decimal::from(5000) * Decimal::ONE_HUNDRED / Decimal::from(19990)
    );
    assert_eq!(discount(brl(0), brl(19990)).unwrap().percent(), 100.into());
    assert_eq!(discount(brl(19990), brl(19990)), None);
    assert_eq!(
        discount(brl(100), Money::new(2.into(), Currency::Usd)),
        None
    );
}

fn money() -> impl Strategy<Value = Money> {
    (
        -10_000_000i64..10_000_000,
        prop_oneof![Just(Currency::Brl), Just(Currency::Usd)],
    )
        .prop_map(|(cents, currency)| Money::new(Decimal::new(cents, 2), currency))
}

/// A template made of any of the variables and a few words, on any lines.
fn template() -> impl Strategy<Value = String> {
    let piece = prop_oneof![
        prop::sample::select(POST_VARIABLES.to_vec()).prop_map(str::to_owned),
        Just("De {de}".to_owned()),
        Just("por ".to_owned()),
        Just("\n".to_owned()),
        Just("\n\n".to_owned()),
    ];
    prop::collection::vec(piece, 0..16).prop_map(|pieces| format!("{}\n{{link}}", pieces.concat()))
}

proptest! {
    #[test]
    fn a_discount_is_never_negative_nor_above_100(price in money(), before in money()) {
        if let Some(cut) = discount(price, before) {
            prop_assert!(cut.percent() > Decimal::ZERO);
            prop_assert!(cut.percent() <= Decimal::ONE_HUNDRED);
            prop_assert_eq!(price.currency(), before.currency());
        }
    }

    #[test]
    fn no_placeholder_nor_dangling_before_price_is_left(
        template in template(),
        price in prop::option::of(0i64..100_000),
        before in prop::option::of(0i64..100_000),
        needs_time in any::<bool>(),
    ) {
        block_on(async {
            let f = Fixture::new().await;
            let channel = f.channel(ChannelKind::Other, "outro").await;
            f.posts.set_template(channel.id, &template).await.unwrap();
            let facts = OfferFacts {
                price: price.map(|price| OfferPrice {
                    price: brl(price),
                    before: before.map(brl),
                    seen_at: f.clock.now(),
                }),
                price_needs_time: needs_time,
                ..facts()
            };
            let draft = f.posts.draft(&facts, &channel).await.unwrap();
            for variable in POST_VARIABLES {
                prop_assert!(!draft.text.contains(variable), "{}", draft.text);
            }
            prop_assert!(!draft.text.contains("\n\n\n"), "{}", draft.text);
            let discounted = matches!((price, before), (Some(price), Some(before)) if before > price);
            if !discounted {
                prop_assert!(!draft.text.contains("De R$"), "{}", draft.text);
            }
            let problems = f.posts.check(&draft).await.unwrap();
            prop_assert!(problems.is_empty(), "{:?}\n{}", problems, draft.text);
            Ok(())
        })?;
    }
}
