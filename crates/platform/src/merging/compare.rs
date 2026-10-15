//! "Comparar com outro banco" (ADR 0032, decisions 2 to 6): what a Merge
//! Import would bring from another database, worked out with nothing
//! written to either one.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use libsql::Value;
use mascate_kernel::{Clock, IdGenerator, Timestamp};

use super::coverage::{MergeGap, merge_gaps, ordered, table_columns_in};
use super::declarations::{Condition, KeyPart, ModuleMerge, RecognisedBy, TableMerge, UniqueKey};
use crate::backups::{
    check_restore, copy_database, is_database_file, remove_database, text, with_suffix,
};
use crate::{
    BackupError, Database, DatabaseError, MigrationError, ModuleMigrations, RestoreError, migrate,
    read_stored,
};

/// What a Merge Import would do, per kind of record, in the order the
/// modules declare them. Kinds with nothing to say are left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MergeReport {
    pub records: Vec<RecordsReport>,
}

impl MergeReport {
    /// Nothing new and nothing different: merging would change nothing.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// One kind of record of one module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordsReport {
    pub module: &'static str,
    pub noun: &'static str,
    /// Records only the other database has, deleted ones too, that come in.
    pub new: usize,
    /// Records here that the other database's newer version replaces.
    pub changed_here: usize,
    /// The records on both sides with different contents, by label.
    pub conflicts: Vec<MergeConflict>,
    /// The other database's records that cannot come in, by label.
    pub left_out: Vec<LeftOut>,
}

/// A Merge Conflict: one record, different on each side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeConflict {
    pub label: String,
    pub here: ConflictSide,
    pub there: ConflictSide,
    pub winner: Side,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConflictSide {
    /// When this side last changed the record; `None` when the stored time
    /// does not read as one.
    pub changed_at: Option<Timestamp>,
    pub deleted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    /// This machine's database.
    Here,
    /// The other database.
    There,
}

