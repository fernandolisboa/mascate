//! The owner's Promotions through Mercado Livre's adapter, against the
//! fake server serving the answers in `fixtures/mercado-livre`, written
//! from the documentation (see its README). What commerce does with them
//! is tested there.

use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use mascate_commerce::{
    ChannelOffer, ChannelPromotion, ChannelPromotions, CouponDiscount, CouponTerms, OfferToJoin,
    PromotionKind, PromotionPlan, PromotionStatus,
};
use mascate_integrations::MercadoLivre;
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{Currency, Money, Percentage, PlatformError};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};
use rust_decimal::Decimal;
use serde_json::json;

const SELLER: &str = "/seller-promotions/users/1234567?app_version=v2";
const SELLER_NEXT: &str = "/seller-promotions/users/1234567?app_version=v2&offset=4&limit=50";
const COUPON: &str =
    "/seller-promotions/promotions/C-MLB1081?promotion_type=SELLER_COUPON_CAMPAIGN&app_version=v2";
const ITEM: &str = "/seller-promotions/items/MLB4100000001?app_version=v2";
const CREATE: &str = "/seller-promotions/promotions?app_version=v2";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/mercado-livre/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn brl(amount: &str) -> Money {
    Money::new(amount.parse::<Decimal>().unwrap(), Currency::Brl)
}

fn on(month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, month, day).unwrap()
}

struct Fake {
    server: FakeHttpServer,
    adapter: MercadoLivre,
}

impl Fake {
    fn connected() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        for (name, value) in [
            ("ML_CLIENT_ID", "1234567890"),
            ("ML_CLIENT_SECRET", "s3cr3t"),
            ("ML_REFRESH_TOKEN", "TG-first"),
        ] {
            store.write(name, &Secret::new(value)).unwrap();
        }
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 5, 15, 0, 0).unwrap(),
        ));
        let adapter = MercadoLivre::new(server.url(), "Mascate/test", store, clock);
        server.serve("/oauth/token", 200, fixture("token.json"));
        server.serve("/users/me", 200, fixture("users-me.json"));
        Self { server, adapter }
    }

    /// The bodies sent with `method` to paths starting with `path`, as JSON.
    fn sent(&self, method: &str, path: &str) -> Vec<serde_json::Value> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.method == method && request.path.starts_with(path))
            .map(|request| serde_json::from_str(&request.body).unwrap())
            .collect()
    }

    fn asked(&self, method: &str) -> Vec<String> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.method == method)
            .map(|request| request.path)
            .collect()
    }
}

fn campaign(name: &str) -> PromotionPlan {
    PromotionPlan {
        kind: PromotionKind::SellerCampaign,
        name: name.into(),
        starts: on(10, 12),
        ends: on(10, 18),
        coupon: None,
    }
}

#[test]
fn the_sellers_campaigns_and_coupons_come_from_every_page_with_each_coupons_terms() {
    let fake = Fake::connected();
    fake.server
        .serve(SELLER, 200, fixture("seller-promotions-user.json"));
    fake.server
        .serve(SELLER_NEXT, 200, fixture("seller-promotions-user-end.json"));
    fake.server
        .serve(COUPON, 200, fixture("seller-promotion-C-MLB1081.json"));

    let promotions = fake.adapter.promotions().unwrap();

    assert_eq!(
        promotions,
        [
            ChannelPromotion {
                id: "C-MLB360923".into(),
                kind: PromotionKind::SellerCampaign,
                name: "Semana do fone".into(),
                starts: on(10, 1),
                ends: on(10, 10),
                status: PromotionStatus::Started,
                coupon: None,
            },
            ChannelPromotion {
                id: "C-MLB1081".into(),
                kind: PromotionKind::Coupon,
                name: "Cupom de outubro".into(),
                starts: on(10, 10),
                ends: on(10, 31),
                status: PromotionStatus::Pending,
                coupon: Some(CouponTerms {
                    discount: CouponDiscount::Amount(brl("20")),
                    minimum_purchase: brl("100"),
                    maximum_discount: None,
                    budget: brl("1000"),
                }),
            },
            ChannelPromotion {
                id: "C-MLB1090".into(),
                kind: PromotionKind::Coupon,
                name: "Dez por cento".into(),
                starts: on(10, 4),
                ends: on(10, 18),
                status: PromotionStatus::Started,
                coupon: Some(CouponTerms {
                    discount: CouponDiscount::Percent(Percentage::new(10.into()).unwrap()),
                    minimum_purchase: brl("50"),
                    maximum_discount: Some(brl("15")),
                    budget: brl("500"),
                }),
            },
        ]
    );
    // The coupon with its terms in the list is not read again.
    assert!(
        !fake
            .server
            .requests()
            .iter()
            .any(|path| path.contains("C-MLB1090"))
    );
}

