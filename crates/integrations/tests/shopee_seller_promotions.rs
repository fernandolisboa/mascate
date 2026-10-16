//! The owner's Shopee discounts through the Open Platform v2 seller
//! adapter, against the fake server serving the answers in
//! `fixtures/shopee-seller`, written from the documentation (see its
//! README). The margin guard and the owner's confirmation are commerce's,
//! tested there.

use std::sync::Arc;

use chrono::{NaiveDate, TimeZone, Utc};
use hmac::{Hmac, Mac};
use mascate_commerce::{
    ChannelOffer, ChannelPromotion, ChannelPromotions, OfferToJoin, PromotionKind, PromotionPlan,
    PromotionStatus,
};
use mascate_integrations::ShopeeSeller;
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{Currency, Money, PlatformError};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};
use rust_decimal::Decimal;
use serde_json::json;
use sha2::Sha256;

const PARTNER: &str = "2001887";
const KEY: &str = "shpk-mascate-partner-key";
const SHOP: &str = "14701711";
/// 2026-10-16 12:00 UTC, the fake clock's time: 09:00 in Brasília.
const NOW: i64 = 1_792_152_000;
const ACCESS: &str = "59777174636562737266615546704c6d";

const RENEW: &str = "/api/v2/auth/access_token/get";
const DISCOUNT_LIST: &str = "/api/v2/discount/get_discount_list";
const DISCOUNT: &str = "/api/v2/discount/get_discount";
const ADD_DISCOUNT: &str = "/api/v2/discount/add_discount";
const UPDATE_DISCOUNT: &str = "/api/v2/discount/update_discount";
const DELETE_DISCOUNT: &str = "/api/v2/discount/delete_discount";
const END_DISCOUNT: &str = "/api/v2/discount/end_discount";
const ADD_DISCOUNT_ITEM: &str = "/api/v2/discount/add_discount_item";
const DELETE_DISCOUNT_ITEM: &str = "/api/v2/discount/delete_discount_item";
const ITEM_PROMOTION: &str = "/api/v2/product/get_item_promotion";
const MODELS: &str = "/api/v2/product/get_model_list";

const UPCOMING: &str = "665123666665499";
const ONGOING: &str = "665123666665500";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/shopee-seller/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn sign(base: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(KEY.as_bytes()).unwrap();
    mac.update(base.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn public(path: &str) -> String {
    format!(
        "{path}?partner_id={PARTNER}&timestamp={NOW}&sign={}",
        sign(&format!("{PARTNER}{path}{NOW}"))
    )
}

fn shop(path: &str, query: &str) -> String {
    format!(
        "{path}?partner_id={PARTNER}&timestamp={NOW}&access_token={ACCESS}&shop_id={SHOP}&sign={}{query}",
        sign(&format!("{PARTNER}{path}{NOW}{ACCESS}{SHOP}"))
    )
}

fn discount_list(status: &str) -> String {
    shop(
        DISCOUNT_LIST,
        &format!("&discount_status={status}&page_no=1&page_size=100"),
    )
}

fn discount(id: &str) -> String {
    shop(
        DISCOUNT,
        &format!("&discount_id={id}&page_no=1&page_size=100"),
    )
}

fn on(day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, day).unwrap()
}

fn brl(amount: &str) -> Money {
    Money::new(amount.parse::<Decimal>().unwrap(), Currency::Brl)
}

fn plan(name: &str, (starts, ends): (u32, u32)) -> PromotionPlan {
    PromotionPlan {
        kind: PromotionKind::SellerCampaign,
        name: name.into(),
        starts: on(starts),
        ends: on(ends),
        coupon: None,
    }
}

struct Fake {
    server: FakeHttpServer,
    adapter: ShopeeSeller,
}

impl Fake {
    fn connected() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        for (name, value) in [
            ("SHOPEE_PARTNER_ID", PARTNER),
            ("SHOPEE_PARTNER_KEY", KEY),
            ("SHOPEE_SHOP_ID", SHOP),
            ("SHOPEE_REFRESH_TOKEN", "3a4b4c4d4e4f50515253545556575859"),
        ] {
            store.write(name, &Secret::new(value)).unwrap();
        }
        let clock = Arc::new(ManualClock::at(Utc.timestamp_opt(NOW, 0).unwrap()));
        let adapter = ShopeeSeller::new(server.url(), "Mascate/test", store, clock);
        server.serve(&public(RENEW), 200, fixture("access-token-get.json"));
        let fake = Self { server, adapter };
        fake.serve(&discount_list("upcoming"), "discount-list-upcoming.json");
        fake.serve(&discount_list("ongoing"), "discount-list-ongoing.json");
        fake
    }

    fn serve(&self, path: &str, name: &str) {
        self.server.serve(path, 200, fixture(name));
    }

    fn serve_post(&self, path: &str, name: &str) {
        self.serve(&shop(path, ""), name);
    }

    fn bodies(&self, path: &str) -> Vec<serde_json::Value> {
        self.server
            .received()
            .into_iter()
            .filter(|request| request.path.starts_with(&format!("{path}?")))
            .map(|request| serde_json::from_str(&request.body).unwrap())
            .collect()
    }

    fn asked(&self, path: &str) -> usize {
        self.server
            .received()
            .iter()
            .filter(|request| request.path.starts_with(&format!("{path}?")))
            .count()
    }
}

