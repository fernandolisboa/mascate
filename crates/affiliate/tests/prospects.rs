//! Prospects through the Affiliate module's public interface with a real
//! temporary database: kept by the Platform's natural key with each
//! signal's origin and time, ranked by their score, dismissed for good,
//! and turned into exactly one provisional Program.

use std::sync::Arc;

use chrono::{NaiveDate, TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliateError, AffiliatePlatform, CommissionStatus, FoundProspect, MIGRATIONS,
    NewProgram, ProspectCategory, ProspectSource, ReportedConversion, ReportedProgram, SignalKind,
    SignalOrigin, Signals, prospect_key_of,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, Percentage, PlatformError, Timestamp};
use mascate_platform::{Database, migrate};
use rust_decimal::Decimal;

fn noon() -> Timestamp {
    Utc.with_ymd_and_hms(2026, 11, 7, 15, 0, 0).unwrap()
}

fn brl(amount: &str) -> Money {
    Money::new(amount.parse::<Decimal>().unwrap(), Currency::Brl)
}

fn percent(value: &str) -> Percentage {
    Percentage::new(value.parse().unwrap()).unwrap()
}

const EXCEL_PAGE: &str = "https://hotmart.com/pt-br/marketplace/produtos/excel-do-zero/X123";

fn excel() -> FoundProspect {
    FoundProspect {
        platform: AffiliatePlatform::Hotmart,
        key: "4021".into(),
        name: "Excel do Zero ao Avançado".into(),
        signals: Signals {
            rating: Some("4.8".parse().unwrap()),
            reviews: Some(963),
            students: Some(12_000),
            launched: Some(NaiveDate::from_ymd_opt(2026, 9, 28).unwrap()),
            product_page: Some(EXCEL_PAGE.into()),
            ..Signals::default()
        },
    }
}

fn photo() -> FoundProspect {
    FoundProspect {
        platform: AffiliatePlatform::Hotmart,
        key: "5150".into(),
        name: "Fotografia com Celular".into(),
        signals: Signals {
            price: Some(brl("47.00")),
            commission_rate: Some(percent("50")),
            ..Signals::default()
        },
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    affiliate: Affiliate,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(noon()));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        let affiliate = Affiliate::new(database, clock.clone(), Arc::new(SequentialIds::default()));
        Self {
            _dir: dir,
            clock,
            affiliate,
        }
    }
}

#[test]
fn saving_the_same_prospect_again_changes_nothing() {
    block_on(async {
        let fixture = Fixture::new().await;
        let id = fixture
            .affiliate
            .save_prospect(&excel(), SignalOrigin::Vitrine)
            .await
            .unwrap();
        let first = fixture.affiliate.prospects(None).await.unwrap();
        fixture.clock.advance(TimeDelta::hours(2));
        let again = fixture
            .affiliate
            .save_prospect(&excel(), SignalOrigin::Vitrine)
            .await
            .unwrap();
        assert_eq!(again, id);
        assert_eq!(fixture.affiliate.prospects(None).await.unwrap(), first);
        let kept = &first[0];
        assert_eq!(kept.key, "4021");
        assert_eq!(kept.signals, excel().signals);
        let rating = kept.reading(SignalKind::Rating).unwrap();
        assert_eq!(rating.origin, SignalOrigin::Vitrine);
        assert_eq!(rating.read_at, noon());
    });
}

#[test]
fn a_partial_save_replaces_only_its_signals_with_their_origin_and_time() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .affiliate
            .save_prospect(&excel(), SignalOrigin::Vitrine)
            .await
            .unwrap();
        fixture.clock.advance(TimeDelta::hours(1));
        let typed = FoundProspect {
            name: "Excel do Zero ao Avançado 2.0".into(),
            signals: Signals {
                commission: Some(brl("97.00")),
                temperature: Some(35),
                rating: Some("4.9".parse().unwrap()),
                ..Signals::default()
            },
            ..excel()
        };
        fixture
            .affiliate
            .save_prospect(&typed, SignalOrigin::Typed)
            .await
            .unwrap();
        let kept = fixture.affiliate.prospects(None).await.unwrap().remove(0);
        assert_eq!(kept.name, "Excel do Zero ao Avançado 2.0");
        assert_eq!(kept.signals.commission, Some(brl("97.00")));
        assert_eq!(kept.signals.temperature, Some(35));
        assert_eq!(kept.signals.rating, Some("4.9".parse().unwrap()));
        assert_eq!(kept.signals.students, Some(12_000));
        assert_eq!(kept.signals.product_page.as_deref(), Some(EXCEL_PAGE));
        let rating = kept.reading(SignalKind::Rating).unwrap();
        assert_eq!(
            (rating.origin, rating.read_at),
            (SignalOrigin::Typed, noon() + TimeDelta::hours(1))
        );
        let students = kept.reading(SignalKind::Students).unwrap();
        assert_eq!(
            (students.origin, students.read_at),
            (SignalOrigin::Vitrine, noon())
        );
    });
}

