//! Where the owner's Work Context is kept (ADR 0037): the Platform every
//! screen follows. The interface owns what the contexts are; this module
//! keeps the chosen one's code between runs, as it keeps the appearance.

use libsql::Value;
use mascate_kernel::{Clock, IdGenerator};

use crate::{Database, Migration, single_row};

const TABLE: &str = "platform_work_context";
const COLUMNS: &[&str] = &["context"];

pub(crate) const CREATE_WORK_CONTEXT: Migration = Migration {
    version: 7,
    name: "create work context",
    risky: false,
    sql: "CREATE TABLE platform_work_context (
        id         TEXT PRIMARY KEY,
        context    TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        deleted_at TEXT
    );",
};

/// The code of the saved Work Context, or `None` before one was saved.
pub async fn load_work_context(database: &Database) -> Result<Option<String>, libsql::Error> {
    let Some(row) = single_row::load(database.connection(), TABLE, COLUMNS).await? else {
        return Ok(None);
    };
    row.get::<String>(0).map(Some)
}

/// Saves the Work Context's `code` over the one already saved, if any.
pub async fn save_work_context(
    database: &Database,
    clock: &dyn Clock,
    ids: &dyn IdGenerator,
    code: &str,
) -> Result<(), libsql::Error> {
    single_row::save(
        database.connection(),
        clock,
        ids,
        TABLE,
        COLUMNS,
        vec![Value::Text(code.to_owned())],
    )
    .await
}
