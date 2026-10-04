mod common;

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;

use futures::executor::block_on;
use mascate_catalog::{
    BestSeller, CatalogError, CatalogMatch, Competition, DemandCategory, DemandError, DemandSource,
    DiscoverySettings, ListingType, MATCHES_PER_OFFER, NewSupplierOffer, Opportunity,
    OpportunityFilter, score,
};
use mascate_kernel::{Currency, Margin, Money, Percentage};
use proptest::prelude::*;
use rust_decimal::Decimal;

use common::{Fixture, brl, new_offer};

const FONE_LINK: &str = "https://shopee.com.br/Fone-Bluetooth-TWS-i.123.456";
const LUMINARIA_LINK: &str = "https://shopee.com.br/Luminaria-LED-i.123.789";

fn decimal(text: &str) -> Decimal {
    Decimal::from_str(text).unwrap()
}

fn percent(text: &str) -> Percentage {
    Percentage::new(decimal(text)).unwrap()
}

fn category(id: &str, name: &str) -> DemandCategory {
    DemandCategory {
        id: id.into(),
        name: name.into(),
    }
}

fn eletronicos() -> DemandCategory {
    category("MLB1000", "Eletrônicos, Áudio e Vídeo")
}

fn casa() -> DemandCategory {
    category("MLB1574", "Casa, Móveis e Decoração")
}

/// A Sales Channel answering from memory, counting what it was asked.
#[derive(Default)]
struct FakeChannel {
    best_sellers: Mutex<HashMap<String, Vec<BestSeller>>>,
    searches: Mutex<HashMap<String, Vec<CatalogMatch>>>,
    competitions: Mutex<HashMap<String, Competition>>,
    roots: HashMap<String, DemandCategory>,
    fees: Mutex<HashMap<(String, String), Money>>,
    failure: Mutex<Option<DemandError>>,
    calls: Mutex<Vec<String>>,
}

impl FakeChannel {
    fn new() -> Self {
        Self {
            roots: HashMap::from([
                ("MLB3697".to_owned(), eletronicos()),
                ("MLB1000".to_owned(), eletronicos()),
                ("MLB5672".to_owned(), casa()),
            ]),
            ..Self::default()
        }
    }

    fn rank(&self, category: &str, best_sellers: Vec<BestSeller>) {
        self.best_sellers
            .lock()
            .unwrap()
            .insert(category.into(), best_sellers);
    }

    fn matches(&self, query: &str, found: &[(&str, &str)]) {
        self.searches.lock().unwrap().insert(
            query.into(),
            found
                .iter()
                .map(|(id, name)| CatalogMatch {
                    id: (*id).into(),
                    name: (*name).into(),
                })
                .collect(),
        );
    }

    fn compete(&self, product: &str, category: &str, sellers: u32, prices: &[&str]) {
        self.competitions.lock().unwrap().insert(
            product.into(),
            Competition {
                sellers,
                prices: prices.iter().map(|price| brl(price)).collect(),
                category: Some(category.into()),
            },
        );
    }

    fn charge(&self, category: &str, listing: ListingType, fee: &str) {
        self.fees
            .lock()
            .unwrap()
            .insert((category.into(), listing.code().into()), brl(fee));
    }