#[test]
fn prospects_rank_by_score_and_by_what_one_sale_earns() {
    block_on(async {
        let fixture = Fixture::new().await;
        let dear = FoundProspect {
            key: "7000".into(),
            name: "Mentoria de Vendas".into(),
            signals: Signals {
                price: Some(brl("497.00")),
                commission_rate: Some(percent("30")),
                ..Signals::default()
            },
            ..photo()
        };
        for found in [photo(), dear] {
            fixture
                .affiliate
                .save_prospect(&found, SignalOrigin::Typed)
                .await
                .unwrap();
        }
        let names: Vec<String> = fixture
            .affiliate
            .prospects(None)
            .await
            .unwrap()
            .into_iter()
            .map(|prospect| prospect.name)
            .collect();
        assert_eq!(names, ["Mentoria de Vendas", "Fotografia com Celular"]);
    });
}

#[test]
fn prospects_filter_by_platform() {
    block_on(async {
        let fixture = Fixture::new().await;
        let kiwify = FoundProspect {
            platform: AffiliatePlatform::Kiwify,
            ..photo()
        };
        for found in [photo(), kiwify] {
            fixture
                .affiliate
                .save_prospect(&found, SignalOrigin::Typed)
                .await
                .unwrap();
        }
        let on_kiwify = fixture
            .affiliate
            .prospects(Some(AffiliatePlatform::Kiwify))
            .await
            .unwrap();
        assert_eq!(on_kiwify.len(), 1);
        assert_eq!(on_kiwify[0].platform, AffiliatePlatform::Kiwify);
        assert_eq!(fixture.affiliate.prospects(None).await.unwrap().len(), 2);
    });
}

#[test]
fn a_dismissed_prospect_stays_dismissed_when_saved_or_read_again() {
    block_on(async {
        let fixture = Fixture::new().await;
        let id = fixture
            .affiliate
            .save_prospect(&excel(), SignalOrigin::Vitrine)
            .await
            .unwrap();
        assert!(matches!(
            fixture.affiliate.dismiss_prospect(id, "  ").await,
            Err(AffiliateError::NoReason)
        ));
        fixture
            .affiliate
            .dismiss_prospect(id, "nicho saturado")
            .await
            .unwrap();
        fixture
            .affiliate
            .save_prospect(&excel(), SignalOrigin::Typed)
            .await
            .unwrap();
        let read = fixture
            .affiliate
            .read_prospects(&Vitrine(vec![excel(), photo()]), None)
            .await
            .unwrap();
        assert_eq!((read.found, read.added, read.dismissed), (2, 1, 1));
        let shown = fixture.affiliate.prospects(None).await.unwrap();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].name, "Fotografia com Celular");
        assert_eq!(
            fixture
                .affiliate
                .dismissed_prospects(Some(AffiliatePlatform::Hotmart))
                .await
                .unwrap(),
            1
        );
        let dismissed = fixture.affiliate.prospect(id).await.unwrap();
        assert_eq!(dismissed.dismissed.as_deref(), Some("nicho saturado"));
    });
}

#[test]
fn promoting_creates_exactly_one_provisional_program() {
    block_on(async {
        let fixture = Fixture::new().await;
        let id = fixture
            .affiliate
            .save_prospect(&excel(), SignalOrigin::Vitrine)
            .await
            .unwrap();
        let program = fixture.affiliate.promote_prospect(id).await.unwrap();
        assert!(program.provisional);
        assert_eq!(program.platform, AffiliatePlatform::Hotmart);
        assert_eq!(
            program.product.as_deref(),
            Some("Excel do Zero ao Avançado")
        );
        let again = fixture.affiliate.promote_prospect(id).await.unwrap();
        assert_eq!(again.id, program.id);
        assert_eq!(fixture.affiliate.programs().await.unwrap().len(), 1);
        let chosen = fixture.affiliate.prospect(id).await.unwrap();
        assert!(chosen.chosen);
        assert_eq!(chosen.program, Some(program.id));
    });
}

