//! The browser engine and the Mercado Livre affiliate panel reader against
//! a fake DevTools endpoint answering with what the reading script would
//! read on the panel (see the fixtures' README), and the Affiliate module's
//! Sync over it with a real temporary database.

use std::str::FromStr;
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliatePlatform, CommissionFilter, CommissionSource, CommissionStatus, MIGRATIONS,
    Recorded,
};
use mascate_integrations::{
    AffiliatePanel, BrowserSession, FLAGS, MERCADO_LIVRE_AFFILIATE_PANEL,
    MERCADO_LIVRE_AFFILIATE_PANEL_URL, MercadoLivreAffiliatePanel, PanelState, TABLES_SCRIPT,
    installed_browser,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, PlatformError, Timestamp};
use mascate_platform::{Database, FlagKind, Phase, migrate};
use rust_decimal::Decimal;
use serde_json::Value;

mod browsing;

use browsing::{FakeDevTools, RecordedPauses};

const REPORT: &str = "https://www.mercadolivre.com.br/afiliados/relatorios";

fn fixture(name: &str) -> Value {
    browsing::fixture("mercado-livre-affiliate-panel", name)
}

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

/// In Brasília time, which the panel shows.
fn brasilia(month: u32, day: u32, hour: u32, minute: u32) -> Timestamp {
    Utc.with_ymd_and_hms(2026, month, day, hour, minute, 0)
        .unwrap()
        + TimeDelta::hours(3)
}

struct Setup {
    devtools: FakeDevTools,
    pauses: Arc<RecordedPauses>,
    panel: MercadoLivreAffiliatePanel,
    _profile: tempfile::TempDir,
}

/// The panel through a browser at the fake endpoint, on a profile the
/// owner already opened.
fn setup() -> Setup {
    let devtools = FakeDevTools::start();
    let profile = tempfile::tempdir().unwrap();
    let pauses = Arc::new(RecordedPauses::default());
    let browser = BrowserSession::attached(devtools.endpoint(), profile.path().to_path_buf())
        .with_pause(pauses.clone());
    let panel =
        MercadoLivreAffiliatePanel::new(MERCADO_LIVRE_AFFILIATE_PANEL_URL, Arc::new(browser));
    Setup {
        devtools,
        pauses,
        panel,
        _profile: profile,
    }
}

fn read(
    panel: &MercadoLivreAffiliatePanel,
) -> Result<Vec<mascate_affiliate::ReportedConversion>, PlatformError> {
    let now = Utc.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap();
    panel.conversions(now - TimeDelta::days(90)..now)
}

#[test]
fn the_reader_is_a_restricted_feature_off_until_the_owner_turns_it_on() {
    assert!(FLAGS.contains(&MERCADO_LIVRE_AFFILIATE_PANEL));
    assert_eq!(
        MERCADO_LIVRE_AFFILIATE_PANEL.key,
        "integrations.mercado_livre_affiliate_panel"
    );
    assert_eq!(
        MERCADO_LIVRE_AFFILIATE_PANEL.kind,
        FlagKind::RestrictedFeature
    );
    assert_eq!(MERCADO_LIVRE_AFFILIATE_PANEL.phase, Phase::Three);
    assert!(MERCADO_LIVRE_AFFILIATE_PANEL.reason.contains("7.6"));
    assert!(
        MERCADO_LIVRE_AFFILIATE_PANEL
            .risk
            .contains("conta de afiliado")
    );
}

/// Key, product, sale, commission, status, the panel's status, tracking.
type Read<'a> = (
    &'a str,
    &'a str,
    Timestamp,
    Money,
    CommissionStatus,
    &'a str,
    Option<&'a str>,
);