    fn fail_with(&self, error: DemandError) {
        *self.failure.lock().unwrap() = Some(error);
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn called(&self, call: String) -> Result<(), DemandError> {
        self.calls.lock().unwrap().push(call);
        match self.failure.lock().unwrap().clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl DemandSource for FakeChannel {
    fn currency(&self) -> Currency {
        Currency::Brl
    }

    fn categories(&self) -> Result<Vec<DemandCategory>, DemandError> {
        self.called("categories".into())?;
        Ok(vec![eletronicos(), casa()])
    }

    fn best_sellers(&self, category: &str) -> Result<Vec<BestSeller>, DemandError> {
        self.called(format!("best_sellers {category}"))?;
        Ok(self
            .best_sellers
            .lock()
            .unwrap()
            .get(category)
            .cloned()
            .unwrap_or_default())
    }

    fn search(&self, query: &str) -> Result<Vec<CatalogMatch>, DemandError> {
        self.called(format!("search {query}"))?;
        Ok(self
            .searches
            .lock()
            .unwrap()
            .get(query)
            .cloned()
            .unwrap_or_default())
    }

    fn competition(&self, product: &str) -> Result<Competition, DemandError> {
        self.called(format!("competition {product}"))?;
        self.competitions
            .lock()
            .unwrap()
            .get(product)
            .cloned()
            .ok_or(DemandError::NotFound)
    }

    fn root_category(&self, category: &str) -> Result<DemandCategory, DemandError> {
        self.called(format!("root {category}"))?;
        self.roots
            .get(category)
            .cloned()
            .ok_or(DemandError::NotFound)
    }

    fn sale_fee(
        &self,
        category: &str,
        price: Money,
        listing: ListingType,
    ) -> Result<Money, DemandError> {
        self.called(format!(
            "fee {category} {} {}",
            price.amount(),
            listing.code()
        ))?;
        self.fees
            .lock()
            .unwrap()
            .get(&(category.to_owned(), listing.code().to_owned()))
            .copied()
            .ok_or(DemandError::NotFound)
    }
}

fn best_seller(id: &str, position: u32, product: Option<&str>) -> BestSeller {
    BestSeller {
        id: id.into(),
        position,
        title: format!("Mais vendido {position}"),
        catalog_product: product.map(Into::into),
        price: Some(brl("99.90")),
    }
}

/// An earbud offer at R$ 30 + R$ 5 shipping that matches one catalog
/// product sold by three listings at R$ 90, R$ 100 and R$ 110, with a R$ 14
/// Clássico fee.
async fn fone(f: &Fixture, channel: &FakeChannel) {
    let shop = f.supplier("Loja Fones").await;
    f.catalog
        .register_offer(new_offer(&shop, FONE_LINK, "Fone Bluetooth TWS", "30"))
        .await
        .unwrap();
    channel.matches(
        "Fone Bluetooth TWS",
        &[("MLB111", "Fone de Ouvido TWS Pro")],
    );
    channel.compete("MLB111", "MLB3697", 3, &["90", "100", "110"]);
    channel.charge("MLB3697", ListingType::Classic, "14");
}

async fn listed(f: &Fixture, tax: Percentage) -> Vec<Opportunity> {
    f.catalog
        .opportunities(&OpportunityFilter::default(), tax)
        .await
        .unwrap()
}

#[test]
fn an_opportunity_shows_its_estimated_margin_after_fee_shipping_tax_and_cost() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = FakeChannel::new();
        fone(&f, &channel).await;

        let report = f.catalog.sync_demand(&channel).await.unwrap();
        let listed = listed(&f, percent("6")).await;

        assert_eq!(report.offers, 1);
        assert_eq!(report.opportunities, 1);
        let [opportunity] = &listed[..] else {
            panic!("one opportunity: {listed:?}");
        };
        assert_eq!(opportunity.product_id, "MLB111");
        assert_eq!(opportunity.product_name, "Fone de Ouvido TWS Pro");
        assert_eq!(opportunity.category, Some(eletronicos()));
        assert_eq!(opportunity.competitors, 3);
        assert_eq!(opportunity.sale_price, brl("100"));
        assert_eq!(opportunity.lowest_price, brl("90"));
        assert_eq!(opportunity.highest_price, brl("110"));
        assert_eq!(opportunity.sale_fee.amount, brl("14"));
        assert!(!opportunity.sale_fee.estimated);
        assert_eq!(opportunity.shipping.amount, brl("20"));
        assert!(opportunity.shipping.estimated);
        assert_eq!(opportunity.tax, brl("6"));
        assert_eq!(opportunity.cost, brl("35"));
        assert_eq!(opportunity.margin.amount, brl("25"));
        assert_eq!(opportunity.margin.percent, Some(decimal("25")));
        assert_eq!(opportunity.offer.link, FONE_LINK);
        // 25 points of margin, no best seller, 3 competitors.
        assert_eq!(opportunity.score, decimal("22"));
        assert!(
            channel
                .calls()
                .contains(&"fee MLB3697 100 gold_special".to_owned())
        );
    });
}

