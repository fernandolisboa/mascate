//! How good a Prospect looks (ADR 0039, decisions 4 and 5): a score from 0
//! to 100 in four parts, each with its reason, and the aposta mark, kept
//! apart from the score so a new product the score would bury still shows.
//! Every threshold lives here.

use chrono::NaiveDate;
use mascate_kernel::{Currency, Money};
use rust_decimal::{Decimal, RoundingStrategy};

use crate::prospect_signals::{Signals, Support};

/// A product that entered its Platform this many days ago or fewer can be
/// an aposta.
pub const BET_MAX_AGE_DAYS: i64 = 90;
/// An aposta has a Blueprint of at least this many percent...
pub const BET_MIN_BLUEPRINT: Decimal = Decimal::from_parts(80, 0, 0, false, 0);
/// ...or a rating of at least this...
pub const BET_MIN_RATING: Decimal = Decimal::from_parts(45, 0, 0, false, 1);
/// ...from at least this many reviews.
pub const BET_MIN_REVIEWS: u32 = 10;
/// Hotmart says a hot product also has many affiliates competing: an
/// aposta is still cool, or its temperature is unknown.
pub const BET_MAX_TEMPERATURE: u32 = 40;

/// What each part weighs when every part has a signal: the commission and
/// the sales are what the owner asked about first.
const COMMISSION_WEIGHT: u32 = 30;
const SALES_WEIGHT: u32 = 30;
const MATERIAL_WEIGHT: u32 = 20;
const CREATOR_WEIGHT: u32 = 20;

/// The earnings per sale that score full marks, in each currency. A dollar
/// commission is never converted (ADR 0030, decision 7): it is measured
/// against dollars only.
const FULL_COMMISSION_BRL: u32 = 150;
const FULL_COMMISSION_USD: u32 = 30;
/// The temperature that scores full marks for sales.
const FULL_TEMPERATURE: u32 = 100;
const FULL_STUDENTS: u32 = 10_000;
const FULL_REVIEWS: u32 = 500;
const FULL_CLASSES: u32 = 60;
const FULL_HOURS: u32 = 20;
const FULL_CREATOR_YEARS: u32 = 5;
/// A rating scores from this up to five stars.
const LOWEST_SCORED_RATING: u32 = 3;

/// One of the four parts of a score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScoreArea {
    Commission,
    Sales,
    Material,
    Creator,
}

impl ScoreArea {
    pub const ALL: [ScoreArea; 4] = [
        ScoreArea::Commission,
        ScoreArea::Sales,
        ScoreArea::Material,
        ScoreArea::Creator,
    ];

    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            ScoreArea::Commission => "comissão",
            ScoreArea::Sales => "vendas",
            ScoreArea::Material => "material",
            ScoreArea::Creator => "criador",
        }
    }

    /// The signals that would give this part, when it has none.
    pub fn wanted(self) -> &'static str {
        match self {
            ScoreArea::Commission => "comissão (em R$, ou preço e %)",
            ScoreArea::Sales => "temperatura, alunos ou avaliações",
            ScoreArea::Material => "Blueprint, aulas, horas, certificado ou vídeo",
            ScoreArea::Creator => {
                "tempo na plataforma, perfil verificado, nota, satisfação ou suporte"
            }
        }
    }

    fn weight(self) -> u32 {
        match self {
            ScoreArea::Commission => COMMISSION_WEIGHT,
            ScoreArea::Sales => SALES_WEIGHT,
            ScoreArea::Material => MATERIAL_WEIGHT,
            ScoreArea::Creator => CREATOR_WEIGHT,
        }
    }
}

/// A part of a score and why it scored that.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScorePart {
    pub area: ScoreArea,
    /// From 0 to 100.
    pub points: u8,
    /// In Portuguese, from the signals it used.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Score {
    /// From 0 to 100: the parts with a signal, weighed, the missing parts'
    /// weight shared among them.
    pub total: u8,
    /// The parts with a signal, in [`ScoreArea::ALL`] order.
    pub parts: Vec<ScorePart>,
    /// The parts left out for want of a signal.
    pub missing: Vec<ScoreArea>,
    /// What one sale earns, the commission part's measure.
    pub per_sale: Option<Money>,
}

