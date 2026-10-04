//! Listing Quality through marketing's public interface, with a real
//! temporary database and an in-memory channel: each listing's level, score,
//! pending actions and visits are kept once by the channel's ids, and the
//! panel puts first the listings with something to fix that sell and draw
//! the most.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, PlatformError};
use mascate_marketing::{
    ActionKind, ChannelQuality, IMPACT_DAYS, ListedItem, ListingQuality, MIGRATIONS, QualityAction,
    QualityLevel, QualityRow, QualitySource, QualitySync, Rating,
};
use mascate_platform::{Database, migrate};

/// A channel answering from memory: each listing's quality (absent while
/// not worked out) and visits, and what it was asked.
#[derive(Default)]
struct Channel {
    quality: Mutex<HashMap<String, ChannelQuality>>,
    visits: Mutex<HashMap<String, u32>>,
    asked: Mutex<Vec<String>>,
    /// Fails asking about this listing while set.
    failing: Mutex<Option<(String, PlatformError)>>,
}

impl Channel {
    fn rates(&self, listing: &str, quality: ChannelQuality) {
        self.quality.lock().unwrap().insert(listing.into(), quality);
    }

    fn visited(&self, listing: &str, visits: u32) {
        self.visits.lock().unwrap().insert(listing.into(), visits);
    }
}

impl QualitySource for Channel {
    fn quality(&self, listing: &str) -> Result<Option<ChannelQuality>, PlatformError> {
        if let Some((failing, error)) = self.failing.lock().unwrap().clone()
            && failing == listing
        {
            return Err(error);
        }
        self.asked.lock().unwrap().push(listing.into());
        Ok(self.quality.lock().unwrap().get(listing).cloned())
    }

    fn visits(&self, listing: &str, days: u32) -> Result<u32, PlatformError> {
        assert_eq!(days, IMPACT_DAYS);
        Ok(self
            .visits
            .lock()
            .unwrap()
            .get(listing)
            .copied()
            .unwrap_or(0))
    }
}

fn action(key: &str, kind: ActionKind, text: &str) -> QualityAction {
    QualityAction {
        key: key.into(),
        kind,
        text: text.into(),
        label: "Corrigir".into(),
        link: Some(format!(
            "https://www.mercadolivre.com.br/syi/core/modify?taskId={key}"
        )),
    }
}

fn photos() -> QualityAction {
    action(
        "PICTURES_QUANTITY_MIN",
        ActionKind::Opportunity,
        "Adicione mais fotos, no mínimo 3.",
    )
}

fn required_specs() -> QualityAction {
    action(
        "TS_MAIN_QUALITY_INCOMPLETE_REQUIRED",
        ActionKind::Problem,
        "Complete os dados marcados como obrigatórios.",
    )
}

fn gtin() -> QualityAction {
    action(
        "HAS_GTIN",
        ActionKind::Opportunity,
        "Informe o código universal do produto.",
    )
}

fn rated(score: u8, level: QualityLevel, pending: Vec<QualityAction>) -> ChannelQuality {
    ChannelQuality {
        score,
        level,
        pending,
    }
}

fn listed(listing: &str, title: &str, units_sold: u32) -> ListedItem {
    ListedItem {
        listing: listing.into(),
        title: title.into(),
        link: Some(format!("https://produto.mercadolivre.com.br/{listing}")),
        units_sold,
    }
}

fn ids(listings: &[&str]) -> Vec<String> {
    listings.iter().map(|&id| id.to_owned()).collect()
}

fn order_of(rows: &[QualityRow]) -> Vec<&str> {
    rows.iter()
        .map(|row| row.listing.listing.as_str())
        .collect()
}

struct Fixture {
    _dir: tempfile::TempDir,
    clock: Arc<ManualClock>,
    quality: ListingQuality,
    channel: Channel,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
        ));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[MIGRATIONS])
            .await
            .unwrap();
        let quality =
            ListingQuality::new(database, clock.clone(), Arc::new(SequentialIds::default()));
        Self {
            _dir: dir,
            clock,
            quality,
            channel: Channel::default(),
        }
    }

    async fn sync(&self, listings: &[&str]) -> QualitySync {
        self.quality
            .sync(&self.channel, &ids(listings))
            .await
            .unwrap()
    }
}

#[test]
fn the_sync_keeps_each_listings_level_score_pending_actions_and_visits() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture.channel.rates(
            "MLB1",
            rated(
                48,
                QualityLevel::Basic,
                vec![photos(), required_specs(), gtin()],
            ),
        );
        fixture.channel.visited("MLB1", 340);

        let report = fixture.sync(&["MLB1"]).await;

        assert_eq!(
            report,
            QualitySync {
                checked: 1,
                not_rated: 0,
                pending: 3,
            }
        );
        let panel = fixture
            .quality
            .panel(&[listed("MLB1", "Fone Bluetooth", 4)])
            .await
            .unwrap();
        assert_eq!(panel.len(), 1);
        let row = &panel[0];
        assert_eq!(row.visits, Some(340));
        assert_eq!(row.checked_at, Some(fixture.clock.now()));
        assert_eq!(
            row.rating,
            // The problem that lowers the score comes before the
            // suggestions, which keep the channel's order.
            Rating::Rated(rated(
                48,
                QualityLevel::Basic,
                vec![required_specs(), photos(), gtin()],
            ))
        );
        assert_eq!(row.pending()[0].kind, ActionKind::Problem);
        assert_eq!(
            fixture.quality.last_sync().await.unwrap(),
            Some(fixture.clock.now())
        );
    });
}