#[test]
fn without_the_channels_fee_the_configured_rate_is_used_and_marked_estimated() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = FakeChannel::new();
        fone(&f, &channel).await;
        channel.fees.lock().unwrap().clear();

        f.catalog.sync_demand(&channel).await.unwrap();
        let opportunity = listed(&f, Percentage::ZERO).await.remove(0);

        assert_eq!(opportunity.sale_fee.amount, brl("14"));
        assert!(opportunity.sale_fee.estimated);

        f.catalog
            .save_discovery_settings(DiscoverySettings {
                estimated_fee: percent("11.5"),
                ..DiscoverySettings::default()
            })
            .await
            .unwrap();
        let opportunity = listed(&f, Percentage::ZERO).await.remove(0);
        assert_eq!(opportunity.sale_fee.amount, brl("11.5"));
        assert_eq!(opportunity.margin.amount, brl("33.5"));
    });
}

#[test]
fn a_fee_read_for_another_listing_type_gives_way_to_the_estimate_until_the_next_sync() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = FakeChannel::new();
        fone(&f, &channel).await;
        channel.charge("MLB3697", ListingType::Premium, "18");
        f.catalog.sync_demand(&channel).await.unwrap();

        let premium = DiscoverySettings {
            listing_type: ListingType::Premium,
            ..DiscoverySettings::default()
        };
        f.catalog.save_discovery_settings(premium).await.unwrap();
        let before = listed(&f, Percentage::ZERO).await.remove(0);
        f.catalog.sync_demand(&channel).await.unwrap();
        let after = listed(&f, Percentage::ZERO).await.remove(0);

        assert!(before.sale_fee.estimated);
        assert_eq!(after.sale_fee.amount, brl("18"));
        assert!(!after.sale_fee.estimated);
    });
}

#[test]
fn below_the_free_shipping_price_the_seller_pays_no_shipping() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = FakeChannel::new();
        fone(&f, &channel).await;
        channel.compete("MLB111", "MLB3697", 2, &["59.90", "69.90"]);

        f.catalog.sync_demand(&channel).await.unwrap();
        let opportunity = listed(&f, Percentage::ZERO).await.remove(0);

        assert_eq!(opportunity.sale_price, brl("64.90"));
        assert_eq!(opportunity.shipping.amount, brl("0"));
        assert_eq!(opportunity.margin.amount, brl("15.90"));
    });
}

#[test]
fn a_new_price_at_the_offers_link_changes_the_margin_without_a_sync() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = FakeChannel::new();
        fone(&f, &channel).await;
        f.catalog.sync_demand(&channel).await.unwrap();
        f.a_day_later();
        let shop = f.catalog.suppliers().await.unwrap().remove(0);

        f.catalog
            .register_offer(new_offer(&shop, FONE_LINK, "Fone Bluetooth TWS", "40"))
            .await
            .unwrap();
        let opportunity = listed(&f, Percentage::ZERO).await.remove(0);

        assert_eq!(opportunity.cost, brl("45"));
        assert_eq!(opportunity.margin.amount, brl("21"));
    });
}

#[test]
fn best_sellers_of_followed_categories_are_synced_and_raise_the_score() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = FakeChannel::new();
        fone(&f, &channel).await;
        f.catalog
            .follow_categories(&[eletronicos(), casa()])
            .await
            .unwrap();
        channel.rank(
            "MLB1000",
            vec![
                best_seller("MLBU999", 1, None),
                best_seller("MLB111", 2, Some("MLB111")),
            ],
        );

        let report = f.catalog.sync_demand(&channel).await.unwrap();
        let ranked = f.catalog.best_sellers().await.unwrap();
        let opportunity = listed(&f, percent("6")).await.remove(0);

        assert_eq!(report.best_sellers, 2);
        let categories: Vec<_> = ranked.iter().map(|r| r.category.clone()).collect();
        assert_eq!(categories, [casa(), eletronicos()]);
        assert!(ranked[0].best_sellers.is_empty());
        let electronics = &ranked[1].best_sellers;
        assert_eq!(electronics[0].best_seller.id, "MLBU999");
        assert!(!electronics[0].has_opportunity);
        assert_eq!(electronics[1].best_seller.price, Some(brl("99.90")));
        assert!(electronics[1].has_opportunity);
        assert_eq!(opportunity.best_seller, Some(2));
        // 25 points of margin + 19 for second place - 3 competitors.
        assert_eq!(opportunity.score, decimal("41"));
        assert!(f.catalog.last_demand_sync().await.unwrap().is_some());
    });
}