/// A record of the other database that stays out, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeftOut {
    pub label: String,
    pub reason: LeftOutReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeftOutReason {
    /// Another live record already holds one of its unique keys, such as
    /// a SKU.
    KeyTaken { by: String },
    /// It points to a record of this kind that is in neither database.
    MissingRecord { noun: &'static str },
}

#[derive(Debug, thiserror::Error)]
pub enum MergeError {
    #[error(transparent)]
    Refused(#[from] RestoreError),
    #[error("{0} is this app's own database")]
    OwnDatabase(PathBuf),
    #[error("the Merge Import leaves out part of the database: {}", gaps(.0))]
    Undeclared(Vec<MergeGap>),
    #[error("could not copy the other database: {0}")]
    Copy(#[from] BackupError),
    #[error("could not open the copy of the other database: {0}")]
    Open(#[from] DatabaseError),
    #[error("could not bring the copy of the other database up to date: {0}")]
    Migration(#[from] MigrationError),
    #[error(transparent)]
    Sql(#[from] libsql::Error),
}

fn gaps(gaps: &[MergeGap]) -> String {
    gaps.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// Merge Imports into one database, from files of the same app or an
/// older one.
pub struct MergeImport {
    database: Arc<Database>,
    /// Every module's migrations, to check and bring the other file up to date.
    modules: &'static [ModuleMigrations],
    merges: &'static [ModuleMerge],
    clock: Arc<dyn Clock>,
    ids: Arc<dyn IdGenerator>,
}

impl MergeImport {
    pub fn new(
        database: Arc<Database>,
        modules: &'static [ModuleMigrations],
        merges: &'static [ModuleMerge],
        clock: Arc<dyn Clock>,
        ids: Arc<dyn IdGenerator>,
    ) -> Self {
        Self {
            database,
            modules,
            merges,
            clock,
            ids,
        }
    }

    /// What merging `file` into this database would do. `file` goes through
    /// the Restore's check; a copy of it, brought up to date beside the
    /// database, is read and removed. Neither database is written.
    pub async fn compare(&self, file: &Path) -> Result<MergeReport, MergeError> {
        if is_database_file(self.database.path(), file) {
            return Err(MergeError::OwnDatabase(file.to_path_buf()));
        }
        let gaps = merge_gaps(self.database.connection(), self.merges).await?;
        if !gaps.is_empty() {
            return Err(MergeError::Undeclared(gaps));
        }
        let copy = self.migrated_copy(file).await?;
        let connection = self.database.connect_for_transaction().await?;
        // Values from the other file are data: no SQL function runs from
        // its schema.
        connection
            .execute("PRAGMA trusted_schema = OFF", ())
            .await?;
        connection
            .execute("ATTACH DATABASE ?1 AS incoming", [text(&copy.0)?])
            .await?;
        let compared = compare_attached(&connection, self.merges).await;
        let detached = connection.execute("DETACH DATABASE incoming", ()).await;
        // Closed before the copy goes: Windows keeps an open file.
        drop(connection);
        drop(copy);
        let report = compared?;
        detached?;
        Ok(report)
    }

    /// Checks `file` as a Restore does, copies it beside the database and
    /// runs this app's migrations on the copy; the file stays as it was.
    async fn migrated_copy(&self, file: &Path) -> Result<TemporaryCopy, MergeError> {
        let checked = check_restore(self.modules, file).await?;
        let copy = TemporaryCopy(
            self.database
                .path()
                .with_file_name(format!(".mascate-merge-{}.db", self.ids.next_id())),
        );
        copy_database(&checked.connection, &copy.0, &with_suffix(&copy.0, ".tmp")).await?;
        drop(checked);
        let database = Database::open(&copy.0).await?;
        database
            .connection()
            .execute("PRAGMA trusted_schema = OFF", ())
            .await?;
        migrate(&database, self.clock.as_ref(), self.modules).await?;
        Ok(copy)
    }
}

/// A copy of the other database, removed with its journals when dropped.
struct TemporaryCopy(PathBuf);

impl Drop for TemporaryCopy {
    fn drop(&mut self) {
        let _ = remove_database(&self.0);
    }
}

/// Compares `main` with the database attached as `incoming`, in one read
/// snapshot of both.
async fn compare_attached(
    connection: &libsql::Connection,
    merges: &[ModuleMerge],
) -> Result<MergeReport, MergeError> {
    let declared: Vec<&TableMerge> = merges.iter().flat_map(|module| module.tables).collect();
    let tables =
        ordered(&declared).map_err(|cycle| MergeError::Undeclared(vec![MergeGap::Cycle(cycle)]))?;
    let transaction = connection.transaction().await?;
    let mut state = State::default();
    let mut reports: HashMap<&str, RecordsReport> = HashMap::new();
    for table in tables {
        let module = merges
            .iter()
            .find(|module| module.tables.iter().any(|t| t.table == table.table))
            .map_or("", |module| module.module);
        let report = compare_table(&transaction, module, table, &mut state).await?;
        reports.insert(table.table, report);
    }
    transaction.rollback().await?;
    let records = declared
        .iter()
        .filter_map(|table| reports.remove(table.table))
        .filter(|report| {
            report.new > 0
                || report.changed_here > 0
                || !report.conflicts.is_empty()
                || !report.left_out.is_empty()
        })
        .collect();
    Ok(MergeReport { records })
}

/// One stored value, comparable and hashable.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Cell {
    Null,
    Integer(i64),
    /// The bits of the float, so equal values compare equal.
    Real(u64),
    Text(String),
    Blob(Vec<u8>),
}

impl From<Value> for Cell {
    fn from(value: Value) -> Self {
        match value {
            Value::Null => Cell::Null,
            Value::Integer(integer) => Cell::Integer(integer),
            Value::Real(real) => Cell::Real(real.to_bits()),
            Value::Text(text) => Cell::Text(text),
            Value::Blob(blob) => Cell::Blob(blob),
        }
    }
}

impl Cell {
    fn text(&self) -> Option<&str> {
        match self {
            Cell::Text(text) => Some(text),
            _ => None,
        }
    }

    fn shown(&self) -> Option<String> {
        match self {
            Cell::Null | Cell::Blob(_) => None,
            Cell::Integer(integer) => Some(integer.to_string()),
            Cell::Real(bits) => Some(f64::from_bits(*bits).to_string()),
            Cell::Text(text) => Some(text.clone()),
        }
    }

    fn time(&self) -> Option<Timestamp> {
        self.text().and_then(read_stored)
    }
}

type Row = Vec<Cell>;

/// The rows of one table and where each column sits.
struct Table {
    columns: Vec<String>,
    rows: Vec<Row>,
}

impl Table {
    fn at(&self, column: &str) -> usize {
        self.columns
            .iter()
            .position(|known| known == column)
            .expect("declared columns exist: merge_gaps checked them")
    }
}

/// What earlier tables left for the ones that point to them.
#[derive(Default)]
struct State {
    /// Per table, every id the merged table would hold, with its label.
    known: HashMap<&'static str, Known>,
    /// Per table, the other database's id of a record recognised by key,
    /// and this machine's id it becomes.
    swaps: HashMap<&'static str, HashMap<String, String>>,
}

struct Known {
    noun: &'static str,
    labels: HashMap<String, String>,
}

/// What becomes of one row of the other database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fate {
    /// Only the other database has it.
    New,
    /// The same record as this local row, identical or older.
    Kept(usize),
    /// The same record as this local row, different and newer there.
    Replaces(usize),
}

async fn read(
    connection: &libsql::Connection,
    schema: &str,
    table: &str,
    columns: &[String],
) -> Result<Vec<Row>, libsql::Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {} FROM {schema}.{table} ORDER BY created_at, id",
                columns.join(", ")
            ),
            (),
        )
        .await?;
    let mut read = Vec::new();
    while let Some(row) = rows.next().await? {
        let mut cells = Vec::with_capacity(columns.len());
        for at in 0..columns.len() {
            cells.push(Cell::from(row.get_value(at as i32)?));
        }
        read.push(cells);
    }
    Ok(read)
}

async fn compare_table(
    connection: &libsql::Connection,
    module: &'static str,
    table: &TableMerge,
    state: &mut State,
) -> Result<RecordsReport, libsql::Error> {
    let columns = table_columns_in(connection, "main", table.table).await?;
    let local = Table {
        rows: read(connection, "main", table.table, &columns).await?,
        columns: columns.clone(),
    };
    let mut incoming = Table {
        rows: read(connection, "incoming", table.table, &columns).await?,
        columns,
    };
    let mut report = RecordsReport {
        module,
        noun: table.noun,
        new: 0,
        changed_here: 0,
        conflicts: Vec::new(),
        left_out: Vec::new(),
    };
    if matches!(table.recognised_by, RecognisedBy::ThisMachine) {
        let labels = local_labels(table, &local, state);
        state.known.insert(
            table.table,
            Known {
                noun: table.noun,
                labels,
            },
        );
        return Ok(report);
    }

    swap_references(table, &mut incoming, state);
    let matches = recognise(table, &local, &incoming);
    let id = local.at("id");
    let swaps: HashMap<String, String> = matches
        .iter()
        .enumerate()
        .filter_map(|(at, matched)| {
            let local_id = local.rows[(*matched)?][id].text()?;
            let incoming_id = incoming.rows[at][id].text()?;
            (local_id != incoming_id).then(|| (incoming_id.to_owned(), local_id.to_owned()))
        })
        .collect();
    state.swaps.insert(table.table, swaps);

    let mut fates = Vec::with_capacity(incoming.rows.len());
    for (at, matched) in matches.iter().enumerate() {
        let Some(here) = *matched else {
            fates.push(Fate::New);
            continue;
        };
        let (local_row, incoming_row) = (&local.rows[here], &incoming.rows[at]);
        if table.immutable || same_contents(&local, local_row, incoming_row) {
            fates.push(Fate::Kept(here));
            continue;
        }
        let winner = winner(&local, local_row, incoming_row);
        report.conflicts.push(MergeConflict {
            label: label(table, &incoming, incoming_row, state),
            here: side(&local, local_row),
            there: side(&local, incoming_row),
            winner,
        });
        fates.push(match winner {
            Side::Here => Fate::Kept(here),
            Side::There => Fate::Replaces(here),
        });
    }

    let mut left_out: Vec<Option<LeftOutReason>> = vec![None; fates.len()];
    check_references(table, &local, &incoming, &fates, state, &mut left_out);
    check_keys(table, &local, &incoming, &fates, state, &mut left_out);

    for (at, (fate, out)) in fates.iter().zip(&left_out).enumerate() {
        match (fate, out) {
            (Fate::Kept(_), _) => {}
            (_, Some(reason)) => report.left_out.push(LeftOut {
                label: label(table, &incoming, &incoming.rows[at], state),
                reason: reason.clone(),
            }),
            (Fate::New, None) => report.new += 1,
            (Fate::Replaces(_), None) => report.changed_here += 1,
        }
    }
    report.conflicts.sort_by(|a, b| a.label.cmp(&b.label));
    report.left_out.sort_by(|a, b| a.label.cmp(&b.label));

    let labels = merged_rows(&local, &incoming, &fates, &left_out)
        .into_iter()
        .filter_map(|(id, row)| Some((id.text()?.to_owned(), label(table, &local, row, state))))
        .collect();
    state.known.insert(
        table.table,
        Known {
            noun: table.noun,
            labels,
        },
    );
    Ok(report)
}

fn local_labels(table: &TableMerge, local: &Table, state: &State) -> HashMap<String, String> {
    let id = local.at("id");
    local
        .rows
        .iter()
        .filter_map(|row| Some((row[id].text()?.to_owned(), label(table, local, row, state))))
        .collect()
}

/// The rows the table would hold after the merge, each by the id it
/// would keep: this machine's, each replaced by the other's newer version
/// unless it stays out, then the new records that come in.
fn merged_rows<'a>(
    local: &'a Table,
    incoming: &'a Table,
    fates: &[Fate],
    left_out: &[Option<LeftOutReason>],
) -> Vec<(&'a Cell, &'a Row)> {
    let id = local.at("id");
    let mut rows: Vec<(&Cell, &Row)> = local.rows.iter().map(|row| (&row[id], row)).collect();
    for (at, fate) in fates.iter().enumerate() {
        if left_out[at].is_some() {
            continue;
        }
        match fate {
            Fate::Replaces(here) => rows[*here].1 = &incoming.rows[at],
            Fate::New => rows.push((&incoming.rows[at][id], &incoming.rows[at])),
            Fate::Kept(_) => {}
        }
    }
    rows
}

/// Rewrites the ids the other database holds of records recognised by key
/// to this machine's ids, so its rows compare and point as these do.
fn swap_references(table: &TableMerge, incoming: &mut Table, state: &State) {
    for reference in table.references {
        let Some(swaps) = state.swaps.get(reference.table) else {
            continue;
        };
        let at = incoming.at(reference.column);
        let when = reference.when.map(|when| (incoming.at(when.column), when));
        for row in &mut incoming.rows {
            if when.is_some_and(|(column, when)| !meets(&row[column], when)) {
                continue;
            }
            let swapped = row[at].text().and_then(|id| swaps.get(id));
            if let Some(swapped) = swapped {
                row[at] = Cell::Text(swapped.clone());
            }
        }
    }
}

fn meets(cell: &Cell, condition: Condition) -> bool {
    cell.text()
        .is_some_and(|value| condition.values.contains(&value))
}

/// For each row of the other database, the local row that is the same
/// record, if any (ADR 0032, decisions 3 and 4).
fn recognise(table: &TableMerge, local: &Table, incoming: &Table) -> Vec<Option<usize>> {
    let id = local.at("id");
    let by_id: HashMap<&Cell, usize> = local
        .rows
        .iter()
        .enumerate()
        .map(|(at, row)| (&row[id], at))
        .collect();
    let mut matches: Vec<Option<usize>> = incoming
        .rows
        .iter()
        .map(|row| by_id.get(&row[id]).copied())
        .collect();
    let mut taken = vec![false; local.rows.len()];
    for here in matches.iter().flatten() {
        taken[*here] = true;
    }

    match table.recognised_by {
        RecognisedBy::Id | RecognisedBy::ThisMachine => {}
        RecognisedBy::Keys(keys) => {
            for key in keys {
                let index: HashMap<Row, usize> = local
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(at, _)| !taken[*at])
                    .filter_map(|(at, row)| Some((key_of(key, local, row)?, at)))
                    .collect();
                for (at, row) in incoming.rows.iter().enumerate() {
                    if matches[at].is_some() {
                        continue;
                    }
                    let found = key_of(key, incoming, row).and_then(|key| index.get(&key));
                    if let Some(&here) = found.filter(|&&here| !taken[here]) {
                        matches[at] = Some(here);
                        taken[here] = true;
                    }
                }
            }
        }
        RecognisedBy::SingleRow => {
            let there = live_row(incoming, |at| matches[at].is_none());
            let here = live_row(local, |at| !taken[at]);
            if let (Some(there), Some(here)) = (there, here) {
                matches[there] = Some(here);
            }
        }
        RecognisedBy::Pairs { when, key } => {
            let paired = |table: &Table, row: &Row| {
                let at_when = when.map(|when| (table.at(when.column), when));
                if at_when.is_some_and(|(column, when)| !meets(&row[column], when)) {
                    return None;
                }
                Some(
                    key.iter()
                        .map(|column| row[table.at(column)].clone())
                        .collect::<Row>(),
                )
            };
            // Rows were read in the order they were recorded.
            let mut waiting: HashMap<Row, VecDeque<usize>> = HashMap::new();
            for (at, row) in local.rows.iter().enumerate() {
                if let Some(key) = paired(local, row).filter(|_| !taken[at]) {
                    waiting.entry(key).or_default().push_back(at);
                }
            }
            for (at, row) in incoming.rows.iter().enumerate() {
                if matches[at].is_some() {
                    continue;
                }
                let next = paired(incoming, row)
                    .and_then(|key| waiting.get_mut(&key))
                    .and_then(VecDeque::pop_front);
                if let Some(here) = next {
                    matches[at] = Some(here);
                }
            }
        }
    }
    matches
}

