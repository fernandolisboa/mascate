//! Mercado Livre's adapter against a fake server serving the answers in
//! `fixtures/mercado-livre`, written from the documentation (see its README).

use std::str::FromStr;
use std::sync::Arc;

use chrono::{TimeDelta, TimeZone, Utc};
use mascate_catalog::{DemandCategory, DemandSource};
use mascate_integrations::{MercadoLivre, SellerAuthorization};
use mascate_kernel::testing::ManualClock;
use mascate_kernel::{Currency, ListingType, Money, PlatformError};
use mascate_platform::testing::{FakeHttpServer, MemorySecretStore};
use mascate_platform::{Secret, SecretStore};
use rust_decimal::Decimal;

const ACCESS_TOKEN: &str = "APP_USR-123456-090515-8cc4448aac10d5105474e1351-1234567";
const RENEWED_REFRESH_TOKEN: &str = "TG-5b9032b4e4b0714aed1f959f-1234567";
const REDIRECT: &str = "https://github.com/fernandolisboa";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/mercado-livre/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn brl(amount: &str) -> Money {
    Money::new(Decimal::from_str(amount).unwrap(), Currency::Brl)
}

struct Fake {
    server: FakeHttpServer,
    store: Arc<MemorySecretStore>,
    clock: Arc<ManualClock>,
    adapter: MercadoLivre,
}

impl Fake {
    /// A connected seller account, with a fake Mercado Livre that renews its
    /// login.
    fn connected() -> Self {
        let fake = Self::not_connected();
        for (name, value) in [
            ("ML_CLIENT_ID", "1234567890"),
            ("ML_CLIENT_SECRET", "s3cr3t"),
            ("ML_REFRESH_TOKEN", "TG-first"),
        ] {
            fake.store.write(name, &Secret::new(value)).unwrap();
        }
        fake.server
            .serve("/oauth/token", 200, fixture("token.json"));
        fake
    }

    /// The app's keys saved, before the seller authorized it.
    fn with_keys() -> Self {
        let fake = Self::not_connected();
        for (name, value) in [
            ("ML_CLIENT_ID", "1234567890"),
            ("ML_CLIENT_SECRET", "s3cr3t"),
        ] {
            fake.store.write(name, &Secret::new(value)).unwrap();
        }
        fake
    }

    fn not_connected() -> Self {
        let server = FakeHttpServer::start();
        let store = Arc::new(MemorySecretStore::default());
        let clock = Arc::new(ManualClock::at(
            Utc.with_ymd_and_hms(2026, 10, 4, 12, 0, 0).unwrap(),
        ));
        let adapter = MercadoLivre::new(server.url(), "Mascate/test", store.clone(), clock.clone());
        Self {
            server,
            store,
            clock,
            adapter,
        }
    }

    fn serve(&self, path: &str, name: &str) {
        self.server.serve(path, 200, fixture(name));
    }

    fn renewals(&self) -> usize {
        self.server
            .received()
            .iter()
            .filter(|request| request.path == "/oauth/token")
            .count()
    }
}

#[test]
fn best_sellers_come_with_title_catalog_product_and_price_whatever_their_kind() {
    let fake = Fake::connected();
    fake.serve(
        "/highlights/MLB/category/MLB1000",
        "highlights-MLB1000.json",
    );
    fake.serve("/products/MLB19615318", "product-MLB19615318.json");
    fake.serve(
        "/user-products/MLBU3013800008",
        "user-product-MLBU3013800008.json",
    );
    fake.serve(
        "/items?ids=MLB6868664726&attributes=id%2Ctitle%2Cprice%2Ccurrency_id%2Ccatalog_product_id",
        "items-multiget.json",
    );
    // MLB24162817 is not served: a product Mercado Livre took down is left out.

    let best_sellers = fake.adapter.best_sellers("MLB1000").unwrap();

    let summary: Vec<_> = best_sellers
        .iter()
        .map(|seller| {
            (
                seller.position,
                seller.id.as_str(),
                seller.title.as_str(),
                seller.catalog_product.as_deref(),
                seller.price,
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                1,
                "MLB19615318",
                "Fone de Ouvido Bluetooth TWS Pro",
                Some("MLB19615318"),
                Some(brl("99.9"))
            ),
            (
                2,
                "MLBU3013800008",
                "Caixa de Som Bluetooth à Prova d'Água",
                Some("MLB24162817"),
                None
            ),
            (
                3,
                "MLB6868664726",
                "Carregador Turbo USB-C 20W Original",
                None,
                Some(brl("45.5"))
            ),
        ]
    );
    let requests = fake.server.received();
    assert_eq!(fake.renewals(), 1);
    assert!(
        requests
            .iter()
            .filter(|request| request.path != "/oauth/token")
            .all(|request| request.authorization.as_deref()
                == Some(&format!("Bearer {ACCESS_TOKEN}")[..]))
    );
}

