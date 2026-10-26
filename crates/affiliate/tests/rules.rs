//! Promotion rules per Program and the phase-2 Restricted Features, through
//! the Affiliate module's public interface with a real temporary database:
//! Programs start with their Platform's defaults, the owner's edits survive
//! every Sync, the check warns for every forbidden or unread rule a
//! promotion touches, and the guard refuses what a flag keeps off.

use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    AMAZON_SHORT_LINK, AMAZON_TYPED_PRICE, Affiliate, AffiliateError, AffiliatePlatform,
    BRAND_KEYWORDS, CommissionStatus, FLAGS, MERCADO_LIVRE_SHORT_LINK, MIGRATIONS, Practice,
    Program, Promotion, PromotionRules, Reach, ReportedConversion, ReportedProgram, RuleWarning,
    SHOPEE_PAID_SEARCH, SHOPEE_SHORT_LINK, ShownPrice, Standing,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, Timestamp};
use mascate_platform::{
    Database, Flag, FlagKind, Flags, ModuleMigrations, Phase, Registry, migrate,
};
use proptest::prelude::*;
use rust_decimal::Decimal;

fn noon() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 10, 13, 12, 0, 0).unwrap()
}

fn sale(key: &str, program: &str, status: CommissionStatus) -> ReportedConversion {
    ReportedConversion {
        key: key.into(),
        program: ReportedProgram {
            id: program.into(),
            name: program.into(),
        },
        product: "Fone Bluetooth TWS".into(),
        sold_at: noon() - TimeDelta::days(3),
        commission: Money::new(Decimal::new(420, 2), Currency::Brl),
        status,
        platform_status: "COMPLETED".into(),
        status_changed_at: None,
        tracking: None,
        raw: format!(r#"{{"id":"{key}"}}"#),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    database: Arc<Database>,
    clock: Arc<ManualClock>,
    affiliate: Affiliate,
    /// The owner's flag switches, the way Configurações shows them.
    flags: Flags,
}

impl Fixture {
    async fn new() -> Self {
        Self::migrated_to(&MIGRATIONS).await
    }

    /// A database with the Affiliate's tables as `affiliate` leaves them.
    async fn migrated_to(affiliate: &ModuleMigrations) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(noon()));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(
            &database,
            clock.as_ref(),
            &[mascate_platform::MIGRATIONS, *affiliate],
        )
        .await
        .unwrap();
        let ids = Arc::new(SequentialIds::default());
        let affiliate = Affiliate::new(database.clone(), clock.clone(), ids.clone());
        let flags = Flags::new(
            database.clone(),
            Registry::new(&[FLAGS]).unwrap(),
            clock.clone(),
            ids,
        );
        Self {
            _dir: dir,
            database,
            clock,
            affiliate,
            flags,
        }
    }

    /// One sale on each Platform's Program named after it.
    async fn with_programs(self, platforms: &[AffiliatePlatform]) -> Self {
        for platform in platforms {
            self.affiliate
                .import(
                    *platform,
                    &[sale(
                        platform.name(),
                        platform.name(),
                        CommissionStatus::Pending,
                    )],
                )
                .await
                .unwrap();
        }
        self
    }

    async fn program(&self, platform: AffiliatePlatform) -> Program {
        self.affiliate
            .programs()
            .await
            .unwrap()
            .into_iter()
            .find(|program| program.platform == platform)
            .unwrap()
    }

    async fn turn_on(&self, flag: Flag) {
        self.flags
            .turn_on(
                self.flags
                    .request_turn_on(flag.key)
                    .unwrap()
                    .confirm("fernando"),
            )
            .await
            .unwrap();
        self.clock.advance(TimeDelta::seconds(1));
    }
}

fn paid_search(keywords: &[&str]) -> Promotion {
    Promotion {
        reach: Reach::PaidSearch,
        keywords: keywords.iter().map(|k| (*k).to_owned()).collect(),
        ..Promotion::default()
    }
}

fn typed_price() -> Promotion {
    Promotion {
        text: "Kindle 11ª geração por R$ 499".into(),
        price: Some(ShownPrice {
            typed: true,
            seen_at: Some(noon()),
        }),
        ..Promotion::default()
    }
}

fn keywords(mut rules: PromotionRules, keywords: &[&str]) -> PromotionRules {
    rules.brand_keywords = keywords.iter().map(|k| (*k).to_owned()).collect();
    rules
}