#[test]
fn a_listings_promotions_are_the_ones_running_or_about_to_with_their_prices() {
    let fake = Fake::connected();
    fake.server
        .serve(ITEM, 200, fixture("seller-promotions-item-MLB4100000001.json"));

    let offers = fake.adapter.offers("MLB4100000001").unwrap();

    let offer = |kind, promotion: Option<&str>, name: &str, status, price: Option<&str>| {
        ChannelOffer {
            listing: "MLB4100000001".into(),
            kind,
            promotion: promotion.map(str::to_owned),
            name: name.into(),
            status,
            price: price.map(brl),
            starts: None,
            ends: None,
        }
    };
    let days = |offer: ChannelOffer, starts, ends| ChannelOffer {
        starts: Some(starts),
        ends: Some(ends),
        ..offer
    };
    assert_eq!(
        offers,
        [
            days(
                offer(
                    PromotionKind::PriceDiscount,
                    None,
                    "",
                    PromotionStatus::Started,
                    Some("79.9")
                ),
                on(10, 4),
                on(10, 10)
            ),
            days(
                offer(
                    PromotionKind::SellerCampaign,
                    Some("C-MLB360923"),
                    "Semana do fone",
                    PromotionStatus::Pending,
                    Some("84.9")
                ),
                on(10, 12),
                on(10, 18)
            ),
            days(
                offer(
                    PromotionKind::Coupon,
                    Some("C-MLB1081"),
                    "Cupom de outubro",
                    PromotionStatus::Pending,
                    None
                ),
                on(10, 10),
                on(10, 31)
            ),
            days(
                offer(
                    PromotionKind::ChannelCampaign,
                    Some("P-MLB1806020"),
                    "Oferta relâmpago",
                    PromotionStatus::Started,
                    Some("74.9")
                ),
                on(10, 5),
                on(10, 5)
            ),
        ]
    );
}

#[test]
fn a_listing_in_no_promotion_may_answer_not_found() {
    let fake = Fake::connected();
    fake.server.serve(
        "/seller-promotions/items/MLB4100000009?app_version=v2",
        404,
        fixture("error-404-seller-promotions.json"),
    );

    assert_eq!(fake.adapter.offers("MLB4100000009"), Ok(Vec::new()));
}

#[test]
fn a_campaign_goes_with_its_days_from_midnight_and_comes_back_with_its_id() {
    let fake = Fake::connected();
    fake.server
        .serve_method("POST", CREATE, 200, fixture("seller-promotion-created.json"));

    let created = fake.adapter.create(&campaign("Semana do \"fone\"")).unwrap();

    assert_eq!(
        fake.sent("POST", CREATE),
        [json!({
            "promotion_type": "SELLER_CAMPAIGN",
            "name": "Semana do \"fone\"",
            "start_date": "2026-10-12T00:00:00",
            "finish_date": "2026-10-18T00:00:00",
            "sub_type": "FLEXIBLE_PERCENTAGE",
        })]
    );
    assert_eq!(
        created,
        ChannelPromotion {
            id: "C-MLB360950".into(),
            kind: PromotionKind::SellerCampaign,
            name: "Semana do fone".into(),
            starts: on(10, 12),
            ends: on(10, 18),
            status: PromotionStatus::Pending,
            coupon: None,
        }
    );
}

