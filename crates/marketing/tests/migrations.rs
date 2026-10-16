//! A database from before Sales Channels in Marketing (v10) moved to this
//! version: every Listing Quality, question, Review and Product Ads row it
//! had stays as it was, on Mercado Livre, and the next Mercado Livre Sync
//! updates those rows instead of adding new ones (ADR 0033).

use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use futures::executor::block_on;
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_kernel::{Clock, Currency, Money, PlatformError, SalesChannelKind};
use mascate_marketing::{
    ActionKind, ChannelQuality, ListedItem, ListingQuality, QualityAction, QualityLevel,
    QualitySource, QuestionStatus, Questions, Rating, Reputation, Score,
};
use mascate_platform::{Database, ModuleMigrations, migrate};
use rust_decimal::Decimal;

/// The rows a Marketing v10 database held, in that version's columns.
const BEFORE_SALES_CHANNELS: &str = "
    INSERT INTO marketing_listing_quality
        (id, ml_item_id, score, level, visits, checked_at, created_at, updated_at)
        VALUES ('00000000-0000-7000-8000-000000000001', 'MLB1', 48, 'basic', 340,
                '2026-10-01T12:00:00+00:00', '2026-10-01T12:00:00+00:00',
                '2026-10-01T12:00:00+00:00');
    INSERT INTO marketing_listing_quality
        (id, ml_item_id, score, level, visits, checked_at, created_at, updated_at)
        VALUES ('00000000-0000-7000-8000-000000000002', 'MLB2', NULL, NULL, 0,
                '2026-10-01T12:00:00+00:00', '2026-10-01T12:00:00+00:00',
                '2026-10-01T12:00:00+00:00');
    INSERT INTO marketing_quality_actions
        (id, quality_id, ml_rule_key, kind, text, label, link, position, created_at, updated_at)
        VALUES ('00000000-0000-7000-8000-000000000003', '00000000-0000-7000-8000-000000000001',
                'PICTURES_QUANTITY_MIN', 'opportunity', 'Adicione mais fotos.', 'Corrigir', NULL,
                0, '2026-10-01T12:00:00+00:00', '2026-10-01T12:00:00+00:00');
    INSERT INTO marketing_questions
        (id, ml_question_id, ml_item_id, text, status, asked_at, created_at, updated_at)
        VALUES ('00000000-0000-7000-8000-000000000004', '13001', 'MLB1', 'Funciona com iPhone?',
                'unanswered', '2026-10-01T11:00:00+00:00', '2026-10-01T12:00:00+00:00',
                '2026-10-01T12:00:00+00:00');
    INSERT INTO marketing_listing_reviews
        (id, ml_item_id, one_star, two_stars, three_stars, four_stars, five_stars, checked_at,
         created_at, updated_at)
        VALUES ('00000000-0000-7000-8000-000000000005', 'MLB1', 0, 1, 0, 0, 2,
                '2026-10-01T12:00:00+00:00', '2026-10-01T12:00:00+00:00',
                '2026-10-01T12:00:00+00:00');
    INSERT INTO marketing_low_reviews
        (id, ml_review_id, ml_item_id, rating, title, text, reviewed_at, created_at, updated_at)
        VALUES ('00000000-0000-7000-8000-000000000006', '52001', 'MLB1', 2, 'Parou de carregar',
                '', '2026-10-01T10:00:00+00:00', '2026-10-01T12:00:00+00:00',
                '2026-10-01T12:00:00+00:00');
    INSERT INTO marketing_ad_campaigns
        (id, ml_campaign_id, name, status, created_at, updated_at)
        VALUES ('00000000-0000-7000-8000-000000000007', '1', 'Campanha', 'active',
                '2026-10-01T12:00:00+00:00', '2026-10-01T12:00:00+00:00');
    INSERT INTO marketing_ad_days
        (id, ml_item_id, day, ml_campaign_id, cost, attributed, currency, clicks, prints, units,
         created_at, updated_at)
        VALUES ('00000000-0000-7000-8000-000000000008', 'MLB1', '2026-10-01', '1', '2.50',
                '0', 'BRL', 3, 120, 0, '2026-10-01T12:00:00+00:00',
                '2026-10-01T12:00:00+00:00');";

/// Mercado Livre rating MLB1 as before.
struct MercadoLivre;

