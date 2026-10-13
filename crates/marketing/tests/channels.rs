//! Promotion Channels through the Marketing module's public interface with
//! a real temporary database: created, edited and deleted, each with a
//! marking no other live channel uses, keeping only what its kind uses.

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_marketing::{
    ChannelError, ChannelKind, MIGRATIONS, NewPromotionChannel, PromotionChannels, suggested_tag,
};
use mascate_platform::{Database, migrate};
use proptest::prelude::*;

struct Fixture {
    _dir: tempfile::TempDir,
    channels: PromotionChannels,
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
        Self {
            _dir: dir,
            channels: PromotionChannels::new(database, clock, Arc::new(SequentialIds::default())),
        }
    }
}

fn telegram() -> NewPromotionChannel {
    NewPromotionChannel {
        name: "Ofertas do Fernando".into(),
        kind: ChannelKind::TelegramChannel,
        default_tag: "telegram".into(),
        keywords: Vec::new(),
        telegram_chat: Some("@ofertasdofernando".into()),
    }
}

fn google_ads() -> NewPromotionChannel {
    NewPromotionChannel {
        name: "Google Ads".into(),
        kind: ChannelKind::PaidSearch,
        default_tag: "google_ads".into(),
        keywords: vec!["curso de fotografia".into(), "fotografia celular".into()],
        telegram_chat: None,
    }
}

#[test]
fn channels_are_created_listed_by_name_edited_and_deleted() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = f.channels.create(&telegram()).await.unwrap();
        let ads = f.channels.create(&google_ads()).await.unwrap();

        assert_eq!(
            f.channels.channels().await.unwrap(),
            vec![ads.clone(), channel.clone()]
        );
        assert_eq!(f.channels.channel(channel.id).await.unwrap(), channel);
        assert_eq!(channel.telegram_chat.as_deref(), Some("@ofertasdofernando"));
        assert_eq!(
            ads.keywords,
            vec!["curso de fotografia", "fotografia celular"]
        );

        let renamed = f
            .channels
            .update(
                channel.id,
                &NewPromotionChannel {
                    name: "  Achados  ".into(),
                    default_tag: "achados".into(),
                    telegram_chat: Some("-1001234567890".into()),
                    ..telegram()
                },
            )
            .await
            .unwrap();
        assert_eq!(renamed.name, "Achados");
        assert_eq!(renamed.default_tag, "achados");
        assert_eq!(renamed.telegram_chat.as_deref(), Some("-1001234567890"));
        assert_eq!(f.channels.channel(channel.id).await.unwrap(), renamed);

        f.channels.delete(channel.id).await.unwrap();
        assert_eq!(f.channels.channels().await.unwrap(), vec![ads]);
        assert!(matches!(
            f.channels.channel(channel.id).await,
            Err(ChannelError::NotFound)
        ));
        assert!(matches!(
            f.channels.delete(channel.id).await,
            Err(ChannelError::NotFound)
        ));
        assert!(matches!(
            f.channels.update(channel.id, &telegram()).await,
            Err(ChannelError::NotFound)
        ));
    });
}

#[test]
fn a_marking_belongs_to_one_live_channel() {
    block_on(async {
        let f = Fixture::new().await;
        let first = f.channels.create(&telegram()).await.unwrap();
        let other = NewPromotionChannel {
            name: "Grupo da família".into(),
            kind: ChannelKind::WhatsAppGroup,
            ..telegram()
        };

        assert!(matches!(
            f.channels.create(&other).await,
            Err(ChannelError::TagTaken(tag)) if tag == "telegram"
        ));
        let second = f
            .channels
            .create(&NewPromotionChannel {
                default_tag: "whatsapp".into(),
                ..other.clone()
            })
            .await
            .unwrap();
        assert!(matches!(
            f.channels.update(second.id, &other).await,
            Err(ChannelError::TagTaken(_))
        ));
        // A channel keeps its own marking when edited.
        f.channels.update(first.id, &telegram()).await.unwrap();
        // A deleted channel frees it.
        f.channels.delete(first.id).await.unwrap();
        f.channels.update(second.id, &other).await.unwrap();
    });
}