#[test]
fn a_product_already_promoted_shows_its_program() {
    block_on(async {
        let fixture = Fixture::new().await;
        let added = fixture
            .affiliate
            .add_program(&NewProgram {
                platform: AffiliatePlatform::Hotmart,
                name: String::new(),
                product: "fotografia com  CELULAR".into(),
            })
            .await
            .unwrap();
        fixture.affiliate.sync(&SoldOnce).await.unwrap();
        for found in [photo(), excel()] {
            fixture
                .affiliate
                .save_prospect(&found, SignalOrigin::Vitrine)
                .await
                .unwrap();
        }
        let shown = fixture.affiliate.prospects(None).await.unwrap();
        let program_of = |name: &str| {
            shown
                .iter()
                .find(|prospect| prospect.name == name)
                .unwrap()
                .program
        };
        assert_eq!(program_of("Fotografia com Celular"), Some(added.id));
        let sold = program_of("Excel do Zero ao Avançado").unwrap();
        assert!(shown.iter().all(|prospect| !prospect.chosen));
        let excel = shown
            .iter()
            .find(|prospect| prospect.program == Some(sold))
            .unwrap();
        let promoted = fixture.affiliate.promote_prospect(excel.id).await.unwrap();
        assert_eq!(promoted.id, sold);
        assert!(!promoted.provisional);
        assert_eq!(fixture.affiliate.programs().await.unwrap().len(), 2);
        assert!(fixture.affiliate.prospect(excel.id).await.unwrap().chosen);
    });
}

/// A Hotmart sale of Excel do Zero under another name, whose Program a
/// Sync keys by Hotmart's product id, the Prospect's key.
struct SoldOnce;

impl mascate_affiliate::CommissionSource for SoldOnce {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Hotmart
    }

    fn history(&self) -> TimeDelta {
        TimeDelta::days(30)
    }

    fn settle_time(&self) -> TimeDelta {
        TimeDelta::days(30)
    }

    fn conversions(
        &self,
        _: std::ops::Range<Timestamp>,
    ) -> Result<Vec<ReportedConversion>, PlatformError> {
        Ok(vec![ReportedConversion {
            key: "HP1".into(),
            program: ReportedProgram {
                id: "4021".into(),
                name: "Excel Avançado (turma 3)".into(),
            },
            product: "Excel Avançado (turma 3)".into(),
            sold_at: noon() - TimeDelta::days(1),
            commission: brl("97.00"),
            status: CommissionStatus::Approved,
            platform_status: "APPROVED".into(),
            status_changed_at: None,
            tracking: None,
            raw: "{}".into(),
        }])
    }
}

#[test]
fn impossible_signals_and_a_missing_key_or_name_are_refused() {
    block_on(async {
        let fixture = Fixture::new().await;
        let hot = FoundProspect {
            signals: Signals {
                temperature: Some(151),
                ..Signals::default()
            },
            ..photo()
        };
        assert!(matches!(
            fixture
                .affiliate
                .save_prospect(&hot, SignalOrigin::Typed)
                .await,
            Err(AffiliateError::ImpossibleSignal(SignalKind::Temperature))
        ));
        let not_a_page = FoundProspect {
            signals: Signals {
                product_page: Some("javascript:alert(1)".into()),
                ..Signals::default()
            },
            ..photo()
        };
        assert!(matches!(
            fixture
                .affiliate
                .save_prospect(&not_a_page, SignalOrigin::Typed)
                .await,
            Err(AffiliateError::ImpossibleSignal(SignalKind::ProductPage))
        ));
        let keyless = FoundProspect {
            key: " ".into(),
            ..photo()
        };
        assert!(matches!(
            fixture
                .affiliate
                .save_prospect(&keyless, SignalOrigin::Typed)
                .await,
            Err(AffiliateError::NoProspectKey)
        ));
        let nameless = FoundProspect {
            name: String::new(),
            ..photo()
        };
        assert!(matches!(
            fixture
                .affiliate
                .save_prospect(&nameless, SignalOrigin::Typed)
                .await,
            Err(AffiliateError::NoProduct)
        ));
        assert!(fixture.affiliate.prospects(None).await.unwrap().is_empty());
    });
}