#[test]
fn a_category_without_a_ranking_has_no_best_sellers() {
    let fake = Fake::connected();

    assert_eq!(fake.adapter.best_sellers("MLB99999"), Ok(Vec::new()));
}

#[test]
fn renewing_the_login_stores_the_new_single_use_refresh_token() {
    let fake = Fake::connected();
    fake.serve("/sites/MLB/categories", "categories.json");

    let categories = fake.adapter.categories().unwrap();

    assert_eq!(categories.len(), 4);
    assert_eq!(
        categories[2],
        DemandCategory {
            id: "MLB1000".into(),
            name: "Eletrônicos, Áudio e Vídeo".into()
        }
    );
    let renewal = &fake.server.received()[0];
    assert_eq!(renewal.method, "POST");
    assert_eq!(renewal.path, "/oauth/token");
    assert_eq!(
        renewal.body,
        "grant_type=refresh_token&client_id=1234567890&client_secret=s3cr3t&refresh_token=TG-first"
    );
    assert_eq!(
        fake.store.read("ML_REFRESH_TOKEN").unwrap(),
        Some(Secret::new(RENEWED_REFRESH_TOKEN))
    );
}

#[test]
fn the_access_token_is_reused_until_it_is_about_to_expire() {
    let fake = Fake::connected();
    fake.serve("/sites/MLB/categories", "categories.json");

    fake.adapter.categories().unwrap();
    fake.clock.advance(TimeDelta::hours(5));
    fake.adapter.categories().unwrap();
    assert_eq!(fake.renewals(), 1);

    // token.json lasts 6 hours; 5 minutes before that the app renews.
    fake.clock.advance(TimeDelta::minutes(56));
    fake.adapter.categories().unwrap();
    assert_eq!(fake.renewals(), 2);
    let second = fake
        .server
        .received()
        .into_iter()
        .filter(|request| request.path == "/oauth/token")
        .nth(1)
        .unwrap();
    assert!(
        second
            .body
            .ends_with(&format!("refresh_token={RENEWED_REFRESH_TOKEN}"))
    );
}

#[test]
fn concurrent_requests_renew_the_login_only_once() {
    let fake = Arc::new(Fake::connected());
    fake.serve("/sites/MLB/categories", "categories.json");

    let workers: Vec<_> = (0..8)
        .map(|_| {
            let fake = fake.clone();
            std::thread::spawn(move || fake.adapter.categories().map(|found| found.len()))
        })
        .collect();
    for worker in workers {
        assert_eq!(worker.join().unwrap(), Ok(4));
    }

    assert_eq!(fake.renewals(), 1);
}

#[test]
fn without_the_seller_login_nothing_is_requested() {
    let fake = Fake::not_connected();
    fake.store
        .write("ML_CLIENT_ID", &Secret::new("1234567890"))
        .unwrap();

    assert_eq!(fake.adapter.categories(), Err(PlatformError::NotConnected));
    assert!(fake.server.received().is_empty());
}

#[test]
fn the_seller_authorizes_the_app_on_mercado_livres_page_with_the_registered_address() {
    let fake = Fake::with_keys();
    let page = MercadoLivre::new(
        fake.server.url(),
        "Mascate/test",
        fake.store.clone(),
        fake.clock.clone(),
    );

    assert_eq!(
        page.authorization_url(REDIRECT).unwrap(),
        "https://auth.mercadolivre.com.br/authorization?response_type=code&client_id=1234567890\
         &redirect_uri=https%3A%2F%2Fgithub.com%2Ffernandolisboa&state=mascate"
    );
    assert_eq!(
        page.with_authorization_page("http://127.0.0.1:8090/authorization")
            .authorization_url(REDIRECT)
            .unwrap(),
        "http://127.0.0.1:8090/authorization?response_type=code&client_id=1234567890\
         &redirect_uri=https%3A%2F%2Fgithub.com%2Ffernandolisboa&state=mascate"
    );
    assert!(fake.server.received().is_empty());
}