#[test]
fn each_sale_of_the_report_is_a_conversion_keyed_on_the_panels_sale_id() {
    let s = setup();
    s.devtools.then_read(fixture("report.json"));

    let conversions = read(&s.panel).unwrap();

    let read: Vec<Read> = conversions
        .iter()
        .map(|conversion| {
            (
                conversion.key.as_str(),
                conversion.product.as_str(),
                conversion.sold_at,
                conversion.commission,
                conversion.status,
                conversion.platform_status.as_str(),
                conversion.tracking.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        read,
        [
            (
                "2000009876543210",
                "Fone de Ouvido Bluetooth JBL Tune 520BT",
                brasilia(10, 18, 14, 32),
                brl("12.50"),
                CommissionStatus::Pending,
                "Pendente",
                Some("telegram-ofertas"),
            ),
            (
                "2000009876500001",
                "Air Fryer Mondial 4L",
                brasilia(10, 12, 9, 5),
                brl("31.92"),
                CommissionStatus::Approved,
                "Aprovada",
                None,
            ),
            (
                "2000009876400002",
                "Kit 3 Cuecas Boxer",
                brasilia(10, 2, 21, 40),
                brl("4.79"),
                CommissionStatus::Paid,
                "Paga",
                Some("instagram"),
            ),
            (
                "2000009876300003",
                "Smartwatch D20",
                brasilia(9, 28, 11, 15),
                brl("7.19"),
                CommissionStatus::Cancelled,
                "Cancelada",
                None,
            ),
            (
                "2000009876200004",
                "Mochila para Notebook",
                brasilia(9, 25, 16, 0),
                brl("91.92"),
                CommissionStatus::Reversed,
                "Devolvida",
                None,
            ),
            // Two items of one sale: one Conversion, the commissions added.
            (
                "2000009876100005",
                "Capinha iPhone 15 + Película iPhone 15",
                brasilia(9, 20, 10, 0),
                brl("3.98"),
                CommissionStatus::Approved,
                "Aprovada",
                None,
            ),
        ]
    );
    assert!(conversions.iter().all(|conversion| {
        conversion.program.id == "mercado-livre-affiliates"
            && conversion.program.name == "Afiliados Mercado Livre"
    }));
    let raw: Value = serde_json::from_str(&conversions[5].raw).unwrap();
    assert_eq!(raw[1]["Produto"], "Película iPhone 15");
    assert_eq!(raw[0]["ID da venda"], "2000009876100005");
    assert_eq!(s.panel.state(), PanelState::Ready);
}

#[test]
fn the_reader_opens_the_report_in_a_tab_and_only_reads_it() {
    let s = setup();
    s.devtools.then_read(fixture("report.json"));

    read(&s.panel).unwrap();

    assert_eq!(s.devtools.navigations(), [REPORT]);
    assert_eq!(
        s.devtools.methods(),
        ["Page.enable", "Page.navigate", "Runtime.evaluate"]
    );
    let evaluated = &s.devtools.commands()[2].params;
    assert_eq!(evaluated["expression"], TABLES_SCRIPT);
    assert_eq!(evaluated["returnByValue"], true);
    assert_eq!(
        s.devtools.tab_requests(),
        ["PUT /json/new?about:blank", "GET /json/close/T1"]
    );
}

#[test]
fn a_report_still_drawing_is_read_again_until_its_table_shows() {
    let s = setup();
    s.devtools.then_read(fixture("drawing.json"));
    s.devtools.then_read(fixture("drawing.json"));
    s.devtools.then_read(fixture("report.json"));

    let conversions = read(&s.panel).unwrap();

    assert_eq!(conversions.len(), 6);
    assert_eq!(s.pauses.count(), 2);
    assert_eq!(
        s.devtools
            .methods()
            .iter()
            .filter(|method| *method == "Runtime.evaluate")
            .count(),
        3
    );
}

#[test]
fn a_report_that_never_draws_its_table_fails_and_says_so() {
    let s = setup();
    s.devtools.then_read(fixture("drawing.json"));

    let error = read(&s.panel).unwrap_err();

    assert!(
        matches!(&error, PlatformError::Failed(why) if why.contains("sem a tabela de vendas")),
        "{error:?}"
    );
    assert_eq!(s.pauses.count(), 29);
    assert_eq!(s.panel.state(), PanelState::Ready);
}

#[test]
fn a_report_whose_columns_changed_fails_rather_than_reading_the_wrong_cells() {
    let s = setup();
    s.devtools.then_read(fixture("changed.json"));

    let error = read(&s.panel).unwrap_err();

    assert!(
        matches!(&error, PlatformError::Failed(why) if why.contains("colunas esperadas")),
        "{error:?}"
    );
}

#[test]
fn a_login_page_instead_of_the_panel_marks_the_connection_as_logged_out() {
    let s = setup();
    s.devtools.then_read(fixture("login.json"));

    let error = read(&s.panel).unwrap_err();

    assert_eq!(error, PlatformError::Expired);
    assert_eq!(s.panel.state(), PanelState::LoggedOut);
    assert!(!s.panel.state().can_read());
    // One reading: a login page is an answer, not a page still drawing.
    assert_eq!(s.pauses.count(), 0);
}

#[test]
fn logging_in_again_opens_the_panel_in_the_window_and_the_next_reading_goes_through() {
    let s = setup();
    s.devtools.then_read(fixture("login.json"));
    read(&s.panel).unwrap_err();

    s.panel.open_login().unwrap();

    assert_eq!(s.panel.state(), PanelState::Ready);
    assert_eq!(
        s.devtools.tab_requests().last().unwrap(),
        "PUT /json/new?https://www.mercadolivre.com.br/afiliados"
    );
    s.devtools.then_read(fixture("report.json"));
    // The login page answered once; the report keeps answering after it.
    assert_eq!(read(&s.panel).unwrap().len(), 6);
    assert_eq!(s.panel.state(), PanelState::Ready);
}

#[test]
fn a_page_that_does_not_open_fails_with_the_browsers_words() {
    let s = setup();
    s.devtools.fail_navigation("net::ERR_INTERNET_DISCONNECTED");

    let error = read(&s.panel).unwrap_err();

    assert!(
        matches!(&error, PlatformError::Failed(why) if why.contains("ERR_INTERNET_DISCONNECTED")),
        "{error:?}"
    );
    assert_eq!(
        s.devtools.tab_requests().last().unwrap(),
        "GET /json/close/T1"
    );
}

#[test]
fn without_a_browser_installed_the_connection_says_so_and_nothing_is_opened() {
    let profile = tempfile::tempdir().unwrap();
    let panel = MercadoLivreAffiliatePanel::new(
        MERCADO_LIVRE_AFFILIATE_PANEL_URL,
        Arc::new(BrowserSession::new(None, profile.path().to_path_buf())),
    );

    assert_eq!(panel.state(), PanelState::NoBrowser);
    assert!(!panel.state().can_read());
    let error = read(&panel).unwrap_err();
    assert!(
        matches!(&error, PlatformError::Refused(why) if why.contains("Edge nem Chrome")),
        "{error:?}"
    );
    assert!(matches!(panel.open_login(), Err(PlatformError::Refused(_))));
}

#[test]
fn before_the_owner_ever_logs_in_the_panel_is_not_connected_and_no_page_opens() {
    let devtools = FakeDevTools::start();
    let profile = tempfile::tempdir().unwrap();
    let never_opened = profile.path().join("navegador");
    let panel = MercadoLivreAffiliatePanel::new(
        MERCADO_LIVRE_AFFILIATE_PANEL_URL,
        Arc::new(BrowserSession::attached(devtools.endpoint(), never_opened)),
    );

    assert_eq!(panel.state(), PanelState::NeverLoggedIn);
    assert_eq!(read(&panel).unwrap_err(), PlatformError::NotConnected);
    assert!(devtools.tab_requests().is_empty());
}

#[test]
fn the_installed_browser_is_found_where_its_package_puts_it() {
    let folder = tempfile::tempdir().unwrap();
    let nothing = |_: &str| None::<String>;
    assert_eq!(installed_browser(&nothing), None);

    let (environment_name, program) = if cfg!(windows) {
        (
            "ProgramFiles(x86)",
            folder.path().join(r"Microsoft\Edge\Application\msedge.exe"),
        )
    } else {
        ("PATH", folder.path().join("chromium"))
    };
    std::fs::create_dir_all(program.parent().unwrap()).unwrap();
    std::fs::write(&program, "").unwrap();
    let folder_text = folder.path().display().to_string();
    let environment = move |name: &str| (name == environment_name).then(|| folder_text.clone());

    assert_eq!(installed_browser(&environment), Some(program));
}

struct Synced {
    _dir: tempfile::TempDir,
    affiliate: Affiliate,
}

async fn affiliate() -> Synced {
    let dir = tempfile::tempdir().unwrap();
    let database = Arc::new(
        Database::open(&dir.path().join("mascate.db"))
            .await
            .unwrap(),
    );
    let clock = Arc::new(ManualClock::at(
        Utc.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap(),
    ));
    migrate(&database, clock.as_ref(), &[MIGRATIONS])
        .await
        .unwrap();
    Synced {
        affiliate: Affiliate::new(database, clock, Arc::new(SequentialIds::default())),
        _dir: dir,
    }
}

#[test]
fn reading_the_same_panel_twice_yields_the_same_conversions_and_commissions() {
    block_on(async {
        let s = setup();
        let synced = affiliate().await;
        s.devtools.then_read(fixture("report.json"));

        let first = synced.affiliate.sync(&s.panel).await.unwrap();
        let commissions = synced
            .affiliate
            .commissions(&CommissionFilter::default())
            .await
            .unwrap();
        let again = synced.affiliate.sync(&s.panel).await.unwrap();

        assert_eq!(first.platform, AffiliatePlatform::MercadoLivre);
        assert_eq!(first.recorded.added, 6);
        assert_eq!(
            again.recorded,
            Recorded {
                added: 0,
                changed: 0,
                unchanged: 6
            }
        );
        assert_eq!(
            synced
                .affiliate
                .commissions(&CommissionFilter::default())
                .await
                .unwrap(),
            commissions
        );
        let earned: Vec<(&str, Money)> = commissions
            .iter()
            .filter(|commission| commission.status.is_earned())
            .map(|commission| (commission.product.as_str(), commission.amount))
            .collect();
        assert_eq!(
            earned,
            [
                ("Air Fryer Mondial 4L", brl("31.92")),
                ("Kit 3 Cuecas Boxer", brl("4.79")),
                ("Capinha iPhone 15 + Película iPhone 15", brl("3.98")),
            ]
        );
    });
}

#[test]
fn a_sale_whose_status_moved_on_the_panel_changes_its_commission_only() {
    block_on(async {
        let s = setup();
        let synced = affiliate().await;
        s.devtools.then_read(fixture("report.json"));
        synced.affiliate.sync(&s.panel).await.unwrap();

        s.devtools.then_read(fixture("report-later.json"));
        let later = synced.affiliate.sync(&s.panel).await.unwrap();

        assert_eq!(
            later.recorded,
            Recorded {
                added: 0,
                changed: 2,
                unchanged: 0
            }
        );
        let commissions = synced
            .affiliate
            .commissions(&CommissionFilter::default())
            .await
            .unwrap();
        let statuses: Vec<(&str, CommissionStatus)> = commissions
            .iter()
            .take(2)
            .map(|commission| (commission.product.as_str(), commission.status))
            .collect();
        assert_eq!(
            statuses,
            [
                (
                    "Fone de Ouvido Bluetooth JBL Tune 520BT",
                    CommissionStatus::Approved
                ),
                ("Air Fryer Mondial 4L", CommissionStatus::Paid),
            ]
        );
        assert_eq!(commissions.len(), 6);
    });
}

#[test]
fn the_report_reads_dates_as_the_panel_may_write_them() {
    let s = setup();
    let row =
        |date: &str, id: &str| serde_json::json!([date, id, "Produto", "R$ 1,00", "Aprovada"]);
    s.devtools.then_read(serde_json::json!({
        "url": REPORT,
        "tables": [{
            "headers": ["Data", "Venda", "Anúncio", "Sua comissão", "Situação"],
            "rows": [
                row("21 out. 2026", "1"),
                row("5 de outubro de 2026", "2"),
                row("07/10/26 08:00", "3"),
                row("07/10/2026", "4"),
            ]
        }]
    }));

    let sold: Vec<Timestamp> = read(&s.panel)
        .unwrap()
        .iter()
        .map(|conversion| conversion.sold_at)
        .collect();

    assert_eq!(
        sold,
        [
            brasilia(10, 21, 0, 0),
            brasilia(10, 5, 0, 0),
            brasilia(10, 7, 8, 0),
            brasilia(10, 7, 0, 0),
        ]
    );
}

#[test]
fn a_sale_the_app_cannot_read_fails_the_reading_and_names_the_sale() {
    let s = setup();
    s.devtools.then_read(serde_json::json!({
        "url": REPORT,
        "tables": [{
            "headers": ["Data da venda", "ID da venda", "Produto", "Comissão", "Status"],
            "rows": [["ontem", "2000009876543210", "Fone", "R$ 1,00", "Aprovada"]]
        }]
    }));

    let error = read(&s.panel).unwrap_err();

    assert!(
        matches!(&error, PlatformError::Failed(why) if why.contains("2000009876543210") && why.contains("ontem")),
        "{error:?}"
    );
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(48))]

    /// Whatever the cents, the commission the panel writes in reais is the
    /// one recorded, and the items of a sale add up to the cent.
    #[test]
    fn commissions_read_to_the_cent_and_items_add_up(
        first in 0u64..10_000_000,
        second in 0u64..10_000_000,
    ) {
        let s = setup();
        let reais = |cents: u64| {
            let whole = (cents / 100).to_string();
            let mut grouped = String::new();
            for (place, digit) in whole.chars().enumerate() {
                if place > 0 && (whole.len() - place).is_multiple_of(3) {
                    grouped.push('.');
                }
                grouped.push(digit);
            }
            format!("R$ {grouped},{:02}", cents % 100)
        };
        let row = |product: &str, cents: u64| {
            serde_json::json!(["18/10/2026", "7", product, reais(cents), "Aprovada"])
        };
        s.devtools.then_read(serde_json::json!({
            "url": REPORT,
            "tables": [{
                "headers": ["Data da venda", "ID da venda", "Produto", "Comissão", "Status"],
                "rows": [row("A", first), row("B", second)]
            }]
        }));

        let conversions = read(&s.panel).unwrap();

        proptest::prop_assert_eq!(conversions.len(), 1);
        proptest::prop_assert_eq!(
            conversions[0].commission,
            Money::new(Decimal::new((first + second) as i64, 2), Currency::Brl)
        );
    }
}