fn practice_warning(practice: Practice, standing: Standing) -> RuleWarning {
    RuleWarning::Practice { practice, standing }
}

#[test]
fn a_program_is_born_with_its_platforms_defaults() {
    block_on(async {
        let fixture = Fixture::new()
            .await
            .with_programs(&[
                AffiliatePlatform::Shopee,
                AffiliatePlatform::Amazon,
                AffiliatePlatform::Hotmart,
                AffiliatePlatform::Kiwify,
            ])
            .await;

        let shopee = fixture.program(AffiliatePlatform::Shopee).await.rules;
        assert_eq!(shopee.standing(Practice::PaidSearch), Standing::Forbidden);
        assert_eq!(shopee.standing(Practice::Email), Standing::Forbidden);
        assert_eq!(shopee.standing(Practice::BoostedPosts), Standing::Allowed);
        assert_eq!(shopee.standing(Practice::Offline), Standing::Unknown);
        assert_eq!(shopee.brand_keywords, ["shopee"]);
        assert!(!shopee.price_needs_time);

        let amazon = fixture.program(AffiliatePlatform::Amazon).await.rules;
        for practice in [
            Practice::Offline,
            Practice::Email,
            Practice::DirectMessage,
            Practice::Shortener,
        ] {
            assert_eq!(
                amazon.standing(practice),
                Standing::Forbidden,
                "{practice:?}"
            );
        }
        assert_eq!(amazon.brand_keywords, ["amazon", "kindle"]);
        assert!(amazon.price_needs_time);

        let hotmart = fixture.program(AffiliatePlatform::Hotmart).await.rules;
        assert_eq!(
            hotmart,
            keywords(PromotionRules::all(Standing::Unknown), &["hotmart"])
        );
        assert_eq!(
            fixture.program(AffiliatePlatform::Kiwify).await.rules,
            PromotionRules::all(Standing::Unknown)
        );
    });
}

