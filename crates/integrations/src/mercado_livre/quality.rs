//! Listing Quality for marketing's port (ADR 0021): the score, level and
//! pending actions of `/item/{id}/performance`, which replaced `/health`,
//! and the visits of `/items/{id}/visits/time_window`.

use mascate_kernel::PlatformError;
use mascate_marketing::{ActionKind, ChannelQuality, QualityAction, QualityLevel, QualitySource};
use rust_decimal::prelude::ToPrimitive;

use super::MercadoLivre;
use super::answers::{PerformanceAnswer, PerformanceRule, VisitsAnswer, decimal};
use super::sales_channel::path_id;

/// The words for a fix link Mercado Livre sends without its own.
const FIX_LABEL: &str = "Corrigir no Mercado Livre";

impl QualitySource for MercadoLivre {
    /// Mercado Livre answers 404 while it has not worked the quality out.
    fn quality(&self, listing: &str) -> Result<Option<ChannelQuality>, PlatformError> {
        let path = format!("/item/{}/performance", path_id(listing)?);
        let answer: PerformanceAnswer = match self.get(&path, &[]) {
            Err(PlatformError::NotFound) => return Ok(None),
            other => other?,
        };
        let Some(score) = decimal(&answer.score)
            .and_then(|score| score.round().to_u8())
            .map(|score| score.min(100))
        else {
            return Ok(None);
        };
        let pending = answer
            .buckets
            .iter()
            .flat_map(|bucket| &bucket.variables)
            .flat_map(|variable| {
                variable
                    .rules
                    .iter()
                    .filter(|rule| rule.status.as_deref() == Some("PENDING"))
                    .map(|rule| action(rule, variable.title.as_deref()))
            })
            .collect();
        Ok(Some(ChannelQuality {
            score,
            level: level(&answer, score),
            pending,
        }))
    }

    fn visits(&self, listing: &str, days: u32) -> Result<u32, PlatformError> {
        let answer: VisitsAnswer = self.get(
            &format!("/items/{}/visits/time_window", path_id(listing)?),
            &[("last", &days.to_string()), ("unit", "day")],
        )?;
        Ok(answer.total_visits)
    }
}

/// A pending rule as marketing keeps it, in the channel's words, which come
/// in Portuguese on Mercado Livre Brasil.
fn action(rule: &PerformanceRule, goal: Option<&str>) -> QualityAction {
    let wordings = rule.wordings.as_ref();
    QualityAction {
        key: rule.key.clone(),
        kind: match rule.mode.as_deref() {
            Some("WARNING") => ActionKind::Problem,
            _ => ActionKind::Opportunity,
        },
        text: worded(wordings.and_then(|w| w.title.as_deref()))
            .or(goal)
            .unwrap_or(&rule.key)
            .to_owned(),
        label: worded(wordings.and_then(|w| w.label.as_deref()))
            .unwrap_or(FIX_LABEL)
            .to_owned(),
        link: wordings
            .and_then(|w| w.link.as_deref())
            .filter(|link| on_mercado_livre(link))
            .map(str::to_owned),
    }
}

/// `text` with something besides blanks, trimmed.
fn worded(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|text| !text.is_empty())
}

/// Whether `link` opens a Mercado Livre page over HTTPS: the panel opens
/// it in the browser, so nothing else from an answer is trusted.
fn on_mercado_livre(link: &str) -> bool {
    let Some(rest) = link.strip_prefix("https://") else {
        return false;
    };
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    ["mercadolivre.com.br", "mercadolibre.com"]
        .iter()
        .any(|domain| host == *domain || host.ends_with(&format!(".{domain}")))
}

/// The level by Mercado Livre's own name for it or, failing that, by the
/// score bands of `/sites/MLB/health_levels` (below 50 basic, from 66
/// professional).
fn level(answer: &PerformanceAnswer, score: u8) -> QualityLevel {
    [&answer.level_wording, &answer.level]
        .into_iter()
        .flatten()
        .find_map(|name| match name.trim().to_lowercase().as_str() {
            "básica" | "basica" | "basic" => Some(QualityLevel::Basic),
            "satisfatória" | "satisfatoria" | "estándar" | "standard" => {
                Some(QualityLevel::Standard)
            }
            "profissional" | "profesional" | "professional" | "good" => {
                Some(QualityLevel::Professional)
            }
            _ => None,
        })
        .unwrap_or(match score {
            0..50 => QualityLevel::Basic,
            50..66 => QualityLevel::Standard,
            _ => QualityLevel::Professional,
        })
}