#[test]
fn a_channel_keeps_only_what_its_kind_uses() {
    block_on(async {
        let f = Fixture::new().await;
        let instagram = f
            .channels
            .create(&NewPromotionChannel {
                name: "Instagram".into(),
                kind: ChannelKind::Instagram,
                default_tag: "instagram".into(),
                keywords: vec!["fone".into()],
                telegram_chat: Some("@ofertasdofernando".into()),
            })
            .await
            .unwrap();
        assert!(instagram.keywords.is_empty());
        assert_eq!(instagram.telegram_chat, None);

        // A Telegram channel may name its chat later.
        let unnamed = f
            .channels
            .create(&NewPromotionChannel {
                default_tag: "telegram_2".into(),
                telegram_chat: Some("  ".into()),
                ..telegram()
            })
            .await
            .unwrap();
        assert_eq!(unnamed.telegram_chat, None);

        let ads = f
            .channels
            .create(&NewPromotionChannel {
                keywords: vec![
                    "  curso   de fotografia ".into(),
                    "".into(),
                    "Curso de Fotografia".into(),
                    "câmera".into(),
                    "camera".into(),
                ],
                ..google_ads()
            })
            .await
            .unwrap();
        assert_eq!(ads.keywords, vec!["curso de fotografia", "câmera"]);
        assert_eq!(f.channels.channel(ads.id).await.unwrap(), ads);
    });
}

#[test]
fn a_channel_needs_a_name_a_marking_and_a_chat_telegram_takes() {
    block_on(async {
        let f = Fixture::new().await;
        for name in ["", "   ", &"a".repeat(61)] {
            assert!(matches!(
                f.channels
                    .create(&NewPromotionChannel {
                        name: name.into(),
                        ..telegram()
                    })
                    .await,
                Err(ChannelError::BadName)
            ));
        }
        for tag in [
            "",
            "Telegram",
            "tele-gram",
            "tele gram",
            "ofertas_do_fernando_1",
        ] {
            assert!(
                matches!(
                    f.channels
                        .create(&NewPromotionChannel {
                            default_tag: tag.into(),
                            ..telegram()
                        })
                        .await,
                    Err(ChannelError::BadTag)
                ),
                "{tag}"
            );
        }
        for chat in [
            "ofertas",
            "@ofe",
            "@1ofertas",
            "@ofertas-do-fernando",
            "-1234567",
            "-100",
            "-100abc",
            "-10099999999999999999999",
        ] {
            assert!(
                matches!(
                    f.channels
                        .create(&NewPromotionChannel {
                            telegram_chat: Some(chat.into()),
                            ..telegram()
                        })
                        .await,
                    Err(ChannelError::BadChat)
                ),
                "{chat}"
            );
        }
        assert!(f.channels.channels().await.unwrap().is_empty());
        f.channels
            .create(&NewPromotionChannel {
                name: "a".repeat(60),
                default_tag: "ofertas_do_fernando".into(),
                telegram_chat: Some("@ofertas_do_fernando_1".into()),
                ..telegram()
            })
            .await
            .unwrap();
    });
}

#[test]
fn the_suggested_marking_comes_from_the_name() {
    assert_eq!(suggested_tag("Ofertas do Fernando"), "ofertas_do_fernando");
    assert_eq!(
        suggested_tag("Promoções & Achadinhos!"),
        "promocoes_achadinhos"
    );
    assert_eq!(
        suggested_tag("Canal do Telegram — Ofertas"),
        "canal_do_telegram_of"
    );
    assert_eq!(
        suggested_tag("Grupo da Família VIP"),
        "grupo_da_familia_vip"
    );
    assert_eq!(
        suggested_tag("Ofertas do Fernando X"),
        "ofertas_do_fernando"
    );
    assert_eq!(suggested_tag("!!!"), "");
}

proptest! {
    /// Whatever the name, the suggestion is a marking a channel takes, or
    /// nothing when the name has no letter or digit.
    #[test]
    fn a_suggested_marking_is_always_one_a_channel_takes(name in "\\PC{0,80}") {
        let tag = suggested_tag(&name);
        prop_assert!(tag.chars().count() <= 20);
        prop_assert!(tag.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
        prop_assert!(!tag.starts_with('_') && !tag.ends_with('_'));
    }
}
