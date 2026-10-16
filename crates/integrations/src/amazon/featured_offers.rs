//! The price at which each of the owner's Amazon offers can expect to be
//! the featured one, for commerce's Automatic Repricing (#135, ADR 0033):
//! `getFeaturedOfferExpectedPriceBatch` of the Product Pricing API
//! 2022-05-01, 40 SKUs a call.

use std::time::Duration;

use mascate_commerce::{FeaturedOffer, FeaturedOfferPrices};
use mascate_kernel::{Currency, Money, PlatformError};
use serde::Deserialize;

use super::answers::Amount;
use super::{Amazon, MARKETPLACE};

const BATCH_PATH: &str = "/batches/products/pricing/2022-05-01/offer/featuredOfferExpectedPrice";

const OFFER_URI: &str = "/products/pricing/2022-05-01/offer/featuredOfferExpectedPrice";

/// The most SKUs one call takes.
const BATCH_SIZE: usize = 40;

/// The operation allows one call every 30 seconds.
const BETWEEN_BATCHES: Duration = Duration::from_secs(30);

#[derive(Deserialize)]
struct BatchAnswer {
    #[serde(default)]
    responses: Vec<OfferResponse>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfferResponse {
    status: ResponseStatus,
    request: OfferRequest,
    body: Option<OfferBody>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResponseStatus {
    status_code: u16,
}

#[derive(Deserialize)]
struct OfferRequest {
    sku: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfferBody {
    #[serde(default)]
    featured_offer_expected_price_results: Vec<OfferResult>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfferResult {
    featured_offer_expected_price: Option<ExpectedPrice>,
    result_status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedPrice {
    listing_price: PriceAmount,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PriceAmount {
    currency_code: String,
    amount: Amount,
}

impl FeaturedOfferPrices for Amazon {
    /// A SKU Amazon answers with an error of its own, such as one it no
    /// longer has, is not eligible; the others still count.
    fn featured_offers(
        &self,
        ids: &[String],
    ) -> Result<Vec<(String, FeaturedOffer)>, PlatformError> {
        let mut offers = Vec::new();
        for (index, chunk) in ids.chunks(BATCH_SIZE).enumerate() {
            if index > 0 {
                self.pause.pause(BETWEEN_BATCHES);
            }
            let answer: BatchAnswer = self.send("POST", BATCH_PATH, &[], &body(chunk))?;
            offers.extend(
                answer
                    .responses
                    .into_iter()
                    .filter(|response| chunk.contains(&response.request.sku))
                    .map(|response| {
                        let offer = featured(&response);
                        (response.request.sku, offer)
                    }),
            );
        }
        Ok(offers)
    }
}

fn body(skus: &[String]) -> String {
    let requests: Vec<serde_json::Value> = skus
        .iter()
        .map(|sku| {
            serde_json::json!({
                "uri": OFFER_URI,
                "method": "GET",
                "marketplaceId": MARKETPLACE,
                "sku": sku,
            })
        })
        .collect();
    serde_json::json!({ "requests": requests }).to_string()
}

/// `VALID_FOEP` with its price; Amazon's sample writes the plural
/// `NO_COMPETING_OFFERS` where its list says `NO_COMPETING_OFFER`, so both
/// count; any other status is an offer that cannot be featured.
fn featured(response: &OfferResponse) -> FeaturedOffer {
    if !(200..300).contains(&response.status.status_code) {
        return FeaturedOffer::NotEligible;
    }
    let Some(result) = response
        .body
        .as_ref()
        .and_then(|body| body.featured_offer_expected_price_results.first())
    else {
        return FeaturedOffer::NotEligible;
    };
    match result.result_status.as_str() {
        "VALID_FOEP" => result
            .featured_offer_expected_price
            .as_ref()
            .and_then(|expected| {
                Some(Money::new(
                    expected.listing_price.amount.decimal()?,
                    Currency::from_code(&expected.listing_price.currency_code)?,
                ))
            })
            .map_or(FeaturedOffer::NotEligible, FeaturedOffer::Expected),
        "NO_COMPETING_OFFER" | "NO_COMPETING_OFFERS" => FeaturedOffer::NoCompetition,
        _ => FeaturedOffer::NotEligible,
    }
}