#[test]
fn programs_recorded_before_the_rules_existed_get_the_defaults() {
    block_on(async {
        let before = ModuleMigrations {
            module: MIGRATIONS.module,
            migrations: &MIGRATIONS.migrations[..3],
        };
        let fixture = Fixture::migrated_to(&before)
            .await
            .with_programs(&[AffiliatePlatform::Amazon])
            .await;

        migrate(&fixture.database, fixture.clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();

        assert_eq!(
            fixture.program(AffiliatePlatform::Amazon).await.rules,
            PromotionRules::defaults(AffiliatePlatform::Amazon)
        );
    });
}

#[test]
fn the_owners_rules_survive_every_sync() {
    block_on(async {
        let fixture = Fixture::new()
            .await
            .with_programs(&[AffiliatePlatform::Hotmart])
            .await;
        let program = fixture.program(AffiliatePlatform::Hotmart).await;
        let mut edited = keywords(
            PromotionRules::all(Standing::Forbidden)
                .with(Practice::BoostedPosts, Standing::Allowed)
                .with(Practice::Shortener, Standing::Unknown),
            &["hotmart", "Curso de Café"],
        );
        edited.price_needs_time = true;

        fixture
            .affiliate
            .set_promotion_rules(program.id, &edited)
            .await
            .unwrap();
        fixture.clock.advance(TimeDelta::hours(1));
        fixture
            .affiliate
            .import(
                AffiliatePlatform::Hotmart,
                &[
                    sale("Hotmart", "Hotmart", CommissionStatus::Approved),
                    sale("Hotmart 2", "Hotmart", CommissionStatus::Pending),
                ],
            )
            .await
            .unwrap();

        let read = fixture.affiliate.program(program.id).await.unwrap();
        assert_eq!(read.rules, edited);
        assert_eq!(read.name, program.name);
        assert_eq!(fixture.affiliate.programs().await.unwrap().len(), 1);
    });
}

#[test]
fn every_rule_is_kept_on_its_own() {
    block_on(async {
        let fixture = Fixture::new()
            .await
            .with_programs(&[AffiliatePlatform::Kiwify])
            .await;
        let program = fixture.program(AffiliatePlatform::Kiwify).await.id;
        for practice in Practice::ALL {
            for standing in Standing::ALL {
                let rules = PromotionRules::all(Standing::Unknown).with(practice, standing);
                fixture
                    .affiliate
                    .set_promotion_rules(program, &rules)
                    .await
                    .unwrap();
                assert_eq!(
                    fixture.affiliate.program(program).await.unwrap().rules,
                    rules,
                    "{practice:?} {standing:?}"
                );
            }
        }
    });
}

#[test]
fn brand_keywords_are_kept_tidy_and_may_be_emptied() {
    block_on(async {
        let fixture = Fixture::new()
            .await
            .with_programs(&[AffiliatePlatform::Shopee])
            .await;
        let program = fixture.program(AffiliatePlatform::Shopee).await.id;
        let typed = keywords(
            PromotionRules::defaults(AffiliatePlatform::Shopee),
            &["  Shopee  ", "SHÓPEE", "", " - ", "shopee\n  express"],
        );

        fixture
            .affiliate
            .set_promotion_rules(program, &typed)
            .await
            .unwrap();
        assert_eq!(
            fixture
                .affiliate
                .program(program)
                .await
                .unwrap()
                .rules
                .brand_keywords,
            ["Shopee", "shopee express"]
        );

        let emptied = keywords(typed, &[]);
        fixture
            .affiliate
            .set_promotion_rules(program, &emptied)
            .await
            .unwrap();
        assert!(
            fixture
                .affiliate
                .program(program)
                .await
                .unwrap()
                .rules
                .brand_keywords
                .is_empty()
        );
    });
}

#[test]
fn a_program_that_is_not_there_has_no_rules() {
    block_on(async {
        let fixture = Fixture::new().await;
        let missing = mascate_kernel::RecordId::from_u128(7);
        assert!(matches!(
            fixture.affiliate.program(missing).await,
            Err(AffiliateError::NoSuchProgram)
        ));
        assert!(matches!(
            fixture
                .affiliate
                .set_promotion_rules(missing, &PromotionRules::all(Standing::Allowed))
                .await,
            Err(AffiliateError::NoSuchProgram)
        ));
    });
}

#[test]
fn shopee_warns_for_paid_search_and_email_and_not_for_boosting() {
    let shopee = PromotionRules::defaults(AffiliatePlatform::Shopee);
    assert_eq!(
        shopee.check(&paid_search(&["fone bluetooth"])),
        [practice_warning(Practice::PaidSearch, Standing::Forbidden)]
    );
    let email = Promotion {
        reach: Reach::Email,
        ..Promotion::default()
    };
    assert_eq!(
        shopee.check(&email),
        [practice_warning(Practice::Email, Standing::Forbidden)]
    );
    let boosted = Promotion {
        reach: Reach::BoostedPost,
        text: "Achadinho da Shopee".into(),
        ..Promotion::default()
    };
    assert!(shopee.check(&boosted).is_empty());
    assert!(shopee.check(&Promotion::default()).is_empty());
}

#[test]
fn amazon_warns_for_its_forbidden_practices_and_a_price_without_its_time() {
    let amazon = PromotionRules::defaults(AffiliatePlatform::Amazon);
    let shortened_offline = Promotion {
        reach: Reach::Offline,
        shortened: true,
        ..Promotion::default()
    };
    assert_eq!(
        amazon.check(&shortened_offline),
        [
            practice_warning(Practice::Offline, Standing::Forbidden),
            practice_warning(Practice::Shortener, Standing::Forbidden),
        ]
    );
    let message = Promotion {
        reach: Reach::DirectMessage,
        ..Promotion::default()
    };
    assert_eq!(
        amazon.check(&message),
        [practice_warning(
            Practice::DirectMessage,
            Standing::Forbidden
        )]
    );

    let untimed = Promotion {
        price: Some(ShownPrice {
            typed: false,
            seen_at: None,
        }),
        ..Promotion::default()
    };
    assert_eq!(amazon.check(&untimed), [RuleWarning::PriceWithoutTime]);
    let timed = Promotion {
        price: Some(ShownPrice {
            typed: false,
            seen_at: Some(noon()),
        }),
        ..Promotion::default()
    };
    assert!(amazon.check(&timed).is_empty());
    // Boosting is not in the Amazon terms the app has read: unread warns.
    let boosted = Promotion {
        reach: Reach::BoostedPost,
        ..Promotion::default()
    };
    assert_eq!(
        amazon.check(&boosted),
        [practice_warning(Practice::BoostedPosts, Standing::Unknown)]
    );
}

#[test]
fn infoproducts_warn_for_every_practice_until_the_owner_reads_the_producers_rules() {
    for platform in [
        AffiliatePlatform::Hotmart,
        AffiliatePlatform::Kiwify,
        AffiliatePlatform::Eduzz,
        AffiliatePlatform::Monetizze,
    ] {
        let rules = PromotionRules::defaults(platform);
        for (reach, practice) in [
            (Reach::BoostedPost, Practice::BoostedPosts),
            (Reach::PaidSearch, Practice::PaidSearch),
            (Reach::Email, Practice::Email),
            (Reach::DirectMessage, Practice::DirectMessage),
            (Reach::Offline, Practice::Offline),
        ] {
            let promotion = Promotion {
                reach,
                ..Promotion::default()
            };
            assert_eq!(
                rules.check(&promotion),
                [practice_warning(practice, Standing::Unknown)],
                "{platform:?} {reach:?}"
            );
        }
        // A price needs no time there, and the own channel is always fine.
        let priced = Promotion {
            price: Some(ShownPrice {
                typed: true,
                seen_at: None,
            }),
            ..Promotion::default()
        };
        assert!(rules.check(&priced).is_empty(), "{platform:?}");
    }
}

#[test]
fn brand_keywords_match_whole_words_case_and_accents_aside_in_paid_search() {
    let amazon = PromotionRules::defaults(AffiliatePlatform::Amazon);
    let keyword = |keywords: &[&str], text: &str| {
        amazon
            .check(&Promotion {
                text: text.into(),
                ..paid_search(keywords)
            })
            .into_iter()
            .filter(|warning| matches!(warning, RuleWarning::BrandKeyword(_)))
            .collect::<Vec<_>>()
    };

    assert_eq!(
        keyword(&["AMAZÔN prime"], ""),
        [RuleWarning::BrandKeyword("amazon".into())]
    );
    assert_eq!(
        keyword(&["leitor"], "O melhor #Kindle!"),
        [RuleWarning::BrandKeyword("kindle".into())]
    );
    assert_eq!(
        keyword(&["amazon kindle"], ""),
        [
            RuleWarning::BrandKeyword("amazon".into()),
            RuleWarning::BrandKeyword("kindle".into()),
        ]
    );
    assert!(keyword(&["amazonas turismo", "kindleira"], "Manaus, Amazonas").is_empty());

    let two_words = keywords(PromotionRules::all(Standing::Allowed), &["Mercado Livre"]);
    let found = |text: &str| {
        two_words.check(&Promotion {
            text: text.into(),
            ..paid_search(&[])
        })
    };
    assert_eq!(
        found("ofertas do mercado-livre"),
        [RuleWarning::BrandKeyword("Mercado Livre".into())]
    );
    assert!(found("mercado de livres").is_empty());

    // Naming the Platform in a post on the owner's own channel is fine.
    let post = Promotion {
        text: "Kindle com desconto na Amazon".into(),
        ..Promotion::default()
    };
    assert!(amazon.check(&post).is_empty());
}

#[test]
fn the_guard_refuses_what_a_restricted_feature_keeps_off() {
    block_on(async {
        let fixture = Fixture::new()
            .await
            .with_programs(&[
                AffiliatePlatform::Shopee,
                AffiliatePlatform::Amazon,
                AffiliatePlatform::Hotmart,
            ])
            .await;
        let shopee = fixture.program(AffiliatePlatform::Shopee).await.id;
        let amazon = fixture.program(AffiliatePlatform::Amazon).await.id;
        let hotmart = fixture.program(AffiliatePlatform::Hotmart).await.id;
        let refused = |result: Result<Vec<RuleWarning>, AffiliateError>| match result {
            Err(AffiliateError::Restricted(flag)) => flag,
            other => panic!("expected a refusal, got {other:?}"),
        };

        assert_eq!(
            refused(
                fixture
                    .affiliate
                    .ensure_allowed(shopee, &paid_search(&["fone"]))
                    .await
            ),
            SHOPEE_PAID_SEARCH
        );
        assert_eq!(
            refused(
                fixture
                    .affiliate
                    .ensure_allowed(hotmart, &paid_search(&["curso hotmart"]))
                    .await
            ),
            BRAND_KEYWORDS
        );
        assert_eq!(
            refused(
                fixture
                    .affiliate
                    .ensure_allowed(amazon, &typed_price())
                    .await
            ),
            AMAZON_TYPED_PRICE
        );
        let error = fixture
            .affiliate
            .ensure_allowed(amazon, &typed_price())
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), AMAZON_TYPED_PRICE.reason);

        // Rules the owner edits only warn: even an allowed paid search on
        // Shopee stays behind its flag.
        fixture
            .affiliate
            .set_promotion_rules(shopee, &PromotionRules::all(Standing::Allowed))
            .await
            .unwrap();
        assert_eq!(
            refused(
                fixture
                    .affiliate
                    .ensure_allowed(shopee, &paid_search(&["fone"]))
                    .await
            ),
            SHOPEE_PAID_SEARCH
        );
    });
}