#[test]
fn listings_with_something_to_fix_come_first_by_sales_then_visits_then_score() {
    block_on(async {
        let fixture = Fixture::new().await;
        let to_fix = |score| rated(score, QualityLevel::Standard, vec![photos()]);
        fixture.channel.rates("SELLS", to_fix(60));
        fixture.channel.rates("VISITED", to_fix(60));
        fixture.channel.rates("LOW_SCORE", to_fix(30));
        fixture.channel.rates("HIGH_SCORE", to_fix(62));
        fixture
            .channel
            .rates("DONE", rated(100, QualityLevel::Professional, Vec::new()));
        fixture.channel.visited("VISITED", 900);
        fixture.channel.visited("DONE", 5000);
        let all = [
            "SELLS",
            "VISITED",
            "LOW_SCORE",
            "HIGH_SCORE",
            "DONE",
            "WAITING",
        ];
        fixture.sync(&all).await;

        let panel = fixture
            .quality
            .panel(&[
                listed("DONE", "Vende muito e está completo", 50),
                listed("HIGH_SCORE", "Quase lá", 0),
                listed("WAITING", "Ainda sem nota", 10),
                listed("LOW_SCORE", "Nota baixa", 0),
                listed("VISITED", "Muitas visitas", 0),
                listed("SELLS", "Vende", 3),
                listed("NEW", "Chegou depois do Sync", 1),
            ])
            .await
            .unwrap();

        assert_eq!(
            order_of(&panel),
            [
                "SELLS",
                "VISITED",
                "LOW_SCORE",
                "HIGH_SCORE",
                "WAITING",
                "NEW",
                "DONE"
            ]
        );
        let rating = |listing: &str| {
            panel
                .iter()
                .find(|row| row.listing.listing == listing)
                .unwrap()
                .rating
                .clone()
        };
        assert_eq!(rating("WAITING"), Rating::NotRated);
        assert_eq!(rating("NEW"), Rating::NotChecked);
    });
}

#[test]
fn a_second_sync_with_the_same_answers_changes_nothing_and_done_actions_leave() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture.channel.rates(
            "MLB1",
            rated(48, QualityLevel::Basic, vec![photos(), gtin()]),
        );
        fixture.sync(&["MLB1"]).await;
        let listing = [listed("MLB1", "Fone Bluetooth", 0)];
        let first = fixture.quality.panel(&listing).await.unwrap();

        fixture.sync(&["MLB1"]).await;
        assert_eq!(fixture.quality.panel(&listing).await.unwrap(), first);

        // The owner added the photos; the channel reworded the GTIN.
        fixture.clock.advance(TimeDelta::hours(1));
        let reworded = QualityAction {
            text: "Informe o GTIN para aparecer mais nas buscas.".into(),
            ..gtin()
        };
        fixture.channel.rates(
            "MLB1",
            rated(70, QualityLevel::Professional, vec![reworded.clone()]),
        );
        let report = fixture.sync(&["MLB1"]).await;

        assert_eq!(report.pending, 1);
        let panel = fixture.quality.panel(&listing).await.unwrap();
        assert_eq!(
            panel[0].rating,
            Rating::Rated(rated(70, QualityLevel::Professional, vec![reworded]))
        );
        assert_eq!(panel[0].checked_at, Some(fixture.clock.now()));
    });
}

#[test]
fn a_listing_the_channel_stops_rating_shows_as_not_rated_without_old_actions() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .rates("MLB1", rated(48, QualityLevel::Basic, vec![photos()]));
        fixture.sync(&["MLB1"]).await;

        fixture.channel.quality.lock().unwrap().clear();
        let report = fixture.sync(&["MLB1"]).await;

        assert_eq!(report.not_rated, 1);
        let panel = fixture
            .quality
            .panel(&[listed("MLB1", "Fone", 0)])
            .await
            .unwrap();
        assert_eq!(panel[0].rating, Rating::NotRated);
        assert!(panel[0].pending().is_empty());
    });
}

#[test]
fn variations_of_a_listing_are_asked_about_and_shown_once() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .rates("MLB1", rated(48, QualityLevel::Basic, vec![photos()]));

        let report = fixture.sync(&["MLB1", "MLB1", "MLB2"]).await;

        assert_eq!(report.checked, 2);
        assert_eq!(
            *fixture.channel.asked.lock().unwrap(),
            ids(&["MLB1", "MLB2"])
        );
        let panel = fixture
            .quality
            .panel(&[listed("MLB1", "Camiseta", 2), listed("MLB1", "Camiseta", 2)])
            .await
            .unwrap();
        assert_eq!(order_of(&panel), ["MLB1"]);
    });
}

#[test]
fn a_failed_sync_keeps_what_the_last_one_read() {
    block_on(async {
        let fixture = Fixture::new().await;
        fixture
            .channel
            .rates("MLB1", rated(48, QualityLevel::Basic, vec![photos()]));
        fixture.sync(&["MLB1"]).await;
        let listing = [listed("MLB1", "Fone", 0)];
        let before = fixture.quality.panel(&listing).await.unwrap();

        fixture
            .channel
            .rates("MLB1", rated(90, QualityLevel::Professional, Vec::new()));
        *fixture.channel.failing.lock().unwrap() =
            Some(("MLB2".into(), PlatformError::RateLimited));
        fixture.clock.advance(TimeDelta::minutes(5));
        let failed = fixture
            .quality
            .sync(&fixture.channel, &ids(&["MLB1", "MLB2"]))
            .await;

        assert!(failed.is_err());
        assert_eq!(fixture.quality.panel(&listing).await.unwrap(), before);
    });
}

#[test]
fn sales_count_toward_the_order_over_the_last_thirty_days() {
    block_on(async {
        let fixture = Fixture::new().await;

        assert_eq!(
            fixture.quality.impact_since(),
            fixture.clock.now() - TimeDelta::days(30)
        );
        assert_eq!(fixture.quality.last_sync().await.unwrap(), None);
    });
}
