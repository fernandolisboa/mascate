//! What the app knows about a Prospect (ADR 0039, decision 1): every
//! signal is optional, and each one keeps where it came from and when, so
//! reading a page again replaces the signals it read and keeps the rest.

use std::str::FromStr;

use chrono::NaiveDate;
use mascate_kernel::{Currency, Money, Percentage, Timestamp};
use rust_decimal::Decimal;

/// Where a signal came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignalOrigin {
    /// The Platform's public pages.
    Vitrine,
    /// The Platform's affiliate market, behind the owner's login.
    Market,
    /// The owner typed it.
    Typed,
}

impl SignalOrigin {
    pub const ALL: [SignalOrigin; 3] = [
        SignalOrigin::Vitrine,
        SignalOrigin::Market,
        SignalOrigin::Typed,
    ];

    pub(crate) fn code(self) -> &'static str {
        match self {
            SignalOrigin::Vitrine => "vitrine",
            SignalOrigin::Market => "market",
            SignalOrigin::Typed => "typed",
        }
    }

    pub(crate) fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|origin| origin.code() == code)
    }

    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            SignalOrigin::Vitrine => "vitrine pública",
            SignalOrigin::Market => "mercado de afiliação",
            SignalOrigin::Typed => "digitado",
        }
    }
}

/// How the producer takes a new affiliate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Affiliation {
    Immediate,
    /// The producer approves each request.
    OnRequest,
}

/// Whether the buyer gets something shipped or something online.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProductFormat {
    Physical,
    Digital,
}

impl ProductFormat {
    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            ProductFormat::Physical => "produto físico",
            ProductFormat::Digital => "produto digital",
        }
    }
}

/// The owner's own word on the creator's support to affiliates, which no
/// Platform measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Support {
    Good,
    Bad,
}

/// How a signal's value is kept as text.
trait SignalText: Sized {
    fn to_text(&self) -> String;
    fn from_text(text: &str) -> Option<Self>;
}

impl SignalText for Money {
    fn to_text(&self) -> String {
        format!("{} {}", self.amount(), self.currency().code())
    }

    fn from_text(text: &str) -> Option<Self> {
        let (amount, currency) = text.split_once(' ')?;
        Some(Money::new(
            Decimal::from_str(amount).ok()?,
            Currency::from_code(currency)?,
        ))
    }
}

impl SignalText for Percentage {
    fn to_text(&self) -> String {
        self.percent().to_string()
    }

    fn from_text(text: &str) -> Option<Self> {
        Percentage::new(Decimal::from_str(text).ok()?).ok()
    }
}

impl SignalText for Decimal {
    fn to_text(&self) -> String {
        self.normalize().to_string()
    }

    fn from_text(text: &str) -> Option<Self> {
        Decimal::from_str(text).ok()
    }
}

impl SignalText for u32 {
    fn to_text(&self) -> String {
        self.to_string()
    }

    fn from_text(text: &str) -> Option<Self> {
        text.parse().ok()
    }
}

impl SignalText for bool {
    fn to_text(&self) -> String {
        if *self { "1" } else { "0" }.to_owned()
    }

    fn from_text(text: &str) -> Option<Self> {
        match text {
            "1" => Some(true),
            "0" => Some(false),
            _ => None,
        }
    }
}

impl SignalText for NaiveDate {
    fn to_text(&self) -> String {
        self.format("%Y-%m-%d").to_string()
    }

    fn from_text(text: &str) -> Option<Self> {
        NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
    }
}

impl SignalText for String {
    fn to_text(&self) -> String {
        self.clone()
    }

    fn from_text(text: &str) -> Option<Self> {
        Some(text.to_owned())
    }
}

impl SignalText for Affiliation {
    fn to_text(&self) -> String {
        match self {
            Affiliation::Immediate => "immediate",
            Affiliation::OnRequest => "on_request",
        }
        .to_owned()
    }

    fn from_text(text: &str) -> Option<Self> {
        match text {
            "immediate" => Some(Affiliation::Immediate),
            "on_request" => Some(Affiliation::OnRequest),
            _ => None,
        }
    }
}

impl SignalText for ProductFormat {
    fn to_text(&self) -> String {
        match self {
            ProductFormat::Physical => "physical",
            ProductFormat::Digital => "digital",
        }
        .to_owned()
    }

    fn from_text(text: &str) -> Option<Self> {
        match text {
            "physical" => Some(ProductFormat::Physical),
            "digital" => Some(ProductFormat::Digital),
            _ => None,
        }
    }
}

impl SignalText for Support {
    fn to_text(&self) -> String {
        match self {
            Support::Good => "good",
            Support::Bad => "bad",
        }
        .to_owned()
    }

    fn from_text(text: &str) -> Option<Self> {
        match text {
            "good" => Some(Support::Good),
            "bad" => Some(Support::Bad),
            _ => None,
        }
    }
}

