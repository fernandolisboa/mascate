//! Whether the declarations cover the schema, so a new table or column
//! never stays out of a Merge Import unnoticed.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use libsql::Connection;

use super::declarations::{ModuleMerge, TableMerge};

/// Every record carries these (ADR 0004).
pub(crate) const RECORD_COLUMNS: [&str; 4] = ["id", "created_at", "updated_at", "deleted_at"];

/// Something the declarations leave out or get wrong about the schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeGap {
    UndeclaredTable(String),
    DeclaredTwice(&'static str),
    MissingTable(&'static str),
    NotARecord {
        table: &'static str,
        missing: &'static str,
    },
    MissingColumn {
        table: &'static str,
        column: &'static str,
    },
    /// A column ending in `_id` that is neither a record reference nor a
    /// Platform id, or is declared as both.
    UnclassifiedId {
        table: &'static str,
        column: String,
    },
    UnknownTarget {
        table: &'static str,
        column: &'static str,
        target: &'static str,
    },
    UndeclaredKey {
        table: &'static str,
        index: String,
    },
    KeyWithoutIndex {
        table: &'static str,
        key: String,
    },
    /// Tables whose references go round, so none can be processed first.
    Cycle(Vec<&'static str>),
}

impl fmt::Display for MergeGap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MergeGap::UndeclaredTable(table) => write!(f, "table {table} is not declared"),
            MergeGap::DeclaredTwice(table) => write!(f, "table {table} is declared twice"),
            MergeGap::MissingTable(table) => write!(f, "declared table {table} does not exist"),
            MergeGap::NotARecord { table, missing } => {
                write!(f, "table {table} has no {missing} column")
            }
            MergeGap::MissingColumn { table, column } => {
                write!(f, "declared column {table}.{column} does not exist")
            }
            MergeGap::UnclassifiedId { table, column } => write!(
                f,
                "column {table}.{column} must be either a record reference or a Platform id"
            ),
            MergeGap::UnknownTarget {
                table,
                column,
                target,
            } => write!(
                f,
                "column {table}.{column} points to {target}, which is not declared"
            ),
            MergeGap::UndeclaredKey { table, index } => {
                write!(f, "unique index {index} of {table} is not a declared key")
            }
            MergeGap::KeyWithoutIndex { table, key } => {
                write!(f, "declared key {key} of {table} has no unique index")
            }
            MergeGap::Cycle(tables) => {
                write!(f, "references go round between {}", tables.join(", "))
            }
        }
    }
}

/// What the declarations leave out of the schema of the `main` database
/// behind `connection`, which has every migration; empty when they cover
/// it all.
pub async fn merge_gaps(
    connection: &Connection,
    modules: &[ModuleMerge],
) -> Result<Vec<MergeGap>, libsql::Error> {
    let mut gaps = Vec::new();
    let declared: Vec<&TableMerge> = modules.iter().flat_map(|module| module.tables).collect();
    let mut seen = BTreeSet::new();
    for table in &declared {
        if !seen.insert(table.table) {
            gaps.push(MergeGap::DeclaredTwice(table.table));
        }
    }

    for table in schema_tables(connection).await? {
        if !seen.contains(table.as_str()) {
            gaps.push(MergeGap::UndeclaredTable(table));
        }
    }

    for table in &declared {
        let columns = table_columns(connection, table.table).await?;
        if columns.is_empty() {
            gaps.push(MergeGap::MissingTable(table.table));
            continue;
        }
        check_columns(table, &columns, &seen, &mut gaps);
        check_keys(connection, table, &mut gaps).await?;
    }

    if let Err(cycle) = ordered(&declared) {
        gaps.push(MergeGap::Cycle(cycle));
    }
    Ok(gaps)
}

fn check_columns(
    table: &TableMerge,
    columns: &[String],
    declared: &BTreeSet<&str>,
    gaps: &mut Vec<MergeGap>,
) {
    let has = |column: &str| columns.iter().any(|known| known == column);
    for missing in RECORD_COLUMNS.into_iter().filter(|column| !has(column)) {
        gaps.push(MergeGap::NotARecord {
            table: table.table,
            missing,
        });
    }
    let mut named: Vec<&'static str> = table.named_columns();
    named.sort_unstable();
    named.dedup();
    for column in named.into_iter().filter(|column| !has(column)) {
        gaps.push(MergeGap::MissingColumn {
            table: table.table,
            column,
        });
    }
    for column in columns.iter().filter(|column| column.ends_with("_id")) {
        let reference = table.references.iter().any(|r| r.column == column);
        let platform = table.platform_ids.contains(&column.as_str());
        if reference == platform {
            gaps.push(MergeGap::UnclassifiedId {
                table: table.table,
                column: column.clone(),
            });
        }
    }
    for reference in table.references {
        if !declared.contains(reference.table) {
            gaps.push(MergeGap::UnknownTarget {
                table: table.table,
                column: reference.column,
                target: reference.table,
            });
        }
    }
}

/// Each unique index is a declared key and each declared key an index, so
/// the report recognises a record exactly as the database refuses a second.
async fn check_keys(
    connection: &Connection,
    table: &TableMerge,
    gaps: &mut Vec<MergeGap>,
) -> Result<(), libsql::Error> {
    let indexes = unique_indexes(connection, table.table).await?;
    let declared: Vec<(String, IndexShape)> = table
        .keys()
        .iter()
        .map(|key| {
            let shape = IndexShape {
                parts: key.parts.iter().map(|part| part.sql()).collect(),
                live_only: key.live_only,
            };
            (shape.to_string(), shape)
        })
        .collect();
    for (name, shape) in &indexes {
        if !declared.iter().any(|(_, key)| key == shape) {
            gaps.push(MergeGap::UndeclaredKey {
                table: table.table,
                index: name.clone(),
            });
        }
    }
    for (text, key) in declared {
        if !indexes.values().any(|shape| *shape == key) {
            gaps.push(MergeGap::KeyWithoutIndex {
                table: table.table,
                key: text,
            });
        }
    }
    Ok(())
}