#[test]
fn a_coupon_goes_with_its_terms_in_reais() {
    let fake = Fake::connected();
    fake.server
        .serve_method("POST", CREATE, 200, fixture("seller-promotion-created.json"));
    let amount = CouponTerms {
        discount: CouponDiscount::Amount(brl("19.999")),
        minimum_purchase: brl("100"),
        maximum_discount: None,
        budget: brl("1000"),
    };
    let share = CouponTerms {
        discount: CouponDiscount::Percent(Percentage::new(Decimal::new(125, 1)).unwrap()),
        maximum_discount: Some(brl("15")),
        ..amount
    };

    for terms in [amount, share] {
        fake.adapter
            .create(&PromotionPlan {
                kind: PromotionKind::Coupon,
                coupon: Some(terms),
                ..campaign("Cupom")
            })
            .unwrap();
    }

    let sent = fake.sent("POST", CREATE);
    assert_eq!(sent[0]["promotion_type"], "SELLER_COUPON_CAMPAIGN");
    assert_eq!(sent[0]["sub_type"], "FIXED_AMOUNT");
    assert_eq!(sent[0]["fixed_amount"].as_f64(), Some(20.0));
    assert_eq!(sent[0]["min_purchase_amount"].as_f64(), Some(100.0));
    assert_eq!(sent[0]["budget"].as_f64(), Some(1000.0));
    assert!(sent[0].get("max_purchase_amount").is_none());
    assert_eq!(sent[1]["sub_type"], "FIXED_PERCENTAGE");
    assert_eq!(sent[1]["fixed_percentage"].as_f64(), Some(12.5));
    assert_eq!(sent[1]["max_purchase_amount"].as_f64(), Some(15.0));
}

#[test]
fn a_listing_joins_with_the_price_and_days_its_promotion_takes() {
    let fake = Fake::connected();
    fake.server.serve_method(
        "POST",
        ITEM,
        200,
        fixture("seller-promotion-item-joined.json"),
    );

    fake.adapter
        .join(
            "MLB4100000001",
            &OfferToJoin {
                kind: PromotionKind::PriceDiscount,
                promotion: None,
                price: Some(brl("79.9")),
                days: Some((on(10, 5), on(10, 11))),
            },
        )
        .unwrap();
    fake.adapter
        .join(
            "MLB4100000001",
            &OfferToJoin {
                kind: PromotionKind::SellerCampaign,
                promotion: Some("C-MLB360923".into()),
                price: Some(brl("84.9")),
                days: None,
            },
        )
        .unwrap();
    fake.adapter
        .join(
            "MLB4100000001",
            &OfferToJoin {
                kind: PromotionKind::Coupon,
                promotion: Some("C-MLB1081".into()),
                price: None,
                days: None,
            },
        )
        .unwrap();

    assert_eq!(
        fake.sent("POST", "/seller-promotions/items/"),
        [
            json!({
                "promotion_type": "PRICE_DISCOUNT",
                "deal_price": 79.9,
                "start_date": "2026-10-05T00:00:00",
                "finish_date": "2026-10-11T00:00:00",
            }),
            json!({
                "promotion_type": "SELLER_CAMPAIGN",
                "promotion_id": "C-MLB360923",
                "deal_price": 84.9,
            }),
            json!({
                "promotion_type": "SELLER_COUPON_CAMPAIGN",
                "promotion_id": "C-MLB1081",
            }),
        ]
    );
}