impl Score {
    /// Some part had no signal.
    pub fn partial(&self) -> bool {
        !self.missing.is_empty()
    }
}

/// Scores `signals` as of `today`. The same signals on the same day always
/// give the same score.
pub fn score(signals: &Signals, today: NaiveDate) -> Score {
    let per_sale = per_sale(signals);
    let mut parts = Vec::new();
    let mut missing = Vec::new();
    for area in ScoreArea::ALL {
        let part = match area {
            ScoreArea::Commission => commission_part(signals, per_sale),
            ScoreArea::Sales => sales_part(signals),
            ScoreArea::Material => material_part(signals),
            ScoreArea::Creator => creator_part(signals, today),
        };
        match part {
            Some((points, reason)) => parts.push((area, points, reason)),
            None => missing.push(area),
        }
    }
    let weights: u32 = parts.iter().map(|(area, _, _)| area.weight()).sum();
    let total = if weights == 0 {
        Decimal::ZERO
    } else {
        parts
            .iter()
            .map(|(area, points, _)| points * Decimal::from(area.weight()))
            .sum::<Decimal>()
            / Decimal::from(weights)
    };
    Score {
        total: whole(total),
        parts: parts
            .into_iter()
            .map(|(area, points, reason)| ScorePart {
                area,
                points: whole(points),
                reason,
            })
            .collect(),
        missing,
        per_sale,
    }
}

/// An aposta (ADR 0039, decision 5): in its Platform for up to 90 days,
/// with a good Blueprint or a well-reviewed rating, and still few
/// affiliates by its temperature.
pub fn is_bet(signals: &Signals, today: NaiveDate) -> bool {
    let new = signals.launched.is_some_and(|launched| {
        launched <= today && (today - launched).num_days() <= BET_MAX_AGE_DAYS
    });
    let good = signals
        .blueprint
        .is_some_and(|blueprint| blueprint.percent() >= BET_MIN_BLUEPRINT)
        || (signals
            .rating
            .is_some_and(|rating| rating >= BET_MIN_RATING)
            && signals
                .reviews
                .is_some_and(|reviews| reviews >= BET_MIN_REVIEWS));
    let cool = signals
        .temperature
        .is_none_or(|degrees| degrees <= BET_MAX_TEMPERATURE);
    new && good && cool
}

/// What one sale earns: the commission in money, else the price times the
/// rate, because 50% of R$ 47 earns less than 30% of R$ 497.
fn per_sale(signals: &Signals) -> Option<Money> {
    signals
        .commission
        .or_else(|| Some(signals.commission_rate?.of(signals.price?)))
}

fn commission_part(signals: &Signals, per_sale: Option<Money>) -> Option<(Decimal, String)> {
    let per_sale = per_sale?;
    let full = match per_sale.currency() {
        Currency::Brl => FULL_COMMISSION_BRL,
        Currency::Usd => FULL_COMMISSION_USD,
    };
    let mut reason = format!("{} por venda", per_sale.rounded().to_pt_br());
    if signals.commission.is_none()
        && let (Some(rate), Some(price)) = (signals.commission_rate, signals.price)
    {
        reason.push_str(&format!(
            " ({} de {})",
            rate.to_pt_br(),
            price.rounded().to_pt_br()
        ));
    }
    if per_sale.currency() == Currency::Usd {
        reason.push_str(", em dólar, comparada só com dólar");
    }
    Some((scale(per_sale.amount(), full), reason))
}

fn sales_part(signals: &Signals) -> Option<(Decimal, String)> {
    if let Some(degrees) = signals.temperature {
        return Some((
            scale(degrees.into(), FULL_TEMPERATURE),
            format!("temperatura {degrees}°"),
        ));
    }
    let mut found = Found::default();
    if let Some(students) = signals.students {
        found.add(
            scale(students.into(), FULL_STUDENTS),
            count_text(students, "aluno", "alunos"),
        );
    }
    if let Some(reviews) = signals.reviews {
        found.add(
            scale(reviews.into(), FULL_REVIEWS),
            count_text(reviews, "avaliação", "avaliações"),
        );
    }
    found.part()
}