#[test]
fn the_code_comes_out_of_the_address_the_browser_landed_on() {
    let code = |address: &str| SellerAuthorization::from_landing(address).map(|found| found.code);

    assert_eq!(
        code("https://github.com/fernandolisboa?code=TG-61828b7fffcc9a001b4bc890-314029626&state=mascate")
            .as_deref(),
        Some("TG-61828b7fffcc9a001b4bc890-314029626")
    );
    assert_eq!(code("https://github.com/fernandolisboa"), None);
    assert_eq!(
        code("https://github.com/fernandolisboa?code=&state=mascate"),
        None
    );
}

#[test]
fn the_code_is_traded_for_the_seller_login_and_its_refresh_token_is_stored() {
    let fake = Fake::with_keys();
    fake.server.serve(
        "/oauth/token",
        200,
        fixture("token-authorization-code.json"),
    );
    fake.serve("/sites/MLB/categories", "categories.json");

    fake.adapter
        .authorize(
            &SellerAuthorization {
                code: "TG-61828b7fffcc9a001b4bc890-314029626".into(),
            },
            REDIRECT,
        )
        .unwrap();

    let trade = &fake.server.received()[0];
    assert_eq!(
        (trade.method.as_str(), trade.path.as_str()),
        ("POST", "/oauth/token")
    );
    assert_eq!(
        trade.body,
        "grant_type=authorization_code&client_id=1234567890&client_secret=s3cr3t\
         &code=TG-61828b7fffcc9a001b4bc890-314029626\
         &redirect_uri=https%3A%2F%2Fgithub.com%2Ffernandolisboa"
    );
    assert_eq!(
        fake.store.read("ML_REFRESH_TOKEN").unwrap(),
        Some(Secret::new("TG-61828b7fffcc9a001b4bc890-first"))
    );
    // The login just traded serves the next call without a renewal.
    fake.adapter.categories().unwrap();
    assert_eq!(fake.renewals(), 1);
    assert_eq!(
        fake.server.received()[1].authorization,
        Some("Bearer APP_USR-123456-090515-authorized-1234567".into())
    );
}

#[test]
fn a_refused_code_keeps_no_login_and_says_why() {
    let fake = Fake::with_keys();
    fake.server
        .serve("/oauth/token", 400, fixture("token-invalid-grant.json"));

    let refused = fake.adapter.authorize(
        &SellerAuthorization {
            code: "TG-used".into(),
        },
        REDIRECT,
    );

    assert_eq!(
        refused,
        Err(PlatformError::Refused(
            "Error validating grant. Your authorization code or refresh token may be expired or \
             it was already used"
                .into()
        ))
    );
    assert_eq!(fake.store.read("ML_REFRESH_TOKEN").unwrap(), None);
}

#[test]
fn keys_mercado_livre_does_not_know_are_refused_as_such() {
    let fake = Fake::with_keys();
    fake.server
        .serve("/oauth/token", 401, fixture("token-invalid-client.json"));

    assert_eq!(
        fake.adapter.authorize(
            &SellerAuthorization {
                code: "TG-new".into()
            },
            REDIRECT
        ),
        Err(PlatformError::Refused(
            "Mercado Livre does not know this app".into()
        ))
    );
}

#[test]
fn without_the_keys_the_authorization_page_is_not_opened() {
    let fake = Fake::not_connected();

    assert_eq!(
        fake.adapter.authorization_url(REDIRECT),
        Err(PlatformError::NotConnected)
    );
    assert_eq!(
        fake.adapter.authorize(
            &SellerAuthorization {
                code: "TG-new".into()
            },
            REDIRECT
        ),
        Err(PlatformError::NotConnected)
    );
    assert!(fake.server.received().is_empty());
}

#[test]
fn a_refresh_token_mercado_livre_no_longer_accepts_means_the_login_expired() {
    let fake = Fake::connected();
    fake.server
        .serve("/oauth/token", 400, fixture("token-invalid-grant.json"));

    assert_eq!(fake.adapter.categories(), Err(PlatformError::Expired));
    assert_eq!(
        fake.store.read("ML_REFRESH_TOKEN").unwrap(),
        Some(Secret::new("TG-first"))
    );
}

