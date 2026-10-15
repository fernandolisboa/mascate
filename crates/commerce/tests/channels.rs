//! The order of the Sales Channels, through commerce's public interface
//! with a real temporary database: Mercado Livre first until the owner says
//! otherwise, each channel connected later at the end, and a new order only
//! of the channels already ranked.

use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use futures::executor::block_on;
use mascate_commerce::{ChannelRanking, ListingError};
use mascate_kernel::SalesChannelKind::{Amazon, Magalu, MercadoLivre, Shopee};
use mascate_kernel::testing::{ManualClock, SequentialIds};
use mascate_platform::{Database, migrate};

struct Fixture {
    _dir: tempfile::TempDir,
    database: Arc<Database>,
    clock: Arc<ManualClock>,
    ids: Arc<SequentialIds>,
    ranking: ChannelRanking,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 15, 12, 0, 0).unwrap(),
        ));
        let database = Arc::new(
            Database::open(&dir.path().join("mascate.db"))
                .await
                .unwrap(),
        );
        migrate(&database, clock.as_ref(), &[mascate_commerce::MIGRATIONS])
            .await
            .unwrap();
        let ids = Arc::new(SequentialIds::default());
        let ranking = ChannelRanking::new(database.clone(), clock.clone(), ids.clone());
        Self {
            _dir: dir,
            database,
            clock,
            ids,
            ranking,
        }
    }
}

#[test]
fn mercado_livre_comes_first_and_a_channel_connected_later_goes_last() {
    block_on(async {
        let fx = Fixture::new().await;
        assert_eq!(fx.ranking.ranking().await.unwrap(), [MercadoLivre]);

        assert_eq!(
            fx.ranking.connected(Shopee).await.unwrap(),
            [MercadoLivre, Shopee]
        );
        fx.clock.advance(TimeDelta::minutes(1));
        assert_eq!(
            fx.ranking.connected(Amazon).await.unwrap(),
            [MercadoLivre, Shopee, Amazon]
        );
        // Seen connected again, a channel keeps its place.
        assert_eq!(
            fx.ranking.connected(Shopee).await.unwrap(),
            [MercadoLivre, Shopee, Amazon]
        );
        assert_eq!(
            fx.ranking.connected(MercadoLivre).await.unwrap(),
            [MercadoLivre, Shopee, Amazon]
        );
    });
}

#[test]
fn the_owners_order_is_saved_read_back_and_keeps_new_channels_last() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.ranking.connected(Amazon).await.unwrap();
        fx.ranking.connected(Shopee).await.unwrap();

        fx.clock.advance(TimeDelta::minutes(1));
        fx.ranking
            .rank(&[Amazon, MercadoLivre, Shopee])
            .await
            .unwrap();

        assert_eq!(
            fx.ranking.ranking().await.unwrap(),
            [Amazon, MercadoLivre, Shopee]
        );
        // Read again from the database, as after a restart.
        let reopened = ChannelRanking::new(fx.database.clone(), fx.clock.clone(), fx.ids.clone());
        assert_eq!(
            reopened.ranking().await.unwrap(),
            [Amazon, MercadoLivre, Shopee]
        );
        assert_eq!(
            reopened.connected(Magalu).await.unwrap(),
            [Amazon, MercadoLivre, Shopee, Magalu]
        );
    });
}

#[test]
fn a_new_order_holds_each_ranked_channel_once() {
    block_on(async {
        let fx = Fixture::new().await;
        fx.ranking.connected(Shopee).await.unwrap();

        for refused in [
            vec![],
            vec![MercadoLivre],
            vec![Shopee, Shopee],
            vec![MercadoLivre, Shopee, Shopee],
            vec![MercadoLivre, Amazon],
            vec![MercadoLivre, Shopee, Amazon],
        ] {
            assert!(
                matches!(
                    fx.ranking.rank(&refused).await,
                    Err(ListingError::NotTheRanking)
                ),
                "{refused:?}"
            );
        }
        assert_eq!(fx.ranking.ranking().await.unwrap(), [MercadoLivre, Shopee]);

        // With nothing connected yet, Mercado Livre alone is the order.
        let fresh = Fixture::new().await;
        fresh.ranking.rank(&[MercadoLivre]).await.unwrap();
        assert_eq!(fresh.ranking.ranking().await.unwrap(), [MercadoLivre]);
    });
}
