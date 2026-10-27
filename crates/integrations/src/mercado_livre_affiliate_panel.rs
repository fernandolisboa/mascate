//! The Mercado Livre affiliate panel reader (#160, ADR 0035 decision 4):
//! Mercado Livre offers affiliates no API, so the app opens the panel's
//! report in the installed browser, logged in by the owner, and reads the
//! sales table it shows into Conversions and their Commissions, keyed on
//! the panel's own sale ids. Built only while its flag is on.
//!
//! The report's address, its columns and its statuses come from public
//! descriptions of the panel, not from a real account: the phase 3
//! regression pass (#116) records the real page (see the fixtures' README).

use std::sync::Arc;

use chrono::TimeDelta;
use mascate_affiliate::{AffiliatePlatform, CommissionSource, ReportedConversion};
use mascate_kernel::{PlatformError, Timestamp};
use mascate_platform::{Flag, FlagKind, Phase};

use crate::affiliate_panel::{AffiliatePanel, Column, Panel, PanelState, SalesReport};
use crate::browser::BrowserSession;

/// The affiliate panel, where the owner logs in, unless a test points
/// elsewhere.
pub const PANEL_URL: &str = "https://www.mercadolivre.com.br/afiliados";

/// The reader, off until the owner turns it on (ADR 0006).
pub const MERCADO_LIVRE_AFFILIATE_PANEL: Flag = Flag {
    key: "integrations.mercado_livre_affiliate_panel",
    name: "Leitura do painel de afiliado do Mercado Livre",
    kind: FlagKind::RestrictedFeature,
    phase: Phase::Three,
    reason: "O Mercado Livre não tem API para afiliados, e os termos do programa de afiliados \
             não foram lidos (a página só abre com login); os Termos e condições de uso para \
             desenvolvedores (7.6) proíbem robôs. O app abre o painel no Edge ou no Chrome, num \
             perfil só do Mascate em que você entrou à mão, e só lê: não clica, não digita senha \
             e não resolve captcha.",
    risk: "O Mercado Livre pode suspender ou encerrar a sua conta de afiliado, e com ela as \
           comissões ainda não pagas.",
};

/// The panel's report of sales and commissions, of the one Program,
/// Mercado Livre's affiliate program. It goes back 90 days, and a
/// commission changes for 60: the delivery, the buyer's days to give up
/// and the monthly payment.
const REPORT: SalesReport = SalesReport {
    path: "/relatorios",
    columns: &[
        (
            Column::Sale,
            &[
                "ID da venda",
                "Nº da venda",
                "Número da venda",
                "Venda",
                "ID do pedido",
                "Pedido",
            ],
        ),
        (Column::SoldAt, &["Data da venda", "Data da compra", "Data"]),
        (Column::Product, &["Produto", "Anúncio", "Item"]),
        (
            Column::Status,
            &["Status", "Status da comissão", "Situação"],
        ),
        (
            Column::Commission,
            &[
                "Comissão",
                "Sua comissão",
                "Valor da comissão",
                "Ganho",
                "Ganhos",
            ],
        ),
        (Column::Tracking, &["Etiqueta", "Tag", "Etiqueta do link"]),
    ],
    platform: AffiliatePlatform::MercadoLivre,
    history_days: 90,
    settle_days: 60,
};

/// Mercado Livre's affiliate panel, read through the shared browser.
pub struct MercadoLivreAffiliatePanel {
    panel_url: String,
    panel: Panel,
}

impl MercadoLivreAffiliatePanel {
    /// The panel at `panel_url`, opened by `browser`.
    pub fn new(panel_url: &str, browser: Arc<BrowserSession>) -> Self {
        Self {
            panel_url: panel_url.trim_end_matches('/').to_owned(),
            panel: Panel::new(browser),
        }
    }
}

impl AffiliatePanel for MercadoLivreAffiliatePanel {
    fn state(&self) -> PanelState {
        self.panel.state()
    }

    /// Mercado Livre sends a logged-out browser from the panel to its login
    /// and back.
    fn open_login(&self) -> Result<(), PlatformError> {
        self.panel.open_login(&self.panel_url)
    }
}

impl CommissionSource for MercadoLivreAffiliatePanel {
    fn platform(&self) -> AffiliatePlatform {
        AffiliatePlatform::MercadoLivre
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
