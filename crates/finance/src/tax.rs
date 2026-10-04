//! The tax rate on each sale (story 73), 0% until the owner has a CNPJ to
//! simulate. Every margin the app works out uses it.

use std::sync::Arc;

use mascate_kernel::{Clock, IdGenerator, Percentage};
use mascate_platform::{
    Database, Migration, StoredRow, StoredValueError, load_single_row, save_single_row,
};

pub(crate) const CREATE_TAX_SETTINGS: Migration = Migration {
    version: 1,
    name: "create the tax rate setting",
    risky: false,
    sql: "CREATE TABLE finance_tax_settings (
        id         TEXT PRIMARY KEY,
        rate       TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );",
};

const TABLE: &str = "finance_tax_settings";
const COLUMNS: &[&str] = &["rate"];

#[derive(Debug, thiserror::Error)]
pub enum TaxError {
    #[error(transparent)]
    Stored(#[from] StoredValueError),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

pub struct Taxes {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl Taxes {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            database,
            clock,
            ids,
        }
    }

    /// The tax rate on each sale; zero until the owner sets one.
    pub async fn rate(&self) -> Result<Percentage, TaxError> {
        let Some(row) = load_single_row(self.database.connection(), TABLE, COLUMNS).await? else {
            return Ok(Percentage::ZERO);
        };
        let rate = row.decimal_at(0)?;
        Percentage::new(rate).map_err(|_| StoredValueError::Unreadable(rate.to_string()).into())
    }

    pub async fn save_rate(&self, rate: Percentage) -> Result<(), TaxError> {
        save_single_row(
            self.database.connection(),
            self.clock.as_ref(),
            self.ids.as_ref(),
            TABLE,
            COLUMNS,
            vec![rate.percent().to_string().into()],
        )
        .await?;
        Ok(())
    }
}