fn material_part(signals: &Signals) -> Option<(Decimal, String)> {
    if let Some(blueprint) = signals.blueprint {
        return Some((
            blueprint.percent(),
            format!("Blueprint {}", blueprint.to_pt_br()),
        ));
    }
    let mut found = Found::default();
    if let Some(classes) = signals.classes {
        found.add(
            scale(classes.into(), FULL_CLASSES),
            count_text(classes, "aula", "aulas"),
        );
    }
    if let Some(hours) = signals.hours {
        found.add(
            scale(hours, FULL_HOURS),
            format!("{} h", decimal_text(hours)),
        );
    }
    if let Some(certificate) = signals.certificate {
        found.add(
            yes_no(certificate),
            if certificate {
                "com certificado"
            } else {
                "sem certificado"
            }
            .to_owned(),
        );
    }
    if let Some(video) = signals.video {
        found.add(
            yes_no(video),
            if video { "com vídeo" } else { "sem vídeo" }.to_owned(),
        );
    }
    found.part()
}

fn creator_part(signals: &Signals, today: NaiveDate) -> Option<(Decimal, String)> {
    let mut found = Found::default();
    if let Some(since) = signals.creator_since {
        let years = Decimal::from((today - since).num_days().max(0)) / Decimal::from(365);
        let whole_years = years.trunc();
        found.add(
            scale(years, FULL_CREATOR_YEARS),
            match whole_years.is_zero() {
                true => "há menos de 1 ano na plataforma".to_owned(),
                false => format!(
                    "há {} na plataforma",
                    count_text(whole_years.try_into().unwrap_or(u32::MAX), "ano", "anos")
                ),
            },
        );
    }
    if let Some(verified) = signals.creator_verified {
        found.add(
            yes_no(verified),
            if verified {
                "perfil verificado"
            } else {
                "perfil não verificado"
            }
            .to_owned(),
        );
    }
    if let Some(rating) = signals.rating {
        found.add(
            scale(
                rating - Decimal::from(LOWEST_SCORED_RATING),
                5 - LOWEST_SCORED_RATING,
            ),
            format!("nota {}", decimal_text(rating)),
        );
    }
    if let Some(satisfaction) = signals.satisfaction {
        found.add(
            satisfaction.percent(),
            format!("satisfação {}", satisfaction.to_pt_br()),
        );
    }
    if let Some(support) = signals.support {
        found.add(
            yes_no(support == Support::Good),
            match support {
                Support::Good => "suporte bom, pela sua marca",
                Support::Bad => "suporte ruim, pela sua marca",
            }
            .to_owned(),
        );
    }
    found.part()
}

/// The signals a part found, averaged.
#[derive(Default)]
struct Found {
    points: Vec<Decimal>,
    reasons: Vec<String>,
}

impl Found {
    fn add(&mut self, points: Decimal, reason: String) {
        self.points.push(points);
        self.reasons.push(reason);
    }

    fn part(self) -> Option<(Decimal, String)> {
        if self.points.is_empty() {
            return None;
        }
        let average = self.points.iter().sum::<Decimal>() / Decimal::from(self.points.len() as u32);
        Some((average, self.reasons.join(", ")))
    }
}

/// `value` as points out of 100, `full` and above scoring 100.
fn scale(value: Decimal, full: u32) -> Decimal {
    (value * Decimal::ONE_HUNDRED / Decimal::from(full)).clamp(Decimal::ZERO, Decimal::ONE_HUNDRED)
}

fn yes_no(yes: bool) -> Decimal {
    if yes {
        Decimal::ONE_HUNDRED
    } else {
        Decimal::ZERO
    }
}

fn whole(points: Decimal) -> u8 {
    points
        .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
        .try_into()
        .unwrap_or(u8::MAX)
}

fn decimal_text(value: Decimal) -> String {
    value.normalize().to_string().replace('.', ",")
}

/// "1 aula", "1.234 alunos".
fn count_text(count: u32, one: &str, many: &str) -> String {
    let digits = count.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push('.');
        }
        grouped.push(digit);
    }
    format!("{grouped} {}", if count == 1 { one } else { many })
}