#[test]
fn a_pasted_link_keys_the_product_without_its_query() {
    assert_eq!(
        prospect_key_of(" HTTPS://Hotmart.com/pt-br/marketplace/produtos/excel/X123/?ref=abc#top "),
        Some("https://hotmart.com/pt-br/marketplace/produtos/excel/X123".into())
    );
    assert_eq!(
        prospect_key_of("http://go.hotmart.com/X123?ap=1"),
        Some("https://go.hotmart.com/X123".into())
    );
    assert_eq!(prospect_key_of("hotmart.com/produto"), None);
    assert_eq!(prospect_key_of("ftp://hotmart.com/x"), None);
    assert_eq!(prospect_key_of("https:///x"), None);
}

#[test]
fn a_product_typed_by_its_link_and_then_read_is_one_prospect_under_the_platform_id() {
    block_on(async {
        let fixture = Fixture::new().await;
        let typed = FoundProspect {
            platform: AffiliatePlatform::Hotmart,
            key: prospect_key_of(&format!("{EXCEL_PAGE}?ref=insta")).unwrap(),
            name: "Excel".into(),
            signals: Signals {
                commission_rate: Some(percent("40")),
                product_page: Some(format!("{EXCEL_PAGE}?ref=insta")),
                ..Signals::default()
            },
        };
        let id = fixture
            .affiliate
            .save_prospect(&typed, SignalOrigin::Typed)
            .await
            .unwrap();
        fixture
            .affiliate
            .dismiss_prospect(id, "já vi")
            .await
            .unwrap();

        let read = fixture
            .affiliate
            .read_prospects(&Vitrine(vec![excel()]), None)
            .await
            .unwrap();

        assert_eq!((read.added, read.dismissed), (0, 1));
        let kept = fixture.affiliate.prospect(id).await.unwrap();
        assert_eq!(kept.key, "4021");
        assert_eq!(kept.name, "Excel do Zero ao Avançado");
        assert_eq!(kept.signals.commission_rate, Some(percent("40")));
        assert_eq!(kept.signals.students, Some(12_000));
        assert_eq!(
            kept.reading(SignalKind::CommissionRate).unwrap().origin,
            SignalOrigin::Typed
        );
        assert_eq!(kept.dismissed.as_deref(), Some("já vi"));
    });
}

#[test]
fn reading_a_prospects_page_adds_what_only_the_page_shows_and_keeps_the_rest() {
    block_on(async {
        let fixture = Fixture::new().await;
        let vitrine = Vitrine(vec![excel(), photo()]);
        fixture
            .affiliate
            .read_prospects(&vitrine, None)
            .await
            .unwrap();
        let id = fixture.affiliate.prospects(None).await.unwrap()[0].id;
        let typed = FoundProspect {
            signals: Signals {
                temperature: Some(30),
                ..Signals::default()
            },
            ..excel()
        };
        fixture
            .affiliate
            .save_prospect(&typed, SignalOrigin::Typed)
            .await
            .unwrap();
        fixture.clock.advance(TimeDelta::hours(1));

        let read = fixture
            .affiliate
            .read_prospect_details(&vitrine, id)
            .await
            .unwrap();
        let again = fixture
            .affiliate
            .read_prospect_details(&vitrine, id)
            .await
            .unwrap();

        assert_eq!(read.id, id);
        assert_eq!(read.signals.warranty_days, Some(7));
        assert_eq!(read.signals.temperature, Some(30));
        assert_eq!(read.signals.students, Some(12_000));
        let warranty = read.reading(SignalKind::WarrantyDays).unwrap();
        assert_eq!(
            (warranty.origin, warranty.read_at),
            (SignalOrigin::Vitrine, noon() + TimeDelta::hours(1))
        );
        assert_eq!(again, read);
        assert_eq!(fixture.affiliate.prospects(None).await.unwrap().len(), 2);
    });
}