/// The columns of a unique index as its SQL writes them, and whether it is
/// partial on the rows not deleted.
#[derive(Debug, PartialEq, Eq)]
struct IndexShape {
    parts: Vec<String>,
    live_only: bool,
}

impl fmt::Display for IndexShape {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({})", self.parts.join(", "))?;
        if self.live_only {
            write!(f, " WHERE deleted_at IS NULL")?;
        }
        Ok(())
    }
}

async fn unique_indexes(
    connection: &Connection,
    table: &str,
) -> Result<BTreeMap<String, IndexShape>, libsql::Error> {
    let mut listed = Vec::new();
    let mut rows = connection
        .query(&format!("PRAGMA main.index_list({table})"), ())
        .await?;
    while let Some(row) = rows.next().await? {
        let name: String = row.get(1)?;
        let unique: i64 = row.get(2)?;
        let origin: String = row.get(3)?;
        if unique == 1 && origin != "pk" {
            listed.push((name, origin));
        }
    }
    let mut indexes = BTreeMap::new();
    for (name, origin) in listed {
        let shape = if origin == "c" {
            let mut rows = connection
                .query(
                    "SELECT sql FROM main.sqlite_master WHERE type = 'index' AND name = ?1",
                    [name.as_str()],
                )
                .await?;
            let sql: String = match rows.next().await? {
                Some(row) => row.get(0)?,
                None => continue,
            };
            parse_index(&sql)
        } else {
            // A UNIQUE constraint on columns, every row included.
            let mut parts = Vec::new();
            let mut rows = connection
                .query(&format!("PRAGMA main.index_info({name})"), ())
                .await?;
            while let Some(row) = rows.next().await? {
                parts.push(row.get::<String>(2)?);
            }
            IndexShape {
                parts,
                live_only: false,
            }
        };
        indexes.insert(name, shape);
    }
    Ok(indexes)
}

/// Reads `CREATE UNIQUE INDEX name ON table (parts) [WHERE condition]`, as
/// this app's own migrations write it.
fn parse_index(sql: &str) -> IndexShape {
    let squeeze = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let start = sql.find('(').map_or(sql.len(), |at| at + 1);
    let mut depth = 0usize;
    let mut parts = Vec::new();
    let mut part = String::new();
    let mut end = sql.len();
    for (at, character) in sql[start..].char_indices() {
        match character {
            '(' => depth += 1,
            ')' if depth == 0 => {
                end = start + at + 1;
                break;
            }
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(squeeze(&part));
                part.clear();
                continue;
            }
            _ => {}
        }
        part.push(character);
    }
    parts.push(squeeze(&part));
    IndexShape {
        parts,
        live_only: squeeze(&sql[end..]).contains("deleted_at IS NULL"),
    }
}

async fn schema_tables(connection: &Connection) -> Result<Vec<String>, libsql::Error> {
    let mut tables = Vec::new();
    let mut rows = connection
        .query(
            "SELECT name FROM main.sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' AND name != 'schema_migrations'
             ORDER BY name",
            (),
        )
        .await?;
    while let Some(row) = rows.next().await? {
        tables.push(row.get(0)?);
    }
    Ok(tables)
}

/// The columns of `table` in `schema`, in order; empty when it does not exist.
pub(crate) async fn table_columns_in(
    connection: &Connection,
    schema: &str,
    table: &str,
) -> Result<Vec<String>, libsql::Error> {
    let mut columns = Vec::new();
    let mut rows = connection
        .query(&format!("PRAGMA {schema}.table_info({table})"), ())
        .await?;
    while let Some(row) = rows.next().await? {
        columns.push(row.get(1)?);
    }
    Ok(columns)
}

async fn table_columns(connection: &Connection, table: &str) -> Result<Vec<String>, libsql::Error> {
    table_columns_in(connection, "main", table).await
}

/// The declared tables with each one after the tables it points to, in the
/// order declared otherwise; the tables left in a cycle when there is one.
pub(crate) fn ordered<'a>(
    tables: &[&'a TableMerge],
) -> Result<Vec<&'a TableMerge>, Vec<&'static str>> {
    let mut left: Vec<&TableMerge> = tables.to_vec();
    let mut done: Vec<&TableMerge> = Vec::new();
    while !left.is_empty() {
        let ready = left.iter().position(|table| {
            table.references.iter().all(|reference| {
                reference.table == table.table
                    || done.iter().any(|known| known.table == reference.table)
                    || !left.iter().any(|other| other.table == reference.table)
            })
        });
        match ready {
            Some(at) => done.push(left.remove(at)),
            None => return Err(left.iter().map(|table| table.table).collect()),
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_parts_and_liveness_are_read_from_its_sql() {
        let shape = parse_index(
            "CREATE UNIQUE INDEX commerce_listings_by_ml_id
                ON commerce_listings (ml_item_id, COALESCE(ml_variation_id, ''))
                WHERE ml_item_id IS NOT NULL AND deleted_at IS NULL",
        );
        assert_eq!(
            shape,
            IndexShape {
                parts: vec!["ml_item_id".into(), "COALESCE(ml_variation_id, '')".into()],
                live_only: true,
            }
        );
        assert_eq!(
            parse_index("CREATE UNIQUE INDEX s ON catalog_suppliers (name COLLATE NOCASE)"),
            IndexShape {
                parts: vec!["name COLLATE NOCASE".into()],
                live_only: false,
            }
        );
    }
}
