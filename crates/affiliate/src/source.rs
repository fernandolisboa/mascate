//! The port a Platform's adapter implements to report the owner's
//! Conversions and Payouts (ADR 0030). It only reads: the Affiliate module
//! picks the window, records what comes back and never asks which Platform
//! answered.

use std::ops::Range;

use chrono::TimeDelta;
use mascate_kernel::{Money, PlatformError, Timestamp};

/// A Platform the owner earns Commissions on. Stored values never change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AffiliatePlatform {
    Hotmart,
    Kiwify,
    Eduzz,
    Monetizze,
    Shopee,
    Amazon,
    MercadoLivre,
    Magalu,
    /// Any Platform without an API or a file: the Program names it.
    Other,
}

impl AffiliatePlatform {
    pub const ALL: [AffiliatePlatform; 9] = [
        AffiliatePlatform::Hotmart,
        AffiliatePlatform::Kiwify,
        AffiliatePlatform::Eduzz,
        AffiliatePlatform::Monetizze,
        AffiliatePlatform::Shopee,
        AffiliatePlatform::Amazon,
        AffiliatePlatform::MercadoLivre,
        AffiliatePlatform::Magalu,
        AffiliatePlatform::Other,
    ];

    pub(crate) fn code(self) -> &'static str {
        match self {
            AffiliatePlatform::Hotmart => "hotmart",
            AffiliatePlatform::Kiwify => "kiwify",
            AffiliatePlatform::Eduzz => "eduzz",
            AffiliatePlatform::Monetizze => "monetizze",
            AffiliatePlatform::Shopee => "shopee",
            AffiliatePlatform::Amazon => "amazon",
            AffiliatePlatform::MercadoLivre => "mercado_livre",
            AffiliatePlatform::Magalu => "magalu",
            AffiliatePlatform::Other => "other",
        }
    }

    pub(crate) fn from_code(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|platform| platform.code() == code)
    }

    /// The one Program of a Platform that has a single one for every
    /// product, as its adapters report it; `None` where each product has
    /// its own.
    pub fn own_program(self) -> Option<ReportedProgram> {
        let (id, name) = match self {
            AffiliatePlatform::Shopee => ("shopee-affiliates", "Shopee Afiliados"),
            AffiliatePlatform::Amazon => ("amazon-associates", "Amazon Associados"),
            AffiliatePlatform::MercadoLivre => {
                ("mercado-livre-affiliates", "Afiliados Mercado Livre")
            }
            AffiliatePlatform::Magalu => ("magalu-affiliates", "Parceiro Magalu"),
            AffiliatePlatform::Hotmart
            | AffiliatePlatform::Kiwify
            | AffiliatePlatform::Eduzz
            | AffiliatePlatform::Monetizze
            | AffiliatePlatform::Other => return None,
        };
        Some(ReportedProgram {
            id: id.into(),
            name: name.into(),
        })
    }

    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            AffiliatePlatform::Hotmart => "Hotmart",
            AffiliatePlatform::Kiwify => "Kiwify",
            AffiliatePlatform::Eduzz => "Eduzz",
            AffiliatePlatform::Monetizze => "Monetizze",
            AffiliatePlatform::Shopee => "Shopee",
            AffiliatePlatform::Amazon => "Amazon",
            AffiliatePlatform::MercadoLivre => "Mercado Livre",
            AffiliatePlatform::Magalu => "Magalu",
            AffiliatePlatform::Other => "Outra",
        }
    }
}

/// Where a Commission stands. No Platform has one of its own: each adapter
/// derives it from the sale's status there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CommissionStatus {
    /// Waiting for the buyer to pay, under review or held.
    Pending,
    /// The buyer paid, within the guarantee or past it.
    Approved,
    /// Passed on to the owner.
    Paid,
    /// The money went back to the buyer: refund, chargeback, fraud.
    Reversed,
    /// The sale never happened, like an expired boleto: there never was a
    /// Commission, so it is never counted as a reversal.
    Cancelled,
}

impl CommissionStatus {
    pub const ALL: [CommissionStatus; 5] = [
        CommissionStatus::Pending,
        CommissionStatus::Approved,
        CommissionStatus::Paid,
        CommissionStatus::Reversed,
        CommissionStatus::Cancelled,
    ];

    pub(crate) fn code(self) -> &'static str {
        match self {
            CommissionStatus::Pending => "pending",
            CommissionStatus::Approved => "approved",
            CommissionStatus::Paid => "paid",
            CommissionStatus::Reversed => "reversed",
            CommissionStatus::Cancelled => "cancelled",
        }
    }

    pub(crate) fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|status| status.code() == code)
    }

    /// As the owner reads it.
    pub fn name(self) -> &'static str {
        match self {
            CommissionStatus::Pending => "pendente",
            CommissionStatus::Approved => "aprovada",
            CommissionStatus::Paid => "paga",
            CommissionStatus::Reversed => "estornada",
            CommissionStatus::Cancelled => "cancelada",
        }
    }

    /// The owner earned it: approved or already paid.
    pub fn is_earned(self) -> bool {
        matches!(self, CommissionStatus::Approved | CommissionStatus::Paid)
    }
}

/// The Program a Conversion belongs to, as the Platform names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportedProgram {
    /// The Platform's id: the product on infoproduct Platforms, the
    /// Platform itself where it has one Program.
    pub id: String,
    pub name: String,
}

/// A Conversion and its Commission, as a Platform reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportedConversion {
    /// What makes it unique on the Platform, built by the adapter: the
    /// transaction on Hotmart, the conversion, order and item on Shopee.
    pub key: String,
    pub program: ReportedProgram,
    /// The product sold, as the Platform names it.
    pub product: String,
    pub sold_at: Timestamp,
    /// What the Platform says the owner gets, in its currency.
    pub commission: Money,
    pub status: CommissionStatus,
    /// The sale's status as the Platform names it.
    pub platform_status: String,
    /// When the Platform says the status changed, when it says.
    pub status_changed_at: Option<Timestamp>,
    /// The marking the link carried (Shopee's subIds, Hotmart's sck, the
    /// Amazon tracking id), when there was one.
    pub tracking: Option<String>,
    /// The Platform's answer for it, kept so a fix to the status
    /// translation can be replayed after the Platform's window closes.
    pub raw: String,
}

/// A transfer of money from a Platform to the owner, as the Platform
/// reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportedPayout {
    /// What makes it unique on the Platform.
    pub key: String,
    pub paid_at: Timestamp,
    pub amount: Money,
    /// The keys of the Conversions it covers, as the Platform says.
    pub covers: Vec<String>,
    pub raw: String,
}

/// How a Platform reports the owner's Conversions and Payouts. Calls block
/// on the network, so they run off the UI thread.
pub trait CommissionSource: Send + Sync {
    fn platform(&self) -> AffiliatePlatform;

    /// How far back the Platform lets the app read.
    fn history(&self) -> TimeDelta;

    /// How long a status still changes there: the guarantee and the release
    /// of the money.
    fn settle_time(&self) -> TimeDelta;

    /// The Conversions in `during`, by the date the Platform filters on.
    fn conversions(
        &self,
        during: Range<Timestamp>,
    ) -> Result<Vec<ReportedConversion>, PlatformError>;

    /// The Payouts in `during`, on Platforms that report them.
    fn payouts(&self, _during: Range<Timestamp>) -> Result<Vec<ReportedPayout>, PlatformError> {
        Ok(Vec::new())
    }
}