#[test]
fn a_page_is_read_only_from_its_own_platform_and_a_failed_read_keeps_the_prospect() {
    block_on(async {
        let fixture = Fixture::new().await;
        let kiwify = FoundProspect {
            platform: AffiliatePlatform::Kiwify,
            ..photo()
        };
        let other = fixture
            .affiliate
            .save_prospect(&kiwify, SignalOrigin::Typed)
            .await
            .unwrap();
        let id = fixture
            .affiliate
            .save_prospect(&photo(), SignalOrigin::Typed)
            .await
            .unwrap();
        let before = fixture.affiliate.prospect(id).await.unwrap();

        let wrong = fixture
            .affiliate
            .read_prospect_details(&Vitrine(Vec::new()), other)
            .await;
        let failed = fixture.affiliate.read_prospect_details(&Blocked, id).await;

        assert!(matches!(wrong, Err(AffiliateError::NoSuchProspect)));
        assert!(matches!(failed, Err(AffiliateError::Platform(_))));
        assert_eq!(fixture.affiliate.prospect(id).await.unwrap(), before);
    });
}

#[test]
fn a_failed_read_keeps_nothing() {
    block_on(async {
        let fixture = Fixture::new().await;
        let failed = fixture.affiliate.read_prospects(&Blocked, None).await;
        assert!(matches!(failed, Err(AffiliateError::Platform(_))));
        assert!(fixture.affiliate.prospects(None).await.unwrap().is_empty());
    });
}

/// A vitrine that shows the same products on every read.
struct Vitrine(Vec<FoundProspect>);

impl ProspectSource for Vitrine {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Hotmart
    }

    fn origin(&self) -> SignalOrigin {
        SignalOrigin::Vitrine
    }

    fn categories(&self) -> Result<Vec<ProspectCategory>, PlatformError> {
        Ok(Vec::new())
    }

    fn prospects(&self, _: Option<&ProspectCategory>) -> Result<Vec<FoundProspect>, PlatformError> {
        Ok(self.0.clone())
    }

    /// The product whose page `known` has, with its students read there.
    fn details(&self, known: &FoundProspect) -> Result<FoundProspect, PlatformError> {
        let mut page = self
            .0
            .iter()
            .find(|found| found.signals.product_page == known.signals.product_page)
            .cloned()
            .ok_or(PlatformError::NotFound)?;
        page.signals.warranty_days = Some(7);
        Ok(page)
    }

    fn blocked_until(&self) -> Option<Timestamp> {
        None
    }
}

/// A vitrine that stopped answering.
struct Blocked;

impl ProspectSource for Blocked {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Hotmart
    }

    fn origin(&self) -> SignalOrigin {
        SignalOrigin::Vitrine
    }

    fn categories(&self) -> Result<Vec<ProspectCategory>, PlatformError> {
        Err(PlatformError::NotFound)
    }

    fn prospects(&self, _: Option<&ProspectCategory>) -> Result<Vec<FoundProspect>, PlatformError> {
        Err(PlatformError::NotFound)
    }

    fn details(&self, _: &FoundProspect) -> Result<FoundProspect, PlatformError> {
        Err(PlatformError::NotFound)
    }

    fn blocked_until(&self) -> Option<Timestamp> {
        Some(noon())
    }
}

const EXCEL_MARKET: &str = "https://app.hotmart.com/market/details?productUcode=c4d5e6f7";

/// Excel as the market shows it: under the market's code when the card
/// does not show Hotmart's id, with its affiliation page.
fn excel_in_the_market() -> FoundProspect {
    FoundProspect {
        platform: AffiliatePlatform::Hotmart,
        key: "c4d5e6f7".into(),
        name: "Excel do Zero ao Avançado".into(),
        signals: Signals {
            temperature: Some(30),
            commission_rate: Some(percent("40")),
            affiliation_page: Some(EXCEL_MARKET.into()),
            ..Signals::default()
        },
    }
}

