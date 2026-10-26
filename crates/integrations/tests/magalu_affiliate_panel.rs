//! The Magalu affiliate panel reader against a fake DevTools endpoint
//! answering with what the reading script would read on the panel (see the
//! fixtures' README), and the Affiliate module's Sync over it with a real
//! temporary database.

use std::str::FromStr;
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_affiliate::{
    Affiliate, AffiliatePlatform, CommissionFilter, CommissionSource, CommissionStatus, MIGRATIONS,
    Recorded, ReportedConversion,
};
use mascate_integrations::{
    AffiliatePanel, BrowserSession, FLAGS, MAGALU_AFFILIATE_PANEL, MAGALU_AFFILIATE_PANEL_URL,
    MagaluAffiliatePanel, PanelState, TABLES_SCRIPT,
};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Currency, Money, PlatformError, Timestamp};
use mascate_platform::{Database, FlagKind, Phase, migrate};
use rust_decimal::Decimal;
use serde_json::Value;

mod browsing;

use browsing::{FakeDevTools, RecordedPauses};

fn fixture(name: &str) -> Value {
    browsing::fixture("magalu-affiliate-panel", name)
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
    panel: MagaluAffiliatePanel,
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
    let panel = MagaluAffiliatePanel::new(MAGALU_AFFILIATE_PANEL_URL, Arc::new(browser));
    Setup {
        devtools,
        pauses,
        panel,
        _profile: profile,
    }
}

fn read(panel: &MagaluAffiliatePanel) -> Result<Vec<ReportedConversion>, PlatformError> {
    let now = Utc.with_ymd_and_hms(2026, 10, 22, 12, 0, 0).unwrap();
    panel.conversions(now - TimeDelta::days(90)..now)
}

#[test]
fn the_reader_is_a_restricted_feature_off_until_the_owner_turns_it_on() {
    assert!(FLAGS.contains(&MAGALU_AFFILIATE_PANEL));
    assert_eq!(
        MAGALU_AFFILIATE_PANEL.key,
        "integrations.magalu_affiliate_panel"
    );
    assert_eq!(MAGALU_AFFILIATE_PANEL.kind, FlagKind::RestrictedFeature);
    assert_eq!(MAGALU_AFFILIATE_PANEL.phase, Phase::Three);
    assert!(MAGALU_AFFILIATE_PANEL.reason.contains("não foram lidos"));
    assert!(
        MAGALU_AFFILIATE_PANEL
            .reason
            .contains("não resolve captcha")
    );
    assert!(MAGALU_AFFILIATE_PANEL.risk.contains("Parceiro Magalu"));
}

/// Key, product, sale, commission, status, the panel's status.
type Read<'a> = (
    &'a str,
    &'a str,
    Timestamp,
    Money,
    CommissionStatus,
    &'a str,
);

#[test]
fn each_order_of_the_sales_page_is_a_conversion_keyed_on_the_panels_order_number() {
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
            )
        })
        .collect();
    assert_eq!(
        read,
        [
            (
                "1491507293822611",
                "Smart TV 50\" 4K Samsung Crystal UHD",
                brasilia(10, 19, 20, 14),
                brl("18.40"),
                CommissionStatus::Pending,
                "Aguardando aprovação",
            ),
            (
                "1491507293700342",
                "Liquidificador Philips Walita 1200W",
                brasilia(10, 11, 10, 2),
                brl("13.99"),
                CommissionStatus::Approved,
                "Aprovada",
            ),
            (
                "1491507293600173",
                "Jogo de Panelas Tramontina 5 Peças",
                brasilia(10, 3, 8, 45),
                brl("17.50"),
                CommissionStatus::Approved,
                "Liberada",
            ),
            (
                "1491507293500984",
                "Fritadeira Elétrica Mondial 4L",
                brasilia(9, 27, 19, 30),
                brl("19.95"),
                CommissionStatus::Paid,
                "Paga",
            ),
            (
                "1491507293400415",
                "Ventilador Arno 40cm",
                brasilia(9, 22, 13, 12),
                brl("6.90"),
                CommissionStatus::Cancelled,
                "Cancelada",
            ),
            (
                "1491507293300556",
                "Cafeteira Nespresso Essenza Mini",
                brasilia(9, 15, 9, 0),
                brl("13.47"),
                CommissionStatus::Reversed,
                "Devolvida",
            ),
            // Two items of one order: one Conversion, the commissions added.
            (
                "1491507293200667",
                "Fone de Ouvido JBL Tune 520BT + Capa para Fone JBL",
                brasilia(9, 10, 17, 40),
                brl("8.69"),
                CommissionStatus::Paid,
                "Paga",
            ),
        ]
    );
    assert!(conversions.iter().all(|conversion| {
        conversion.program.id == "magalu-affiliates"
            && conversion.program.name == "Parceiro Magalu"
            && conversion.tracking.is_none()
    }));
    let raw: Value = serde_json::from_str(&conversions[6].raw).unwrap();
    assert_eq!(raw[1]["Produto"], "Capa para Fone JBL");
    assert_eq!(raw[1]["Quantidade"], "2");
    assert_eq!(s.panel.state(), PanelState::Ready);
}