/// The value of `key` in `row`; `None` when the row holds no key: deleted
/// under a partial index, or NULL in a plain column.
fn key_of(key: &UniqueKey, table: &Table, row: &Row) -> Option<Row> {
    if key.live_only && row[table.at("deleted_at")] != Cell::Null {
        return None;
    }
    key.parts
        .iter()
        .map(|part| {
            let cell = &row[table.at(part.column())];
            match (part, cell) {
                (KeyPart::NullAsEmpty(_), Cell::Null) => Some(Cell::Text(String::new())),
                (_, Cell::Null) => None,
                (KeyPart::IgnoreCase(_), Cell::Text(text)) => {
                    Some(Cell::Text(text.to_ascii_lowercase()))
                }
                _ => Some(cell.clone()),
            }
        })
        .collect()
}

/// The live row a `single_row` table reads: the newest not deleted.
fn live_row(table: &Table, eligible: impl Fn(usize) -> bool) -> Option<usize> {
    let (deleted, updated) = (table.at("deleted_at"), table.at("updated_at"));
    table
        .rows
        .iter()
        .enumerate()
        .filter(|(at, row)| eligible(*at) && row[deleted] == Cell::Null)
        .max_by(|(_, a), (_, b)| a[updated].time().cmp(&b[updated].time()))
        .map(|(at, _)| at)
}