#[test]
fn the_market_finds_a_product_read_before_by_its_affiliation_page_and_keeps_its_key() {
    block_on(async {
        let fixture = Fixture::new().await;
        let read_before = FoundProspect {
            signals: Signals {
                affiliation_page: Some(EXCEL_MARKET.into()),
                ..excel().signals
            },
            ..excel()
        };
        let id = fixture
            .affiliate
            .save_prospect(&read_before, SignalOrigin::Vitrine)
            .await
            .unwrap();

        let read = fixture
            .affiliate
            .read_prospects(&Market(vec![excel_in_the_market()]), None)
            .await
            .unwrap();

        assert_eq!((read.found, read.added), (1, 0));
        let kept = fixture.affiliate.prospects(None).await.unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!((kept[0].id, kept[0].key.as_str()), (id, "4021"));
        assert_eq!(kept[0].signals.temperature, Some(30));
        assert_eq!(kept[0].signals.students, Some(12_000));
        assert_eq!(
            kept[0].reading(SignalKind::Temperature).unwrap().origin,
            SignalOrigin::Market
        );
        assert_eq!(
            kept[0].reading(SignalKind::Students).unwrap().origin,
            SignalOrigin::Vitrine
        );
    });
}

#[test]
fn two_prospects_of_one_product_fold_into_the_one_just_read_keeping_signals_and_choices() {
    block_on(async {
        let fixture = Fixture::new().await;
        let in_the_market = fixture
            .affiliate
            .save_prospect(&excel_in_the_market(), SignalOrigin::Market)
            .await
            .unwrap();
        let program = fixture
            .affiliate
            .promote_prospect(in_the_market)
            .await
            .unwrap();
        fixture
            .affiliate
            .dismiss_prospect(in_the_market, "nicho saturado")
            .await
            .unwrap();
        let in_the_vitrine = fixture
            .affiliate
            .save_prospect(&excel(), SignalOrigin::Vitrine)
            .await
            .unwrap();
        fixture.clock.advance(TimeDelta::hours(1));
        let page = FoundProspect {
            signals: Signals {
                warranty_days: Some(7),
                affiliation_page: Some(EXCEL_MARKET.into()),
                ..Signals::default()
            },
            ..excel()
        };

        let id = fixture
            .affiliate
            .save_prospect(&page, SignalOrigin::Vitrine)
            .await
            .unwrap();

        assert_eq!(id, in_the_vitrine);
        assert!(fixture.affiliate.prospects(None).await.unwrap().is_empty());
        assert!(matches!(
            fixture.affiliate.prospect(in_the_market).await,
            Err(AffiliateError::NoSuchProspect)
        ));
        let kept = fixture.affiliate.prospect(id).await.unwrap();
        assert_eq!(kept.key, "4021");
        assert_eq!(kept.dismissed.as_deref(), Some("nicho saturado"));
        assert_eq!((kept.program, kept.chosen), (Some(program.id), true));
        assert_eq!(kept.signals.temperature, Some(30));
        assert_eq!(kept.signals.commission_rate, Some(percent("40")));
        assert_eq!(kept.signals.students, Some(12_000));
        assert_eq!(kept.signals.warranty_days, Some(7));
        let temperature = kept.reading(SignalKind::Temperature).unwrap();
        assert_eq!(
            (temperature.origin, temperature.read_at),
            (SignalOrigin::Market, noon())
        );
        let page_reading = kept.reading(SignalKind::AffiliationPage).unwrap();
        assert_eq!(
            (page_reading.origin, page_reading.read_at),
            (SignalOrigin::Vitrine, noon() + TimeDelta::hours(1))
        );
        assert_eq!(
            fixture.affiliate.dismissed_prospects(None).await.unwrap(),
            1
        );

        let again = fixture
            .affiliate
            .read_prospects(&Market(vec![excel_in_the_market()]), None)
            .await
            .unwrap();
        assert_eq!((again.added, again.dismissed), (0, 1));
        assert_eq!(fixture.affiliate.prospect(id).await.unwrap().key, "4021");
    });
}

/// The logged-in market, showing the same products on every read.
struct Market(Vec<FoundProspect>);

impl ProspectSource for Market {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Hotmart
    }

    fn origin(&self) -> SignalOrigin {
        SignalOrigin::Market
    }

    fn categories(&self) -> Result<Vec<ProspectCategory>, PlatformError> {
        Ok(Vec::new())
    }

    fn prospects(&self, _: Option<&ProspectCategory>) -> Result<Vec<FoundProspect>, PlatformError> {
        Ok(self.0.clone())
    }

    fn details(&self, known: &FoundProspect) -> Result<FoundProspect, PlatformError> {
        Ok(known.clone())
    }

    fn blocked_until(&self) -> Option<Timestamp> {
        None
    }
}