#[test]
fn the_reader_opens_the_sales_page_in_a_tab_and_only_reads_it() {
    let s = setup();
    s.devtools.then_read(fixture("report.json"));

    read(&s.panel).unwrap();

    assert_eq!(
        s.devtools.navigations(),
        ["https://www.magazinevoce.com.br/painel/vendas"]
    );
    assert_eq!(
        s.devtools.methods(),
        ["Page.enable", "Page.navigate", "Runtime.evaluate"]
    );
    assert_eq!(s.devtools.commands()[2].params["expression"], TABLES_SCRIPT);
    assert_eq!(
        s.devtools.tab_requests(),
        ["PUT /json/new?about:blank", "GET /json/close/T1"]
    );
}

#[test]
fn a_sales_page_still_drawing_is_read_again_until_its_table_shows() {
    let s = setup();
    s.devtools.then_read(fixture("drawing.json"));
    s.devtools.then_read(fixture("report.json"));

    assert_eq!(read(&s.panel).unwrap().len(), 7);
    assert_eq!(s.pauses.count(), 1);
}

#[test]
fn a_sales_page_whose_columns_changed_fails_rather_than_reading_the_wrong_cells() {
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
    assert_eq!(s.pauses.count(), 0);
}

#[test]
fn magalus_check_of_who_is_asking_waits_for_the_owner_like_a_login() {
    let s = setup();
    s.devtools.then_read(fixture("verification.json"));

    let error = read(&s.panel).unwrap_err();

    assert_eq!(error, PlatformError::Expired);
    assert_eq!(s.panel.state(), PanelState::LoggedOut);
    // The reader never answers the check: one reading, no click.
    assert_eq!(s.pauses.count(), 0);
    assert!(
        !s.devtools
            .methods()
            .iter()
            .any(|method| method.starts_with("Input."))
    );
}

#[test]
fn logging_in_again_opens_the_panel_in_the_window_and_the_next_reading_goes_through() {
    let s = setup();
    s.devtools.then_read(fixture("verification.json"));
    read(&s.panel).unwrap_err();

    s.panel.open_login().unwrap();

    assert_eq!(s.panel.state(), PanelState::Ready);
    assert_eq!(
        s.devtools.tab_requests().last().unwrap(),
        "PUT /json/new?https://www.magazinevoce.com.br/painel"
    );
    s.devtools.then_read(fixture("report.json"));
    assert_eq!(read(&s.panel).unwrap().len(), 7);
}

#[test]
fn without_a_browser_or_a_login_nothing_is_opened() {
    let profile = tempfile::tempdir().unwrap();
    let no_browser = MagaluAffiliatePanel::new(
        MAGALU_AFFILIATE_PANEL_URL,
        Arc::new(BrowserSession::new(None, profile.path().to_path_buf())),
    );
    let devtools = FakeDevTools::start();
    let never_logged_in = MagaluAffiliatePanel::new(
        MAGALU_AFFILIATE_PANEL_URL,
        Arc::new(BrowserSession::attached(
            devtools.endpoint(),
            profile.path().join("navegador"),
        )),
    );

    assert_eq!(no_browser.state(), PanelState::NoBrowser);
    assert!(matches!(read(&no_browser), Err(PlatformError::Refused(_))));
    assert_eq!(never_logged_in.state(), PanelState::NeverLoggedIn);
    assert_eq!(
        read(&never_logged_in).unwrap_err(),
        PlatformError::NotConnected
    );
    assert!(devtools.tab_requests().is_empty());
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
fn reading_the_same_panel_twice_yields_the_same_commissions() {
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

        assert_eq!(first.platform, AffiliatePlatform::Magalu);
        assert_eq!(first.recorded.added, 7);
        assert_eq!(
            again.recorded,
            Recorded {
                added: 0,
                changed: 0,
                unchanged: 7
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
                ("Liquidificador Philips Walita 1200W", brl("13.99")),
                ("Jogo de Panelas Tramontina 5 Peças", brl("17.50")),
                ("Fritadeira Elétrica Mondial 4L", brl("19.95")),
                (
                    "Fone de Ouvido JBL Tune 520BT + Capa para Fone JBL",
                    brl("8.69")
                ),
            ]
        );
    });
}

#[test]
fn an_order_whose_status_moved_on_the_panel_changes_its_commission_only() {
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
        assert_eq!(commissions.len(), 7);
        let statuses: Vec<(&str, CommissionStatus)> = commissions
            .iter()
            .take(2)
            .map(|commission| (commission.platform_status.as_str(), commission.status))
            .collect();
        assert_eq!(
            statuses,
            [
                ("Aprovada", CommissionStatus::Approved),
                ("Paga", CommissionStatus::Paid),
            ]
        );
    });
}
