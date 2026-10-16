//! Mercado Livre's own price automation, for commerce's Automatic Repricing
//! (#135, ADR 0033): the rules an item takes, its automation turned on,
//! changed, read and ended under `/pricing-automation/items/{id}`, and the
//! price that wins its catalog page from `/items/{id}/price_to_win`.

use mascate_commerce::{
    ChannelAutomation, ChannelPriceAutomation, RepricingLimits, RepricingStatus,
};
use mascate_kernel::{Currency, Money, PlatformError};
use serde::Deserialize;
use serde_json::Number;

use super::MercadoLivre;
use super::answers::decimal;
use super::sales_channel::path_id;

/// The rule that follows the lowest price inside and outside Mercado
/// Livre; `INT` follows only the one inside, for an item without the first.
const PREFERRED_RULES: [&str; 2] = ["INT_EXT", "INT"];

/// Mercado Livre Brasil prices in reais; the automation sends no currency.
const CURRENCY: Currency = Currency::Brl;

#[derive(Deserialize)]
struct RulesAnswer {
    #[serde(default)]
    rules: Vec<RuleAnswer>,
}

#[derive(Deserialize)]
struct RuleAnswer {
    rule_id: String,
}

#[derive(Deserialize)]
struct AutomationAnswer {
    status: String,
    item_rule: Option<RuleAnswer>,
    min_price: Option<Number>,
    max_price: Option<Number>,
    status_detail: Option<StatusDetail>,
}

#[derive(Deserialize)]
struct StatusDetail {
    cause: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct PriceToWinAnswer {
    price_to_win: Option<Number>,
    currency_id: Option<String>,
}

impl ChannelPriceAutomation for MercadoLivre {
    /// Changes the item's automation when it has one, keeping its rule;
    /// else turns one on with the best rule the item takes.
    fn automate(
        &self,
        id: &str,
        limits: &RepricingLimits,
    ) -> Result<ChannelAutomation, PlatformError> {
        let path = automation_path(id)?;
        let answer: AutomationAnswer = match self.current(id)? {
            Some(running) => {
                let rule = match running.item_rule {
                    Some(rule) => rule.rule_id,
                    None => self.best_rule(id)?,
                };
                self.put_for(&path, &body(&rule, limits))?
            }
            None => self.post(&path, &body(&self.best_rule(id)?, limits))?,
        };
        automation(answer)
    }

    fn automation(&self, id: &str) -> Result<Option<ChannelAutomation>, PlatformError> {
        self.current(id)?.map(automation).transpose()
    }

    fn stop(&self, id: &str) -> Result<(), PlatformError> {
        match self.delete(&automation_path(id)?, &[]) {
            Err(PlatformError::NotFound) => Ok(()),
            done => done,
        }
    }

    /// Only an item on a catalog page competes for it; any other has no
    /// price to win.
    fn price_to_win(&self, id: &str) -> Result<Option<Money>, PlatformError> {
        let answer: PriceToWinAnswer = match self.get(
            &format!("/items/{}/price_to_win", path_id(id)?),
            &[("version", "v2")],
        ) {
            Err(PlatformError::NotFound) => return Ok(None),
            answer => answer?,
        };
        let currency = answer
            .currency_id
            .as_deref()
            .and_then(Currency::from_code)
            .unwrap_or(CURRENCY);
        Ok(decimal(&answer.price_to_win).map(|amount| Money::new(amount, currency)))
    }
}

impl MercadoLivre {
    /// The item's automation; `None` when it has none.
    fn current(&self, id: &str) -> Result<Option<AutomationAnswer>, PlatformError> {
        match self.get(&automation_path(id)?, &[]) {
            Ok(answer) => Ok(Some(answer)),
            Err(PlatformError::NotFound) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// The first of the preferred rules the item takes, else its first.
    fn best_rule(&self, id: &str) -> Result<String, PlatformError> {
        let answer: RulesAnswer = self.get(
            &format!("/pricing-automation/items/{}/rules", path_id(id)?),
            &[],
        )?;
        PREFERRED_RULES
            .into_iter()
            .find(|preferred| answer.rules.iter().any(|rule| rule.rule_id == *preferred))
            .map(str::to_owned)
            .or_else(|| answer.rules.into_iter().next().map(|rule| rule.rule_id))
            .ok_or_else(|| {
                PlatformError::Refused("Mercado Livre has no price automation for this item".into())
            })
    }
}

fn automation_path(id: &str) -> Result<String, PlatformError> {
    Ok(format!(
        "/pricing-automation/items/{}/automation",
        path_id(id)?
    ))
}

/// The amounts go as their decimal text, JSON numbers, never floats.
fn body(rule: &str, limits: &RepricingLimits) -> String {
    format!(
        r#"{{"rule_id":{},"min_price":{},"max_price":{}}}"#,
        serde_json::Value::from(rule),
        limits.minimum.rounded().amount(),
        limits.maximum.rounded().amount()
    )
}

fn automation(answer: AutomationAnswer) -> Result<ChannelAutomation, PlatformError> {
    let price = |number: &Option<Number>| {
        decimal(number)
            .map(|amount| Money::new(amount, CURRENCY))
            .ok_or_else(|| {
                PlatformError::Failed("Mercado Livre answered an automation without limits".into())
            })
    };
    Ok(ChannelAutomation {
        limits: RepricingLimits {
            minimum: price(&answer.min_price)?,
            maximum: price(&answer.max_price)?,
        },
        status: status(&answer.status, answer.status_detail),
    })
}

/// `ACTIVE`, or paused with the cause Mercado Livre gives: it pauses the
/// automation of an item that joins a Promotion.
fn status(status: &str, detail: Option<StatusDetail>) -> RepricingStatus {
    if status == "ACTIVE" {
        return RepricingStatus::Following;
    }
    let (cause, message) = detail
        .map(|detail| (detail.cause, detail.message))
        .unwrap_or_default();
    if cause
        .as_deref()
        .is_some_and(|cause| cause.to_ascii_uppercase().contains("PROMOTION"))
    {
        return RepricingStatus::PausedByPromotion;
    }
    RepricingStatus::Paused(message.or(cause).unwrap_or_else(|| status.to_owned()))
}