#[test]
fn a_category_no_longer_followed_stops_syncing_and_showing() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = FakeChannel::new();
        f.catalog
            .follow_categories(&[eletronicos(), casa()])
            .await
            .unwrap();
        channel.rank("MLB1574", vec![best_seller("MLB222", 1, Some("MLB222"))]);
        f.catalog.sync_demand(&channel).await.unwrap();

        f.catalog.follow_categories(&[eletronicos()]).await.unwrap();
        channel.calls.lock().unwrap().clear();
        f.catalog.sync_demand(&channel).await.unwrap();

        assert_eq!(
            f.catalog.demand_categories().await.unwrap(),
            [eletronicos()]
        );
        let ranked = f.catalog.best_sellers().await.unwrap();
        assert_eq!(ranked.len(), 1);
        assert_eq!(channel.calls(), ["best_sellers MLB1000"]);
    });
}

#[test]
fn syncing_again_replaces_the_ranking_and_keeps_the_same_records() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = FakeChannel::new();
        fone(&f, &channel).await;
        f.catalog.follow_categories(&[eletronicos()]).await.unwrap();
        channel.rank(
            "MLB1000",
            vec![
                best_seller("MLB111", 1, Some("MLB111")),
                best_seller("MLB333", 2, Some("MLB333")),
            ],
        );
        f.catalog.sync_demand(&channel).await.unwrap();
        let first = listed(&f, Percentage::ZERO).await;

        f.catalog.sync_demand(&channel).await.unwrap();
        assert_eq!(listed(&f, Percentage::ZERO).await, first);

        channel.rank("MLB1000", vec![best_seller("MLB333", 1, Some("MLB333"))]);
        f.catalog.sync_demand(&channel).await.unwrap();
        let ids: Vec<_> = f.catalog.best_sellers().await.unwrap()[0]
            .best_sellers
            .iter()
            .map(|ranked| (ranked.best_seller.id.clone(), ranked.best_seller.position))
            .collect();
        assert_eq!(ids, [("MLB333".to_owned(), 1)]);
        let opportunity = listed(&f, Percentage::ZERO).await.remove(0);
        assert_eq!(opportunity.id, first[0].id);
        assert_eq!(opportunity.best_seller, None);
    });
}

#[test]
fn each_offer_is_compared_with_the_best_few_matches_and_drops_the_ones_no_longer_found() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = FakeChannel::new();
        fone(&f, &channel).await;
        let found = [
            ("MLB111", "Fone A"),
            ("MLB112", "Fone B"),
            ("MLB113", "Fone C"),
            ("MLB114", "Fone D"),
        ];
        channel.matches("Fone Bluetooth TWS", &found);
        for (product, _) in found {
            channel.compete(product, "MLB3697", 1, &["100"]);
        }

        f.catalog.sync_demand(&channel).await.unwrap();
        assert_eq!(listed(&f, Percentage::ZERO).await.len(), MATCHES_PER_OFFER);
        assert!(!channel.calls().contains(&"competition MLB114".to_owned()));

        channel.matches("Fone Bluetooth TWS", &found[1..2]);
        f.catalog.sync_demand(&channel).await.unwrap();
        let names: Vec<_> = listed(&f, Percentage::ZERO)
            .await
            .into_iter()
            .map(|opportunity| opportunity.product_name)
            .collect();
        assert_eq!(names, ["Fone B"]);
    });
}