#[test]
fn the_owners_promotions_are_the_discounts_about_to_start_and_running() {
    let fake = Fake::connected();

    let promotions = fake.adapter.promotions().unwrap();

    assert_eq!(
        promotions,
        vec![
            ChannelPromotion {
                id: UPCOMING.into(),
                kind: PromotionKind::SellerCampaign,
                name: "Semana do fone".into(),
                starts: on(18),
                ends: on(24),
                status: PromotionStatus::Pending,
                coupon: None,
            },
            ChannelPromotion {
                id: ONGOING.into(),
                kind: PromotionKind::SellerCampaign,
                name: "Outubro".into(),
                starts: on(10),
                ends: on(31),
                status: PromotionStatus::Started,
                coupon: None,
            },
        ]
    );
}

#[test]
fn a_listings_offers_name_the_owners_discounts_and_shopees_own_campaigns() {
    let fake = Fake::connected();
    fake.serve(
        &shop(ITEM_PROMOTION, "&item_id_list=800002"),
        "item-promotion.json",
    );

    let offers = fake.adapter.offers("800002").unwrap();

    assert_eq!(
        offers,
        vec![
            // Each model has its row; the offer takes the lowest price.
            ChannelOffer {
                listing: "800002".into(),
                kind: PromotionKind::SellerCampaign,
                promotion: Some(ONGOING.into()),
                name: "Outubro".into(),
                status: PromotionStatus::Started,
                price: Some(brl("42.9")),
                starts: Some(on(10)),
                ends: Some(on(31)),
            },
            ChannelOffer {
                listing: "800002".into(),
                kind: PromotionKind::ChannelCampaign,
                promotion: Some("2000001".into()),
                name: "Flash Sale".into(),
                status: PromotionStatus::Pending,
                price: Some(brl("39.9")),
                starts: Some(on(20)),
                ends: Some(on(20)),
            },
        ]
    );
}

#[test]
fn a_listing_in_no_promotion_needs_no_discount_list() {
    let fake = Fake::connected();
    fake.serve(
        &shop(ITEM_PROMOTION, "&item_id_list=800001"),
        "item-promotion-none.json",
    );

    assert!(fake.adapter.offers("800001").unwrap().is_empty());
    assert_eq!(fake.asked(DISCOUNT_LIST), 0);
    // Shopee's ids are digits: nothing else reaches a request.
    assert_eq!(
        fake.adapter.offers("800001&shop_id=1"),
        Err(PlatformError::NotFound)
    );
}

#[test]
fn a_discount_starts_on_its_first_day_or_a_little_over_an_hour_from_now() {
    let fake = Fake::connected();
    fake.serve_post(ADD_DISCOUNT, "add-discount.json");

    let today = fake
        .adapter
        .create(&plan("Semana do fone", (16, 22)))
        .unwrap();
    fake.adapter
        .create(&plan("Fim de semana", (18, 19)))
        .unwrap();

    assert_eq!(
        today,
        ChannelPromotion {
            id: "665123666665501".into(),
            kind: PromotionKind::SellerCampaign,
            name: "Semana do fone".into(),
            starts: on(16),
            ends: on(22),
            status: PromotionStatus::Pending,
            coupon: None,
        }
    );
    assert_eq!(
        fake.bodies(ADD_DISCOUNT),
        [
            // Shopee takes a start an hour from now at the soonest; the
            // last day ends at 23:59:59 in Brasília.
            json!({"discount_name": "Semana do fone", "start_time": NOW + 65 * 60,
                   "end_time": 1_792_724_399}),
            json!({"discount_name": "Fim de semana", "start_time": 1_792_292_400,
                   "end_time": 1_792_465_199}),
        ]
    );
}

#[test]
fn a_discount_whose_answer_is_lost_is_not_created_again() {
    let fake = Fake::connected();
    fake.server.hang_up(&shop(ADD_DISCOUNT, ""));

    assert!(
        fake.adapter
            .create(&plan("Semana do fone", (18, 22)))
            .is_err()
    );
    assert_eq!(fake.asked(ADD_DISCOUNT), 1);
}

#[test]
fn a_change_sends_only_the_days_that_moved_and_never_a_running_discounts_start() {
    let fake = Fake::connected();
    fake.serve(&discount(UPCOMING), "discount-upcoming.json");
    fake.serve(&discount(ONGOING), "discount-ongoing.json");
    fake.serve_post(UPDATE_DISCOUNT, "update-discount.json");

    fake.adapter
        .change(UPCOMING, &plan("Semana do fone", (19, 24)))
        .unwrap();
    fake.adapter
        .change(UPCOMING, &plan("Semana do som", (18, 21)))
        .unwrap();
    fake.adapter
        .change(ONGOING, &plan("Outubro", (10, 25)))
        .unwrap();

    assert_eq!(
        fake.bodies(UPDATE_DISCOUNT),
        [
            json!({"discount_id": 665_123_666_665_499_u64, "discount_name": "Semana do fone",
                   "start_time": 1_792_378_800}),
            json!({"discount_id": 665_123_666_665_499_u64, "discount_name": "Semana do som",
                   "end_time": 1_792_637_999}),
            json!({"discount_id": 665_123_666_665_500_u64, "discount_name": "Outubro",
                   "end_time": 1_792_983_599}),
        ]
    );
}