#[test]
fn everything_else_only_warns() {
    block_on(async {
        let fixture = Fixture::new()
            .await
            .with_programs(&[AffiliatePlatform::Amazon, AffiliatePlatform::Hotmart])
            .await;
        let amazon = fixture.program(AffiliatePlatform::Amazon).await.id;
        let hotmart = fixture.program(AffiliatePlatform::Hotmart).await.id;

        // A price from Amazon's own link, without its time: a warning.
        let linked_price = Promotion {
            price: Some(ShownPrice {
                typed: false,
                seen_at: None,
            }),
            ..Promotion::default()
        };
        assert_eq!(
            fixture
                .affiliate
                .ensure_allowed(amazon, &linked_price)
                .await
                .unwrap(),
            [RuleWarning::PriceWithoutTime]
        );
        // Paid search without the brand on an infoproduct: a warning.
        assert_eq!(
            fixture
                .affiliate
                .ensure_allowed(hotmart, &paid_search(&["curso de café"]))
                .await
                .unwrap(),
            [practice_warning(Practice::PaidSearch, Standing::Unknown)]
        );
        assert!(
            fixture
                .affiliate
                .ensure_allowed(amazon, &Promotion::default())
                .await
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn a_restricted_feature_turned_on_is_allowed_and_still_warns() {
    block_on(async {
        let fixture = Fixture::new()
            .await
            .with_programs(&[AffiliatePlatform::Shopee, AffiliatePlatform::Amazon])
            .await;
        let shopee = fixture.program(AffiliatePlatform::Shopee).await.id;
        let amazon = fixture.program(AffiliatePlatform::Amazon).await.id;
        fixture.turn_on(SHOPEE_PAID_SEARCH).await;
        fixture.turn_on(AMAZON_TYPED_PRICE).await;

        // Brand keywords are still off.
        assert!(matches!(
            fixture
                .affiliate
                .ensure_allowed(shopee, &paid_search(&["shopee fone"]))
                .await,
            Err(AffiliateError::Restricted(BRAND_KEYWORDS))
        ));
        assert_eq!(
            fixture
                .affiliate
                .ensure_allowed(shopee, &paid_search(&["fone"]))
                .await
                .unwrap(),
            [
                practice_warning(Practice::PaidSearch, Standing::Forbidden),
                RuleWarning::Restricted(SHOPEE_PAID_SEARCH),
            ]
        );
        assert_eq!(
            fixture
                .affiliate
                .ensure_allowed(amazon, &typed_price())
                .await
                .unwrap(),
            [RuleWarning::Restricted(AMAZON_TYPED_PRICE)]
        );

        fixture.turn_on(BRAND_KEYWORDS).await;
        assert_eq!(
            fixture
                .affiliate
                .ensure_allowed(shopee, &paid_search(&["shopee fone"]))
                .await
                .unwrap(),
            [
                practice_warning(Practice::PaidSearch, Standing::Forbidden),
                RuleWarning::BrandKeyword("shopee".into()),
                RuleWarning::Restricted(SHOPEE_PAID_SEARCH),
                RuleWarning::Restricted(BRAND_KEYWORDS),
            ]
        );

        // Turned off again, it refuses again.
        fixture
            .flags
            .turn_off(SHOPEE_PAID_SEARCH.key, "fernando")
            .await
            .unwrap();
        assert!(matches!(
            fixture
                .affiliate
                .ensure_allowed(shopee, &paid_search(&["fone"]))
                .await,
            Err(AffiliateError::Restricted(SHOPEE_PAID_SEARCH))
        ));
    });
}

#[test]
fn the_restricted_features_are_declared_for_their_phases() {
    assert_eq!(
        FLAGS,
        [
            SHOPEE_PAID_SEARCH,
            BRAND_KEYWORDS,
            AMAZON_TYPED_PRICE,
            AMAZON_SHORT_LINK,
            SHOPEE_SHORT_LINK,
            MERCADO_LIVRE_SHORT_LINK,
        ]
    );
    for (at, flag) in FLAGS.iter().enumerate() {
        assert!(flag.key.starts_with("affiliate."), "{}", flag.key);
        assert_eq!(flag.kind, FlagKind::RestrictedFeature, "{}", flag.key);
        let phase = if at < 3 { Phase::Two } else { Phase::Three };
        assert_eq!(flag.phase, phase, "{}", flag.key);
        assert!(!flag.reason.trim().is_empty(), "{}", flag.key);
        assert!(!flag.risk.trim().is_empty(), "{}", flag.key);
    }
    assert!(Registry::new(&[FLAGS]).is_ok());
}

#[test]
fn mercado_livre_forbids_a_shortener_by_default() {
    let mercado_livre = PromotionRules::defaults(AffiliatePlatform::MercadoLivre);

    assert_eq!(
        mercado_livre,
        PromotionRules::all(Standing::Unknown).with(Practice::Shortener, Standing::Forbidden)
    );
}

fn any_reach() -> impl Strategy<Value = Reach> {
    prop_oneof![
        Just(Reach::OwnChannel),
        Just(Reach::BoostedPost),
        Just(Reach::PaidSearch),
        Just(Reach::Email),
        Just(Reach::DirectMessage),
        Just(Reach::Offline),
    ]
}

fn any_standing() -> impl Strategy<Value = Standing> {
    prop_oneof![
        Just(Standing::Allowed),
        Just(Standing::Forbidden),
        Just(Standing::Unknown),
    ]
}

fn any_promotion() -> impl Strategy<Value = Promotion> {
    (
        any_reach(),
        prop::collection::vec("[a-zA-Zçãé ]{0,20}", 0..4),
        "[a-zA-Zçãé#!, ]{0,40}",
        any::<bool>(),
        prop::option::of((any::<bool>(), any::<bool>())),
    )
        .prop_map(|(reach, keywords, text, shortened, price)| Promotion {
            reach,
            keywords,
            text,
            shortened,
            price: price.map(|(typed, timed)| ShownPrice {
                typed,
                seen_at: timed.then(noon),
            }),
        })
}

/// `word` with each letter's case flipped at random and some vowels
/// accented, the way the owner might type it.
fn retyped(word: &'static str) -> impl Strategy<Value = String> {
    prop::collection::vec((any::<bool>(), any::<bool>()), word.len()).prop_map(move |flips| {
        word.chars()
            .zip(flips)
            .map(|(c, (upper, accent))| {
                let c = match (c, accent) {
                    ('a', true) => 'ã',
                    ('e', true) => 'é',
                    ('o', true) => 'ô',
                    (c, _) => c,
                };
                if upper {
                    c.to_uppercase().next().unwrap()
                } else {
                    c
                }
            })
            .collect()
    })
}

proptest! {
    #[test]
    fn an_all_allowed_rule_set_never_warns(promotion in any_promotion()) {
        prop_assert!(PromotionRules::all(Standing::Allowed).check(&promotion).is_empty());
    }

    #[test]
    fn a_practice_not_allowed_always_warns(
        promotion in any_promotion(),
        practice in prop::sample::select(Practice::ALL.to_vec()),
        standing in prop_oneof![Just(Standing::Forbidden), Just(Standing::Unknown)],
        others in any_standing(),
    ) {
        let rules = PromotionRules::all(others).with(practice, standing);
        let touched = promotion.reach.practice() == Some(practice)
            || (practice == Practice::Shortener && promotion.shortened);
        let warned = rules
            .check(&promotion)
            .contains(&practice_warning(practice, standing));
        prop_assert_eq!(warned, touched);
    }

    #[test]
    fn a_brand_keyword_is_found_however_it_is_typed_but_never_inside_a_word(
        typed in retyped("kindle"),
        before in "[ ,.#!-]{1,3}",
        after in "[ ,.#!-]{0,3}",
        glued in "[a-z]{1,4}",
    ) {
        let amazon = PromotionRules::defaults(AffiliatePlatform::Amazon);
        let found = |text: String| {
            amazon
                .check(&Promotion { text, ..paid_search(&[]) })
                .contains(&RuleWarning::BrandKeyword("kindle".into()))
        };
        let alone = format!("leitor{before}{typed}{after}");
        let glued_after = format!("leitor{before}{typed}{glued}");
        let glued_before = format!("{glued}{typed}{after}");
        prop_assert!(found(alone));
        prop_assert!(!found(glued_after));
        prop_assert!(!found(glued_before));
    }
}