impl QualitySource for MercadoLivre {
    fn rated_channel(&self) -> SalesChannelKind {
        SalesChannelKind::MercadoLivre
    }

    fn quality(&self, _: &str) -> Result<Option<ChannelQuality>, PlatformError> {
        Ok(Some(before()))
    }

    fn visits(&self, _: &str, _: u32) -> Result<Option<u32>, PlatformError> {
        Ok(Some(340))
    }
}

fn before() -> ChannelQuality {
    ChannelQuality {
        score: Some(Score {
            points: 48,
            level: QualityLevel::Basic,
        }),
        pending: vec![QualityAction {
            key: "PICTURES_QUANTITY_MIN".into(),
            kind: ActionKind::Opportunity,
            text: "Adicione mais fotos.".into(),
            label: "Corrigir".into(),
            link: None,
        }],
    }
}

fn listed(listing: &str) -> ListedItem {
    ListedItem {
        channel: SalesChannelKind::MercadoLivre,
        listing: listing.into(),
        title: listing.into(),
        link: None,
        units_sold: 0,
    }
}

#[test]
fn every_row_from_before_sales_channels_stays_as_it_was_on_mercado_livre() {
    block_on(async {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 16, 12, 0, 0).unwrap(),
        ));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        let migrations = mascate_marketing::MIGRATIONS.migrations;
        let before_channels = migrations
            .iter()
            .position(|migration| migration.version == 11)
            .unwrap();
        let v10 = ModuleMigrations {
            module: "marketing",
            migrations: &migrations[..before_channels],
        };
        migrate(&database, clock.as_ref(), &[v10]).await.unwrap();
        database
            .connection()
            .execute_batch(BEFORE_SALES_CHANNELS)
            .await
            .unwrap();

        migrate(&database, clock.as_ref(), &[mascate_marketing::MIGRATIONS])
            .await
            .unwrap();

        let ids = Arc::new(SequentialIds::default());
        let quality = ListingQuality::new(database.clone(), clock.clone(), ids.clone());
        let panel = quality
            .panel(&[listed("MLB1"), listed("MLB2")])
            .await
            .unwrap();
        assert_eq!(panel[0].listing.listing, "MLB1");
        assert_eq!(panel[0].rating, Rating::Rated(before()));
        assert_eq!(panel[0].visits, Some(340));
        assert_eq!(panel[1].rating, Rating::NotRated);
        assert_eq!(panel[1].visits, Some(0));
        // The same listing on another channel is a different listing.
        let elsewhere = quality
            .panel(&[ListedItem {
                channel: SalesChannelKind::Shopee,
                ..listed("MLB1")
            }])
            .await
            .unwrap();
        assert_eq!(elsewhere[0].rating, Rating::NotChecked);

        let inbox = Questions::new(database.clone(), clock.clone(), ids.clone())
            .inbox()
            .await
            .unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].asked.id, "13001");
        assert_eq!(inbox[0].asked.listing, "MLB1");
        assert_eq!(inbox[0].asked.status, QuestionStatus::Unanswered);

        let reputation = Reputation::new(database.clone(), clock.clone(), ids.clone());
        let low = reputation.low_reviews().await.unwrap();
        assert_eq!(low.len(), 1);
        assert_eq!(low[0].listing, "MLB1");
        assert_eq!(low[0].review.id, "52001");

        let costs =
            mascate_marketing::ProductAds::new(database.clone(), clock.clone(), ids.clone())
                .costs()
                .await
                .unwrap();
        assert_eq!(costs.len(), 1);
        assert_eq!(costs[0].listing, "MLB1");
        assert_eq!(costs[0].day, NaiveDate::from_ymd_opt(2026, 10, 1).unwrap());
        assert_eq!(
            costs[0].cost,
            Money::new(Decimal::new(250, 2), Currency::Brl)
        );

        // Mercado Livre's next Sync finds the row it had.
        quality
            .sync(&MercadoLivre, &["MLB1".to_owned()])
            .await
            .unwrap();
        let panel = quality.panel(&[listed("MLB1")]).await.unwrap();
        assert_eq!(panel[0].rating, Rating::Rated(before()));
        assert_eq!(panel[0].checked_at, Some(clock.now()));
    });
}
