//! Tables that hold one live row: an owner's choice saved over the last one.

use libsql::{Connection, Row, Value};
use mascate_kernel::{Clock, IdGenerator, Record};

/// The live row's `columns`, or `None` before anything was saved.
pub(crate) async fn load(
    connection: &Connection,
    table: &'static str,
    columns: &[&'static str],
) -> Result<Option<Row>, libsql::Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {} FROM {table} WHERE deleted_at IS NULL ORDER BY updated_at DESC LIMIT 1",
                columns.join(", ")
            ),
            (),
        )
        .await?;
    rows.next().await
}

/// Saves `values` into `columns` of the live row, creating it the first time.
/// Each statement is atomic on its own, so two saves racing on a new database
/// still leave one live row.
pub(crate) async fn save(
    connection: &Connection,
    clock: &dyn Clock,
    ids: &dyn IdGenerator,
    table: &'static str,
    columns: &[&'static str],
    values: Vec<Value>,
) -> Result<(), libsql::Error> {
    let count = columns.len();
    let assignments: Vec<String> = columns
        .iter()
        .zip(1..)
        .map(|(column, at)| format!("{column} = ?{at}"))
        .collect();
    let update = format!(
        "UPDATE {table} SET {}, updated_at = ?{}
         WHERE id = (SELECT id FROM {table}
                     WHERE deleted_at IS NULL ORDER BY updated_at DESC LIMIT 1)",
        assignments.join(", "),
        count + 1
    );
    let placeholders: Vec<String> = (1..=count + 3).map(|at| format!("?{at}")).collect();
    let insert = format!(
        "INSERT INTO {table} ({}, id, created_at, updated_at)
         SELECT {}
         WHERE NOT EXISTS (SELECT 1 FROM {table} WHERE deleted_at IS NULL)",
        columns.join(", "),
        placeholders.join(", ")
    );
    loop {
        let mut params = values.clone();
        params.push(Value::Text(clock.now().to_rfc3339()));
        if connection.execute(&update, params).await? > 0 {
            return Ok(());
        }
        let record = Record::new(ids, clock);
        let mut params = values.clone();
        params.extend([
            Value::Text(record.id.to_string()),
            Value::Text(record.created_at.to_rfc3339()),
            Value::Text(record.updated_at.to_rfc3339()),
        ]);
        if connection.execute(&insert, params).await? > 0 {
            return Ok(());
        }
        // Another save inserted the first row in between: update it.
    }
}