#[test]
fn matches_without_a_price_or_unknown_to_the_channel_make_no_opportunity() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = FakeChannel::new();
        fone(&f, &channel).await;
        channel.matches(
            "Fone Bluetooth TWS",
            &[("MLB404", "Sumiu"), ("MLB000", "Sem preço")],
        );
        channel.compete("MLB000", "MLB3697", 0, &[]);

        let report = f.catalog.sync_demand(&channel).await.unwrap();

        assert_eq!(report.unmatched, 1);
        assert_eq!(report.opportunities, 0);
        assert!(listed(&f, Percentage::ZERO).await.is_empty());
    });
}

#[test]
fn offers_priced_in_another_currency_are_left_out_and_counted() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = FakeChannel::new();
        let shop = f.supplier("AliExpress").await;
        f.catalog
            .register_offer(NewSupplierOffer {
                price: Money::new(decimal("5"), Currency::Usd),
                shipping: Money::new(decimal("1"), Currency::Usd),
                ..new_offer(&shop, FONE_LINK, "Fone Bluetooth TWS", "1")
            })
            .await
            .unwrap();

        let report = f.catalog.sync_demand(&channel).await.unwrap();

        assert_eq!(report.other_currency, 1);
        assert!(channel.calls().is_empty());
    });
}

#[test]
fn a_discarded_opportunity_needs_a_reason_and_never_comes_back() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = FakeChannel::new();
        fone(&f, &channel).await;
        f.catalog.sync_demand(&channel).await.unwrap();
        let opportunity = listed(&f, Percentage::ZERO).await.remove(0);

        assert!(matches!(
            f.catalog.dismiss_opportunity(opportunity.id, "  ").await,
            Err(CatalogError::MissingReason)
        ));
        f.catalog
            .dismiss_opportunity(opportunity.id, "Produto diferente do anúncio")
            .await
            .unwrap();
        f.catalog.sync_demand(&channel).await.unwrap();

        assert!(listed(&f, Percentage::ZERO).await.is_empty());
        assert_eq!(f.catalog.dismissed_opportunities().await.unwrap(), 1);
        assert!(matches!(
            f.catalog
                .dismiss_opportunity(opportunity.id, "de novo")
                .await,
            Err(CatalogError::UnknownOpportunity(_))
        ));
    });
}

#[test]
fn filters_by_minimum_margin_category_and_price_range_and_ranks_by_score() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = FakeChannel::new();
        fone(&f, &channel).await;
        let shop = f.catalog.suppliers().await.unwrap().remove(0);
        f.catalog
            .register_offer(new_offer(&shop, LUMINARIA_LINK, "Luminária LED", "10"))
            .await
            .unwrap();
        channel.matches("Luminária LED", &[("MLB222", "Luminária de Mesa LED")]);
        channel.compete("MLB222", "MLB5672", 1, &["60"]);
        channel.charge("MLB5672", ListingType::Classic, "9");
        f.catalog.sync_demand(&channel).await.unwrap();
        let names = |listed: Vec<Opportunity>| {
            listed
                .into_iter()
                .map(|opportunity| opportunity.product_name)
                .collect::<Vec<_>>()
        };
        let filtered = |filter: OpportunityFilter| {
            let catalog = &f.catalog;
            async move {
                names(
                    catalog
                        .opportunities(&filter, Percentage::ZERO)
                        .await
                        .unwrap(),
                )
            }
        };

        // Luminária: 60 - 9 - 15 = 36 (60%), score 59; fone: 31%, score 28.
        assert_eq!(
            filtered(OpportunityFilter::default()).await,
            ["Luminária de Mesa LED", "Fone de Ouvido TWS Pro"]
        );
        assert_eq!(
            filtered(OpportunityFilter {
                min_margin: Some(decimal("40")),
                ..OpportunityFilter::default()
            })
            .await,
            ["Luminária de Mesa LED"]
        );
        assert_eq!(
            filtered(OpportunityFilter {
                category: Some("MLB1000".into()),
                ..OpportunityFilter::default()
            })
            .await,
            ["Fone de Ouvido TWS Pro"]
        );
        assert_eq!(
            filtered(OpportunityFilter {
                min_price: Some(decimal("70")),
                max_price: Some(decimal("100")),
                ..OpportunityFilter::default()
            })
            .await,
            ["Fone de Ouvido TWS Pro"]
        );
        assert!(
            filtered(OpportunityFilter {
                max_price: Some(decimal("59.99")),
                ..OpportunityFilter::default()
            })
            .await
            .is_empty()
        );
    });
}