#[test]
fn ending_deletes_an_upcoming_discount_ends_a_running_one_and_leaves_one_over() {
    let fake = Fake::connected();
    fake.serve(&discount(UPCOMING), "discount-upcoming.json");
    fake.serve(&discount(ONGOING), "discount-ongoing.json");
    fake.serve(&discount("665123666665400"), "discount-expired.json");
    fake.serve(&discount("665123666665401"), "discount-not-found.json");
    fake.serve_post(DELETE_DISCOUNT, "delete-discount.json");
    fake.serve_post(END_DISCOUNT, "end-discount.json");

    let kind = PromotionKind::SellerCampaign;
    fake.adapter.end(UPCOMING, kind).unwrap();
    fake.adapter.end(ONGOING, kind).unwrap();
    fake.adapter.end("665123666665400", kind).unwrap();

    assert_eq!(
        fake.bodies(DELETE_DISCOUNT),
        [json!({"discount_id": 665_123_666_665_499_u64})]
    );
    assert_eq!(
        fake.bodies(END_DISCOUNT),
        [json!({"discount_id": 665_123_666_665_500_u64})]
    );
    // One Shopee no longer has is gone already, for commerce too.
    assert_eq!(
        fake.adapter.end("665123666665401", kind),
        Err(PlatformError::NotFound)
    );
}

#[test]
fn joining_puts_every_model_on_sale_at_the_price_with_no_limit_per_buyer() {
    let fake = Fake::connected();
    fake.serve(&shop(MODELS, "&item_id=800002"), "model-list-camiseta.json");
    fake.serve(&shop(MODELS, "&item_id=800001"), "model-list-empty.json");
    fake.serve_post(ADD_DISCOUNT_ITEM, "add-discount-item.json");
    let offer = OfferToJoin {
        kind: PromotionKind::SellerCampaign,
        promotion: Some(ONGOING.into()),
        price: Some(brl("44.90")),
        days: None,
    };

    fake.adapter.join("800002", &offer).unwrap();
    fake.adapter.join("800001", &offer).unwrap();

    assert_eq!(
        fake.bodies(ADD_DISCOUNT_ITEM),
        [
            // The model Shopee does not sell stays out.
            json!({"discount_id": 665_123_666_665_500_u64, "item_list": [{
            "item_id": 800002, "purchase_limit": 0, "model_list": [
                {"model_id": 900021, "model_promotion_price": 44.9},
                {"model_id": 900022, "model_promotion_price": 44.9},
            ]}]}),
            json!({"discount_id": 665_123_666_665_500_u64, "item_list": [{
                "item_id": 800001, "item_promotion_price": 44.9, "purchase_limit": 0}]}),
        ]
    );
    let sent = fake.server.received().pop().unwrap();
    // The decimal's own text: a price never goes through a float.
    assert!(sent.body.contains(r#""item_promotion_price":44.90"#));
}

#[test]
fn a_join_shopee_refuses_in_its_answer_is_refused_once_with_its_reason() {
    let fake = Fake::connected();
    fake.serve(&shop(MODELS, "&item_id=800001"), "model-list-empty.json");
    fake.serve_post(ADD_DISCOUNT_ITEM, "add-discount-item-refused.json");
    let offer = OfferToJoin {
        kind: PromotionKind::SellerCampaign,
        promotion: Some(ONGOING.into()),
        price: Some(brl("44.9")),
        days: None,
    };

    assert_eq!(
        fake.adapter.join("800001", &offer),
        Err(PlatformError::Refused("The item status is abnormal".into()))
    );
    assert_eq!(fake.asked(ADD_DISCOUNT_ITEM), 1);
}

#[test]
fn leaving_takes_out_each_model_the_discount_has() {
    let fake = Fake::connected();
    fake.serve(&discount(ONGOING), "discount-ongoing.json");
    fake.serve_post(DELETE_DISCOUNT_ITEM, "delete-discount-item.json");
    let kind = PromotionKind::SellerCampaign;

    fake.adapter.leave("800002", kind, Some(ONGOING)).unwrap();

    assert_eq!(
        fake.bodies(DELETE_DISCOUNT_ITEM),
        [
            json!({"discount_id": 665_123_666_665_500_u64, "item_id": 800002, "model_id": 900021}),
            json!({"discount_id": 665_123_666_665_500_u64, "item_id": 800002, "model_id": 900022}),
        ]
    );
    // An item the discount does not have is out already.
    assert_eq!(
        fake.adapter.leave("800001", kind, Some(ONGOING)),
        Err(PlatformError::NotFound)
    );
}