/// Declares each signal once: its field, its kind, its stored code and its
/// name for the owner.
macro_rules! signals {
    ($($field:ident: $type:ty => $kind:ident, $code:literal, $name:literal;)+) => {
        /// The signals of a Prospect; `None` where nothing told the app.
        #[derive(Debug, Clone, Default, PartialEq, Eq)]
        pub struct Signals {
            $(pub $field: Option<$type>,)+
        }

        /// Which signal.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum SignalKind {
            $($kind,)+
        }

        impl SignalKind {
            pub const ALL: &[SignalKind] = &[$(SignalKind::$kind,)+];

            pub(crate) fn code(self) -> &'static str {
                match self {
                    $(SignalKind::$kind => $code,)+
                }
            }

            pub(crate) fn from_code(code: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|kind| kind.code() == code)
            }

            /// As the owner reads it.
            pub fn name(self) -> &'static str {
                match self {
                    $(SignalKind::$kind => $name,)+
                }
            }
        }

        impl Signals {
            /// The signals it has, each as kept.
            pub(crate) fn texts(&self) -> Vec<(SignalKind, String)> {
                let mut texts = Vec::new();
                $(
                    if let Some(value) = &self.$field {
                        texts.push((SignalKind::$kind, value.to_text()));
                    }
                )+
                texts
            }

            /// Sets `kind` from its kept text; `false` when the text is not
            /// one this version reads.
            pub(crate) fn set_text(&mut self, kind: SignalKind, text: &str) -> bool {
                match kind {
                    $(
                        SignalKind::$kind => {
                            self.$field = SignalText::from_text(text);
                            self.$field.is_some()
                        }
                    )+
                }
            }

            /// The signals of `self` that `other` has with another value,
            /// or does not have.
            pub fn changed_from(&self, other: &Signals) -> Signals {
                Signals {
                    $(
                        $field: self.$field.clone().filter(|value| other.$field.as_ref() != Some(value)),
                    )+
                }
            }
        }
    };
}

signals! {
    price: Money => Price, "price", "preço";
    commission: Money => Commission, "commission", "comissão em R$";
    commission_rate: Percentage => CommissionRate, "commission_rate", "comissão em %";
    temperature: u32 => Temperature, "temperature", "temperatura";
    bars: u32 => Bars, "bars", "BARs";
    blueprint: Percentage => Blueprint, "blueprint", "Blueprint";
    satisfaction: Percentage => Satisfaction, "satisfaction", "satisfação";
    rating: Decimal => Rating, "rating", "nota";
    reviews: u32 => Reviews, "reviews", "avaliações";
    students: u32 => Students, "students", "alunos";
    launched: NaiveDate => Launched, "launched", "entrou na plataforma";
    warranty_days: u32 => WarrantyDays, "warranty_days", "garantia";
    classes: u32 => Classes, "classes", "aulas";
    hours: Decimal => Hours, "hours", "horas";
    certificate: bool => Certificate, "certificate", "certificado";
    video: bool => Video, "video", "vídeo";
    affiliation: Affiliation => Affiliation, "affiliation", "afiliação";
    format: ProductFormat => Format, "format", "formato";
    category: String => Category, "category", "categoria";
    creator: String => Creator, "creator", "produtor";
    creator_since: NaiveDate => CreatorSince, "creator_since", "produtor desde";
    creator_verified: bool => CreatorVerified, "creator_verified", "perfil verificado";
    support: Support => Support, "support", "suporte do produtor";
    product_page: String => ProductPage, "product_page", "página do produto";
    affiliation_page: String => AffiliationPage, "affiliation_page", "página de afiliação";
}

/// The highest temperature a Platform shows (Hotmart's 150°).
pub const MAX_TEMPERATURE: u32 = 150;

/// The highest rating, in stars.
const MAX_RATING: Decimal = Decimal::from_parts(5, 0, 0, false, 0);

impl Signals {
    /// The first signal that cannot be true, if any.
    pub(crate) fn impossible(&self) -> Option<SignalKind> {
        let negative = |money: &Option<Money>| money.is_some_and(|money| money.is_negative());
        let page = |link: &Option<String>| {
            link.as_ref()
                .is_some_and(|link| !link.starts_with("https://") && !link.starts_with("http://"))
        };
        if negative(&self.price) {
            Some(SignalKind::Price)
        } else if negative(&self.commission) {
            Some(SignalKind::Commission)
        } else if self
            .temperature
            .is_some_and(|degrees| degrees > MAX_TEMPERATURE)
        {
            Some(SignalKind::Temperature)
        } else if self
            .rating
            .is_some_and(|rating| rating.is_sign_negative() || rating > MAX_RATING)
        {
            Some(SignalKind::Rating)
        } else if self.hours.is_some_and(|hours| hours.is_sign_negative()) {
            Some(SignalKind::Hours)
        } else if page(&self.product_page) {
            Some(SignalKind::ProductPage)
        } else if page(&self.affiliation_page) {
            Some(SignalKind::AffiliationPage)
        } else {
            None
        }
    }
}

/// Where one signal came from and when it took its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalReading {
    pub kind: SignalKind,
    pub origin: SignalOrigin,
    pub read_at: Timestamp,
}