#[test]
fn an_opportunity_becomes_a_product_through_its_offer() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = FakeChannel::new();
        fone(&f, &channel).await;
        f.catalog.sync_demand(&channel).await.unwrap();
        let opportunity = listed(&f, Percentage::ZERO).await.remove(0);

        let sku = f
            .catalog
            .suggest_sku(&opportunity.product_name)
            .await
            .unwrap();
        let product = f
            .catalog
            .create_product(
                opportunity.offer.id,
                &opportunity.product_name,
                sku.as_str(),
            )
            .await
            .unwrap();
        let after = listed(&f, Percentage::ZERO).await.remove(0);

        assert_eq!(after.offer.product, Some(product.id));
        assert_eq!(product.name, "Fone de Ouvido TWS Pro");
    });
}

#[test]
fn a_sync_stops_when_the_channel_no_longer_accepts_the_login() {
    block_on(async {
        let f = Fixture::new().await;
        let channel = FakeChannel::new();
        fone(&f, &channel).await;
        f.catalog.follow_categories(&[eletronicos()]).await.unwrap();
        channel.fail_with(DemandError::Expired);

        assert!(matches!(
            f.catalog.sync_demand(&channel).await,
            Err(CatalogError::Demand(DemandError::Expired))
        ));
        assert_eq!(channel.calls(), ["best_sellers MLB1000"]);
    });
}

#[test]
fn discovery_settings_start_from_defaults_and_keep_what_the_owner_saves() {
    block_on(async {
        let f = Fixture::new().await;
        let defaults = f.catalog.discovery_settings().await.unwrap();
        assert_eq!(defaults.listing_type, ListingType::Classic);
        assert_eq!(defaults.estimated_fee, percent("14"));
        assert_eq!(defaults.estimated_shipping, brl("20"));

        let saved = DiscoverySettings {
            listing_type: ListingType::Premium,
            estimated_fee: percent("17.5"),
            estimated_shipping: brl("24.90"),
        };
        f.catalog.save_discovery_settings(saved).await.unwrap();
        assert_eq!(f.catalog.discovery_settings().await.unwrap(), saved);
        assert!(matches!(
            f.catalog
                .save_discovery_settings(DiscoverySettings {
                    estimated_shipping: brl("-1"),
                    ..saved
                })
                .await,
            Err(CatalogError::Negative)
        ));
    });
}

fn margin_of(percent: i64) -> Margin {
    Margin::of(brl("100"), &[brl(&(100 - percent).to_string())]).unwrap()
}

proptest! {
    #[test]
    fn a_higher_margin_always_scores_higher(
        low in -50i64..100,
        extra in 1i64..50,
        position in prop::option::of(1u32..=20),
        competitors in 0u32..100,
    ) {
        prop_assert!(
            score(&margin_of(low + extra), position, competitors)
                > score(&margin_of(low), position, competitors)
        );
    }

    #[test]
    fn a_better_place_among_best_sellers_never_scores_lower(
        margin in -50i64..100,
        position in 1u32..=20,
        competitors in 0u32..100,
    ) {
        let margin = margin_of(margin);
        let better = score(&margin, Some(position.saturating_sub(1).max(1)), competitors);
        prop_assert!(better >= score(&margin, Some(position), competitors));
        prop_assert!(score(&margin, Some(position), competitors) > score(&margin, None, competitors));
    }

    #[test]
    fn more_competitors_never_score_higher(
        margin in -50i64..100,
        competitors in 0u32..100,
        more in 0u32..100,
    ) {
        let margin = margin_of(margin);
        prop_assert!(score(&margin, None, competitors + more) <= score(&margin, None, competitors));
    }
}