#[test]
fn changing_ending_and_leaving_name_the_promotion_type() {
    let fake = Fake::connected();
    let campaign_path = "/seller-promotions/promotions/C-MLB360923";
    fake.server.serve_method(
        "PUT",
        &format!("{campaign_path}?app_version=v2"),
        200,
        fixture("seller-promotion-created.json"),
    );
    fake.server.serve_method(
        "DELETE",
        &format!("{campaign_path}?promotion_type=SELLER_CAMPAIGN&app_version=v2"),
        200,
        "",
    );
    fake.server.serve_method(
        "DELETE",
        "/seller-promotions/items/MLB4100000001?promotion_type=SELLER_COUPON_CAMPAIGN&promotion_id=C-MLB1081&app_version=v2",
        200,
        "",
    );
    fake.server.serve_method(
        "DELETE",
        "/seller-promotions/items/MLB4100000001?promotion_type=PRICE_DISCOUNT&app_version=v2",
        200,
        "",
    );

    fake.adapter
        .change("C-MLB360923", &campaign("Quinzena do fone"))
        .unwrap();
    fake.adapter
        .end("C-MLB360923", PromotionKind::SellerCampaign)
        .unwrap();
    fake.adapter
        .leave("MLB4100000001", PromotionKind::Coupon, Some("C-MLB1081"))
        .unwrap();
    fake.adapter
        .leave("MLB4100000001", PromotionKind::PriceDiscount, None)
        .unwrap();

    assert_eq!(
        fake.sent("PUT", campaign_path),
        [json!({
            "promotion_type": "SELLER_CAMPAIGN",
            "name": "Quinzena do fone",
            "start_date": "2026-10-12T00:00:00",
            "finish_date": "2026-10-18T00:00:00",
        })]
    );
    assert_eq!(fake.asked("DELETE").len(), 3);
}

#[test]
fn a_promotion_mercado_livre_refuses_says_why() {
    let fake = Fake::connected();
    fake.server
        .serve_method("POST", ITEM, 400, fixture("error-400-promotion.json"));

    let refused = fake.adapter.join(
        "MLB4100000001",
        &OfferToJoin {
            kind: PromotionKind::PriceDiscount,
            promotion: None,
            price: Some(brl("10")),
            days: Some((on(10, 5), on(10, 11))),
        },
    );

    assert_eq!(
        refused,
        Err(PlatformError::Refused(
            "The discounted price is not credible.".into()
        ))
    );
}

#[test]
fn a_rate_limited_or_unknown_promotion_is_not_taken_as_done() {
    let fake = Fake::connected();
    fake.server
        .serve_method("POST", CREATE, 429, fixture("error-429.json"));
    fake.server.serve_method(
        "DELETE",
        "/seller-promotions/promotions/C-MLB9?promotion_type=SELLER_CAMPAIGN&app_version=v2",
        404,
        fixture("error-404-seller-promotions.json"),
    );

    assert_eq!(
        fake.adapter.create(&campaign("Semana do fone")),
        Err(PlatformError::RateLimited)
    );
    assert_eq!(
        fake.adapter.end("C-MLB9", PromotionKind::SellerCampaign),
        Err(PlatformError::NotFound)
    );
}

#[test]
fn a_promotion_turned_down_for_an_old_token_goes_once_more_with_a_renewed_one() {
    let fake = Fake::connected();
    fake.server
        .serve_method("POST", CREATE, 401, r#"{"message":"invalid_token"}"#);
    fake.server.then_serve(
        &format!("POST {CREATE}"),
        200,
        fixture("seller-promotion-created.json"),
    );

    fake.adapter.create(&campaign("Semana do fone")).unwrap();

    assert_eq!(fake.sent("POST", CREATE).len(), 2);
}

#[test]
fn an_id_that_is_not_mercado_livres_never_reaches_a_path_or_a_body() {
    let fake = Fake::connected();

    assert_eq!(
        fake.adapter.offers("MLB1/../../users/me"),
        Err(PlatformError::NotFound)
    );
    assert_eq!(
        fake.adapter.end("C-MLB1/../x", PromotionKind::SellerCampaign),
        Err(PlatformError::NotFound)
    );
    assert_eq!(
        fake.adapter
            .leave("MLB1", PromotionKind::Coupon, Some("C-MLB1&promotion_type=DEAL")),
        Err(PlatformError::NotFound)
    );
    assert_eq!(
        fake.adapter.join(
            "MLB1",
            &OfferToJoin {
                kind: PromotionKind::SellerCampaign,
                promotion: Some("C-MLB1\",\"deal_price\":1".into()),
                price: Some(brl("10")),
                days: None,
            },
        ),
        Err(PlatformError::NotFound)
    );
    assert_eq!(
        fake.adapter.end("C-MLB1", PromotionKind::ChannelCampaign),
        Err(PlatformError::NotFound)
    );
    assert!(
        fake.server
            .requests()
            .iter()
            .all(|path| !path.starts_with("/seller-promotions"))
    );
}
