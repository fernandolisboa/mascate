//! The Magalu affiliate panel reader (#161, ADR 0035 decision 4): the
//! Parceiro Magalu program offers affiliates no API, so the app opens the
//! panel's sales in the installed browser, logged in by the owner, and
//! reads the table it shows into Conversions and their Commissions, keyed
//! on the panel's own order numbers. Built only while its flag is on.
//!
//! The sales page's address, its columns and its statuses come from public
//! descriptions of the panel, not from a real account: the phase 3
//! regression pass (#116) records the real page (see the fixtures' README).

use std::sync::Arc;

use chrono::TimeDelta;
use mascate_affiliate::{AffiliatePlatform, CommissionSource, ReportedConversion};
use mascate_kernel::{PlatformError, Timestamp};
use mascate_platform::{Flag, FlagKind, Phase};

use crate::affiliate_panel::{AffiliatePanel, Column, Panel, PanelState, SalesReport};
use crate::browser::BrowserSession;

/// The Parceiro Magalu panel, where the owner logs in, unless a test points
/// elsewhere: `parceiromagalu.com.br/painel` redirects here.
pub const PANEL_URL: &str = "https://www.magazinevoce.com.br/painel";

/// The reader, off until the owner turns it on (ADR 0006).
pub const MAGALU_AFFILIATE_PANEL: Flag = Flag {
    key: "integrations.magalu_affiliate_panel",
    name: "Leitura do painel do Parceiro Magalu",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "A Magalu não tem API para afiliados, e os termos do Parceiro Magalu não foram lidos \
             (o painel só abre com login, e o site pede uma verificação contra robôs a quem \
             chega de um jeito diferente do comum). O app abre o painel no Edge ou no Chrome, \
             num perfil só do Mascate em que você entrou à mão, e só lê: não clica, não digita \
             senha e não resolve captcha; quando a Magalu pede a verificação, é você quem \
             responde, pela janela.",
    risk: "A Magalu pode bloquear ou encerrar a sua loja de Parceiro Magalu, e com ela as \
           comissões ainda não pagas.",
};

/// The panel's sales, of the one Program, Parceiro Magalu. It goes back
/// 90 days, and a commission changes for 60: the delivery, the buyer's
/// days to give up and the monthly payment.
const REPORT: SalesReport = SalesReport {
    path: "/vendas",
    columns: &[
        (
            Column::Sale,
            &[
                "Pedido",
                "Nº do pedido",
                "Número do pedido",
                "Código do pedido",
                "ID do pedido",
            ],
        ),
        (
            Column::SoldAt,
            &["Data do pedido", "Data da compra", "Data da venda", "Data"],
        ),
        (Column::Product, &["Produto", "Produtos", "Item"]),
        (
            Column::Status,
            &[
                "Status",
                "Status do pedido",
                "Status da comissão",
                "Situação",
            ],
        ),
        (
            Column::Commission,
            &[
                "Comissão",
                "Sua comissão",
                "Valor da comissão",
                "Comissão prevista",
            ],
        ),
    ],
    platform: AffiliatePlatform::Magalu,
    history_days: 90,
    settle_days: 60,
};

/// Magalu's affiliate panel, read through the shared browser.
pub struct MagaluAffiliatePanel {
    panel_url: String,
    panel: Panel,
}

impl MagaluAffiliatePanel {
    /// The panel at `panel_url`, opened by `browser`.
    pub fn new(panel_url: &str, browser: Arc<BrowserSession>) -> Self {
        Self {
            panel_url: panel_url.trim_end_matches('/').to_owned(),
            panel: Panel::new(browser),
        }
    }
}

impl AffiliatePanel for MagaluAffiliatePanel {
    fn state(&self) -> PanelState {
        self.panel.state()
    }

    /// Magalu sends a logged-out browser from the panel to its login, or to
    /// its check of who is asking, and back.
    fn open_login(&self) -> Result<(), PlatformError> {
        self.panel.open_login(&self.panel_url)
    }
}

impl CommissionSource for MagaluAffiliatePanel {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::Magalu
    }

    fn history(&self) -> TimeDelta {
        REPORT.history()
    }

    fn settle_time(&self) -> TimeDelta {
        REPORT.settle_time()
    }

    fn conversions(
        &self,
        _during: std::ops::Range<Timestamp>,
    ) -> Result<Vec<ReportedConversion>, PlatformError> {
        self.panel.conversions(&self.panel_url, &REPORT)
    }
}