#[test]
fn a_turned_down_access_token_is_renewed_once_before_the_login_counts_as_expired() {
    let fake = Fake::connected();
    fake.server.serve("/sites/MLB/categories", 401, "{}");
    fake.server
        .then_serve("/sites/MLB/categories", 200, fixture("categories.json"));

    assert_eq!(fake.adapter.categories().map(|found| found.len()), Ok(4));
    assert_eq!(fake.renewals(), 2);

    fake.server.serve("/sites/MLB/categories", 401, "{}");
    assert_eq!(fake.adapter.categories(), Err(PlatformError::Expired));
    assert_eq!(fake.renewals(), 3);
}

#[test]
fn rate_limits_and_refusals_are_reported_as_such() {
    let fake = Fake::connected();
    fake.server
        .serve("/sites/MLB/categories", 429, fixture("error-429.json"));
    fake.server
        .serve("/categories/MLB3697", 403, fixture("error-403.json"));
    fake.server
        .serve("/products/MLB1/items?limit=50", 500, "{}");

    assert_eq!(fake.adapter.categories(), Err(PlatformError::RateLimited));
    assert_eq!(
        fake.adapter.root_category("MLB3697"),
        Err(PlatformError::Refused("forbidden".into()))
    );
    assert!(matches!(
        fake.adapter.competition("MLB1"),
        Err(PlatformError::Failed(_))
    ));
    assert_eq!(
        fake.adapter.competition("MLB404"),
        Err(PlatformError::NotFound)
    );
}

#[test]
fn an_answer_that_is_not_what_the_documentation_shows_is_a_failure() {
    let fake = Fake::connected();
    fake.server
        .serve("/sites/MLB/categories", 200, r#"{"unexpected": true}"#);

    assert!(matches!(
        fake.adapter.categories(),
        Err(PlatformError::Failed(_))
    ));
}

#[test]
fn searching_the_catalog_finds_products_by_title() {
    let fake = Fake::connected();
    fake.serve(
        "/products/search?status=active&site_id=MLB&q=Fone%20Bluetooth%20TWS&limit=5",
        "products-search.json",
    );

    let found = fake.adapter.search("Fone Bluetooth TWS").unwrap();

    let names: Vec<_> = found
        .iter()
        .map(|m| (m.id.as_str(), m.name.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("MLB19615318", "Fone de Ouvido Bluetooth TWS Pro"),
            ("MLB22345678", "Fone Bluetooth TWS com Estojo de Carga"),
        ]
    );
}

#[test]
fn the_competition_counts_every_listing_and_prices_the_first_page() {
    let fake = Fake::connected();
    fake.serve(
        "/products/MLB19615318/items?limit=50",
        "product-items-MLB19615318.json",
    );

    let competition = fake.adapter.competition("MLB19615318").unwrap();

    assert_eq!(competition.sellers, 7);
    assert_eq!(competition.prices, [brl("89.9"), brl("99.9"), brl("119.9")]);
    assert_eq!(competition.category.as_deref(), Some("MLB3697"));
}

#[test]
fn a_category_is_filed_under_its_top_level_category() {
    let fake = Fake::connected();
    fake.serve("/categories/MLB3697", "category-MLB3697.json");

    assert_eq!(
        fake.adapter.root_category("MLB3697"),
        Ok(DemandCategory {
            id: "MLB1000".into(),
            name: "Eletrônicos, Áudio e Vídeo".into()
        })
    );
}

#[test]
fn the_sale_fee_is_mercado_livres_for_the_category_price_and_listing_type() {
    let fake = Fake::connected();
    fake.serve(
        "/sites/MLB/listing_prices?price=99.93&listing_type_id=gold_special&category_id=MLB3697",
        "listing-prices.json",
    );

    let fee = fake
        .adapter
        .sale_fee("MLB3697", brl("99.9333"), ListingType::Classic)
        .unwrap();

    assert_eq!(fee, brl("13.99"));
    // The answer is for Clássico; asking for Premium finds no fee in it.
    fake.serve(
        "/sites/MLB/listing_prices?price=99.93&listing_type_id=gold_pro&category_id=MLB3697",
        "listing-prices.json",
    );
    assert_eq!(
        fake.adapter
            .sale_fee("MLB3697", brl("99.93"), ListingType::Premium),
        Err(PlatformError::NotFound)
    );
}
