//! The sales volume limit (story 74): past it, a Reminder about formalizing
//! the business shows, without blocking anything (ADR 0009, ADR 0026).

use std::sync::Arc;

use mascate_kernel::{Clock, Currency, IdGenerator, Money};
use mascate_platform::{
    Database, Migration, Reminder, ReminderShows, ReminderTopic, StoredRow, StoredValueError,
    load_single_row, save_single_row,
};
use rust_decimal::Decimal;

pub(crate) const CREATE_VOLUME_SETTINGS: Migration = Migration {
    version: 2,
    name: "create the sales volume limit",
    risky: false,
    sql: "CREATE TABLE finance_volume_settings (
        id          TEXT PRIMARY KEY,
        sales_limit TEXT NOT NULL,
        currency    TEXT NOT NULL,
        created_at  TEXT NOT NULL,
        updated_at  TEXT NOT NULL,
        deleted_at  TEXT
    );",
};

const TABLE: &str = "finance_volume_settings";
const COLUMNS: &[&str] = &["sales_limit", "currency"];

/// The sales counted against the limit: those of the last 12 months, so the
/// count never starts over in January.
pub const VOLUME_WINDOW_DAYS: i64 = 365;

/// R$ 81.000,00, the yearly revenue ceiling of an MEI when this was
/// written: a known line to think about formalizing, until the owner picks
/// another.
fn default_limit() -> Money {
    Money::new(Decimal::from(81_000), Currency::Brl)
}

pub const SALES_VOLUME: Reminder = Reminder {
    key: "finance.sales_volume",
    topic: ReminderTopic::Fiscal,
    title: "Vendas acima do limite que você definiu",
    text: "As vendas dos últimos 12 meses passaram do limite de Configurações › Finanças. \
           Vale conversar com um contador sobre abrir um CNPJ: o app continua funcionando do \
           mesmo jeito, e o Painel mostra o volume.",
    reappears_after_days: 30,
    shows: ReminderShows::WhenRaised,
};

#[derive(Debug, thiserror::Error)]
pub enum VolumeError {
    #[error("the sales limit is an amount above zero")]
    InvalidLimit,
    #[error(transparent)]
    Stored(#[from] StoredValueError),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

/// The sales of the last 12 months against the owner's limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeCheck {
    pub sold: Money,
    pub limit: Money,
}

impl VolumeCheck {
    /// The sales went past the limit: the Reminder shows.
    pub fn passed(&self) -> bool {
        self.sold.currency() == self.limit.currency() && self.sold.amount() > self.limit.amount()
    }
}

pub struct SalesVolume {
    database: Arc<Database>,
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl SalesVolume {
    pub fn new(database: Arc<Database>, clock: Arc<dyn Clock>, ids: Arc<dyn IdGenerator>) -> Self {
        Self {
            database,
            clock,
            ids,
        }
    }

    /// The limit the sales of the last 12 months are held against; R$
    /// 81.000,00 until the owner sets one.
    pub async fn limit(&self) -> Result<Money, VolumeError> {
        let Some(row) = load_single_row(self.database.connection(), TABLE, COLUMNS).await? else {
            return Ok(default_limit());
        };
        Ok(Money::new(row.decimal_at(0)?, row.currency_at(1)?))
    }

    pub async fn save_limit(&self, limit: Money) -> Result<(), VolumeError> {
        if limit.amount() <= Decimal::ZERO {
            return Err(VolumeError::InvalidLimit);
        }
        save_single_row(
            self.database.connection(),
            self.clock.as_ref(),
            self.ids.as_ref(),
            TABLE,
            COLUMNS,
            vec![
                limit.amount().to_string().into(),
                limit.currency().code().into(),
            ],
        )
        .await?;
        Ok(())
    }

    /// `sold`, the sales of the last 12 months, against the limit.
    pub async fn check(&self, sold: Money) -> Result<VolumeCheck, VolumeError> {
        Ok(VolumeCheck {
            sold,
            limit: self.limit().await?,
        })
    }
}
