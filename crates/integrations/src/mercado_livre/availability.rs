//! How long an On-Order Listing takes to be ready (#207, ADR 0038), for
//! commerce's Listing Availability port: Mercado Livre's sale term
//! `MANUFACTURING_TIME`, up to 60 days, in a unit the listing's category
//! allows.

use mascate_commerce::ListingAvailability;
use mascate_kernel::PlatformError;
use serde_json::json;

use super::MercadoLivre;
use super::answers::{ChangeNotices, ItemCategory, SaleTermAnswer};
use super::sales_channel::path_id;

const MANUFACTURING_TIME: &str = "MANUFACTURING_TIME";

/// What Mercado Livre answers, with a 200, when it will drop the days in
/// the background: on a listing with Flex or in Full (cause 2110).
const DROPPED_CODE: &str = "delete.item.sale_terms.manufacturing_time";
const DROPPED_CAUSE: u64 = 2110;

impl ListingAvailability for MercadoLivre {
    /// The days go in the unit the category names, read right before, as
    /// the documentation asks; `None` sends the term with null values,
    /// which is how Mercado Livre takes it off.
    fn set_availability(&self, id: &str, days: Option<u32>) -> Result<(), PlatformError> {
        let path = format!("/items/{}", path_id(id)?);
        let Some(days) = days else {
            let body = json!({
                "sale_terms": [{ "id": MANUFACTURING_TIME, "value_id": null, "value_name": null }]
            });
            return self.put(&path, &body.to_string());
        };
        let unit = self.manufacturing_unit(&path)?;
        let body = json!({
            "sale_terms": [{ "id": MANUFACTURING_TIME, "value_name": format!("{days} {unit}") }]
        });
        let answer: ChangeNotices = self.put_for(&path, &body.to_string())?;
        let dropped = answer.warnings.iter().chain(&answer.cause).find(|notice| {
            notice.code.as_deref() == Some(DROPPED_CODE) || notice.cause_id == Some(DROPPED_CAUSE)
        });
        match dropped {
            Some(notice) => Err(PlatformError::Refused(match &notice.message {
                Some(message) => format!(
                    "anúncio com Mercado Envios Flex ou no Full não aceita prazo de \
                     disponibilidade ({message})"
                ),
                None => "anúncio com Mercado Envios Flex ou no Full não aceita prazo de \
                         disponibilidade"
                    .into(),
            })),
            None => Ok(()),
        }
    }
}

impl MercadoLivre {
    /// The unit of `MANUFACTURING_TIME` in the category of the listing at
    /// `path`: its default, else the first it allows.
    fn manufacturing_unit(&self, path: &str) -> Result<String, PlatformError> {
        let not_taken = || {
            PlatformError::Refused(
                "a categoria deste anúncio não aceita prazo de disponibilidade".into(),
            )
        };
        let item: ItemCategory = self.get(path, &[])?;
        let category = item.category_id.ok_or_else(not_taken)?;
        let terms: Vec<SaleTermAnswer> = self.get(
            &format!("/categories/{}/sale_terms", path_id(&category)?),
            &[],
        )?;
        let term = terms
            .into_iter()
            .find(|term| term.id == MANUFACTURING_TIME)
            .ok_or_else(not_taken)?;
        let allowed = |unit: &String| {
            term.allowed_units.is_empty()
                || term.allowed_units.iter().any(|known| &known.id == unit)
        };
        term.default_unit
            .clone()
            .filter(allowed)
            .or_else(|| term.allowed_units.first().map(|unit| unit.id.clone()))
            .ok_or_else(not_taken)
    }
}