/// Whether two rows of one record hold the same: every column but the id
/// and the times it was created and last saved.
fn same_contents(table: &Table, here: &Row, there: &Row) -> bool {
    table
        .columns
        .iter()
        .zip(here.iter().zip(there))
        .filter(|(column, _)| !matches!(column.as_str(), "id" | "created_at" | "updated_at"))
        .all(|(_, (here, there))| here == there)
}

/// The newer change wins the whole row; an exact tie, or a time that does
/// not read, keeps this machine's.
fn winner(table: &Table, here: &Row, there: &Row) -> Side {
    let updated = table.at("updated_at");
    match (here[updated].time(), there[updated].time()) {
        (Some(here), Some(there)) if there > here => Side::There,
        (None, Some(_)) => Side::There,
        _ => Side::Here,
    }
}

fn side(table: &Table, row: &Row) -> ConflictSide {
    ConflictSide {
        changed_at: row[table.at("updated_at")].time(),
        deleted: row[table.at("deleted_at")] != Cell::Null,
    }
}

/// Leaves out each row that would come in pointing to a record in neither
/// database. Tables pointed to were compared first, so what they hold is
/// known; a table pointing to itself holds its own rows and those coming in.
fn check_references(
    table: &TableMerge,
    local: &Table,
    incoming: &Table,
    fates: &[Fate],
    state: &State,
    left_out: &mut [Option<LeftOutReason>],
) {
    let id = local.at("id");
    let own: HashSet<&str> = local
        .rows
        .iter()
        .chain(&incoming.rows)
        .filter_map(|row| row[id].text())
        .collect();
    for (at, fate) in fates.iter().enumerate() {
        if matches!(fate, Fate::Kept(_)) {
            continue;
        }
        let row = &incoming.rows[at];
        for reference in table.references {
            if reference
                .when
                .is_some_and(|when| !meets(&row[incoming.at(when.column)], when))
            {
                continue;
            }
            let Some(target) = row[incoming.at(reference.column)].text() else {
                continue;
            };
            let (exists, noun) = if reference.table == table.table {
                (own.contains(target), table.noun)
            } else {
                match state.known.get(reference.table) {
                    Some(known) => (known.labels.contains_key(target), known.noun),
                    None => (false, table.noun),
                }
            };
            if !exists {
                left_out[at] = Some(LeftOutReason::MissingRecord { noun });
                break;
            }
        }
    }
}

