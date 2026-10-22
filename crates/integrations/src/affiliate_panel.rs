//! What every logged-in affiliate panel reader shares (#160, ADR 0035
//! decision 4): the tables a panel page shows, read through the
//! [`BrowserSession`] by one script that only reads; whether the page is a
//! login instead of the panel; and the panel's state, as its Connection
//! shows it. The login lives in the browser's profile, never in the app.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use mascate_affiliate::CommissionSource;
use mascate_kernel::{PlatformError, fold};
use serde::Deserialize;
use serde_json::Value;

use crate::browser::BrowserSession;
use crate::crawler::asks_who_is_there;

/// Reads every table on the page, by its `<table>` or ARIA roles, as the
/// text of each cell, with the address the page ended on. It reads the
/// page and nothing else.
///
/// To record what a panel shows for its fixture, paste this into the
/// console of the browser's developer tools on the panel's page and copy
/// what it prints with `copy(...)` around it.
pub const TABLES_SCRIPT: &str = r#"(() => {
  const text = (cell) => (cell.innerText || cell.textContent || "").replace(/\s+/g, " ").trim();
  const cells = (row, which) => Array.from(row.querySelectorAll(which)).map(text);
  const tables = Array.from(document.querySelectorAll('table, [role="table"], [role="grid"]'))
    .map((table) => {
      const rows = Array.from(table.querySelectorAll('tr, [role="row"]'));
      const head = rows.find((row) => row.querySelector('th, [role="columnheader"]'));
      return {
        headers: head ? cells(head, 'th, td, [role="columnheader"]') : [],
        rows: rows
          .filter((row) => row !== head)
          .map((row) => cells(row, 'td, th, [role="cell"], [role="gridcell"], [role="rowheader"]'))
          .filter((row) => row.some((cell) => cell !== "")),
      };
    })
    .filter((table) => table.headers.length > 0);
  return { url: location.href, tables };
})()"#;

/// A panel page as the script read it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct PanelPage {
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) tables: Vec<PanelTable>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct PanelTable {
    pub(crate) headers: Vec<String>,
    #[serde(default)]
    pub(crate) rows: Vec<Vec<String>>,
}

impl PanelPage {
    /// Whether the panel sent the browser to a login or to a check of who
    /// is asking, which only the owner answers.
    pub(crate) fn is_login(&self) -> bool {
        let path = self
            .url
            .split_once("://")
            .map_or(self.url.as_str(), |(_, rest)| rest)
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        asks_who_is_there(&path)
    }

    /// The first table whose headers `columns` reads, with what it read.
    pub(crate) fn table<C>(
        &self,
        columns: impl Fn(&[String]) -> Option<C>,
    ) -> Option<(&PanelTable, C)> {
        self.tables
            .iter()
            .find_map(|table| columns(&table.headers).map(|found| (table, found)))
    }
}

/// A header or a status as letters and digits only, accents dropped:
/// `Comissão` and `comissao` are the same.
pub(crate) fn folded(text: &str) -> String {
    text.chars().filter_map(fold).collect()
}

/// Where a panel's reading stands, as its Connection shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelState {
    /// Neither Edge nor Chrome is installed: nothing can open the panel.
    NoBrowser,
    /// The browser's profile was never opened, so there is no login yet.
    NeverLoggedIn,
    /// The last reading found a login page: the owner logs in again.
    LoggedOut,
    /// Nothing stands in the way of the next reading.
    Ready,
}

impl PanelState {
    /// Whether the hourly Sync asks the panel: a login page or a missing
    /// browser waits for the owner.
    pub fn can_read(self) -> bool {
        self == PanelState::Ready
    }
}

/// A logged-in affiliate panel read as a [`CommissionSource`], with the
/// state its Connection shows and the window the owner logs in through.
pub trait AffiliatePanel: CommissionSource {
    fn state(&self) -> PanelState;

    /// Opens the panel in the browser's window for the owner to log in by
    /// hand; the next reading tries again.
    fn open_login(&self) -> Result<(), PlatformError>;
}

/// A panel read through the shared browser, which remembers when its last
/// reading found a login page instead of the panel.
pub(crate) struct Panel {
    browser: Arc<BrowserSession>,
    logged_out: AtomicBool,
}

impl Panel {
    pub(crate) fn new(browser: Arc<BrowserSession>) -> Self {
        Self {
            browser,
            logged_out: AtomicBool::new(false),
        }
    }

    pub(crate) fn state(&self) -> PanelState {
        if !self.browser.is_installed() {
            PanelState::NoBrowser
        } else if !self.browser.has_profile() {
            PanelState::NeverLoggedIn
        } else if self.logged_out.load(Ordering::Relaxed) {
            PanelState::LoggedOut
        } else {
            PanelState::Ready
        }
    }

    /// Opens `login` in the browser's window for the owner to log in; the
    /// next reading tries again.
    pub(crate) fn open_login(&self, login: &str) -> Result<(), PlatformError> {
        self.browser.open_window(login)?;
        self.logged_out.store(false, Ordering::Relaxed);
        Ok(())
    }

    /// Reads the tables at `url` once a table is drawn, or the login page:
    /// without a browser, the profile never opened or the login gone, it
    /// says so instead.
    pub(crate) fn read(&self, url: &str) -> Result<PanelPage, PlatformError> {
        match self.state() {
            PanelState::NoBrowser => {
                return Err(PlatformError::Refused(
                    "não há Edge nem Chrome instalado neste computador".into(),
                ));
            }
            PanelState::NeverLoggedIn => return Err(PlatformError::NotConnected),
            PanelState::LoggedOut | PanelState::Ready => {}
        }
        let ready = |value: &Value| {
            PanelPage::deserialize(value)
                .is_ok_and(|page| page.is_login() || !page.tables.is_empty())
        };
        let value = self.browser.read(url, TABLES_SCRIPT, &ready)?;
        let page = PanelPage::deserialize(&value).map_err(|error| {
            PlatformError::Failed(format!(
                "a leitura do painel voltou sem as tabelas: {error}"
            ))
        })?;
        let logged_out = page.is_login();
        self.logged_out.store(logged_out, Ordering::Relaxed);
        if logged_out {
            return Err(PlatformError::Expired);
        }
        Ok(page)
    }
}