/// Leaves out each row that would come in holding a unique key another
/// live record keeps, until the merged table holds every key once: the
/// write must never fail on an index.
fn check_keys(
    table: &TableMerge,
    local: &Table,
    incoming: &Table,
    fates: &[Fate],
    state: &State,
    left_out: &mut [Option<LeftOutReason>],
) {
    loop {
        let mut excluded = false;
        for key in table.keys() {
            // Each holder: the incoming row written, or the local row kept.
            let mut holders: HashMap<Row, Vec<(Option<usize>, &Row)>> = HashMap::new();
            let mut replaced = vec![false; local.rows.len()];
            for (at, fate) in fates.iter().enumerate() {
                if left_out[at].is_some() {
                    continue;
                }
                let row = &incoming.rows[at];
                match fate {
                    Fate::Kept(_) => continue,
                    Fate::Replaces(here) => replaced[*here] = true,
                    Fate::New => {}
                }
                if let Some(value) = key_of(key, incoming, row) {
                    holders.entry(value).or_default().push((Some(at), row));
                }
            }
            for (here, row) in local.rows.iter().enumerate() {
                if replaced[here] {
                    continue;
                }
                if let Some(value) = key_of(key, local, row) {
                    holders.entry(value).or_default().push((None, row));
                }
            }
            for group in holders.values().filter(|group| group.len() > 1) {
                // A record kept here stays; among records coming in, the
                // first recorded does.
                let stays = group
                    .iter()
                    .find(|(written, _)| written.is_none())
                    .or_else(|| group.iter().min_by_key(|(written, _)| *written))
                    .expect("a group has rows");
                let by = label(table, local, stays.1, state);
                for (written, _) in group {
                    if let Some(at) = written.filter(|at| Some(*at) != stays.0) {
                        left_out[at] = Some(LeftOutReason::KeyTaken { by: by.clone() });
                        excluded = true;
                    }
                }
            }
        }
        if !excluded {
            return;
        }
    }
}

/// How the owner reads the record: its label columns, a reference read as
/// the label of the record it points to.
fn label(table: &TableMerge, rows: &Table, row: &Row, state: &State) -> String {
    let parts: Vec<String> = table
        .label
        .iter()
        .filter_map(|column| {
            let cell = &row[rows.at(column)];
            let target = table
                .references
                .iter()
                .find(|reference| {
                    reference.column == *column
                        && reference.table != table.table
                        && reference
                            .when
                            .is_none_or(|when| meets(&row[rows.at(when.column)], when))
                })
                .and_then(|reference| state.known.get(reference.table));
            match target {
                Some(known) => cell.text().and_then(|id| known.labels.get(id)).cloned(),
                None => cell.shown(),
            }
        })
        .filter(|part| !part.trim().is_empty())
        .collect();
    parts.join(" · ")
}
