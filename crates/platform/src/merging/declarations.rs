//! What each module declares about its tables for a Merge Import (ADR
//! 0032): how a record is recognised in both databases, which columns hold
//! ids of other records, and how the owner reads it in the report.

/// One module's tables, published next to its migrations.
#[derive(Debug, Clone, Copy)]
pub struct ModuleMerge {
    pub module: &'static str,
    pub tables: &'static [TableMerge],
}

/// How one table comes together with the same table of another database.
#[derive(Debug, Clone, Copy)]
pub struct TableMerge {
    pub table: &'static str,
    /// What the owner calls one record, as the report names it: "Pedido".
    pub noun: &'static str,
    pub recognised_by: RecognisedBy,
    /// Columns that hold the id of another record, of this module or another.
    pub references: &'static [Reference],
    /// Columns ending in `_id` that hold a Platform's id, not a record's.
    pub platform_ids: &'static [&'static str],
    /// The columns that name a record to the owner, in order. A reference
    /// among them reads as the label of the record it points to.
    pub label: &'static [&'static str],
    /// Rows never change once written (the Stock Movement ledger, ADR 0005):
    /// a record recognised on both sides stays as this machine wrote it.
    pub immutable: bool,
}

/// How a record of the other database is recognised as one of this one.
#[derive(Debug, Clone, Copy)]
pub enum RecognisedBy {
    /// Only by the same id: something the owner recorded by hand on one
    /// machine is a fact of that machine.
    Id,
    /// By the same id, then by the same value of one of the unique keys the
    /// table already enforces, in the order given.
    Keys(&'static [UniqueKey]),
    /// The one live row each machine saved (`single_row`).
    SingleRow,
    /// By the same id, then, among the rows that meet `when`, by the same
    /// values of `key`, pairing equal rows one to one in the order they were
    /// recorded: a key no index enforces, where equal rows can repeat.
    Pairs {
        when: Option<Condition>,
        key: &'static [&'static str],
    },
    /// Settings of this machine (a folder on its disk): the other
    /// database's row never comes in.
    ThisMachine,
}

/// The columns of one unique index, as it compares them.
#[derive(Debug, Clone, Copy)]
pub struct UniqueKey {
    pub parts: &'static [KeyPart],
    /// Only rows not deleted hold the key: the index is partial on
    /// `deleted_at IS NULL`.
    pub live_only: bool,
}

impl UniqueKey {
    /// A key among the rows not deleted, like most indexes here.
    pub const fn live(parts: &'static [KeyPart]) -> Self {
        Self {
            parts,
            live_only: true,
        }
    }

    /// A key every row holds, deleted or not: a `UNIQUE` column.
    pub const fn every_row(parts: &'static [KeyPart]) -> Self {
        Self {
            parts,
            live_only: false,
        }
    }
}

/// One column of a unique key. A plain column that is NULL never matches,
/// as SQLite's unique indexes count NULLs as distinct; that is also what an
/// index's `WHERE column IS NOT NULL` says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPart {
    Column(&'static str),
    /// `COALESCE(column, '')`: NULL matches NULL.
    NullAsEmpty(&'static str),
    /// `column COLLATE NOCASE`.
    IgnoreCase(&'static str),
}

impl KeyPart {
    pub(crate) fn column(self) -> &'static str {
        match self {
            KeyPart::Column(column)
            | KeyPart::NullAsEmpty(column)
            | KeyPart::IgnoreCase(column) => column,
        }
    }

    /// The part as the index's SQL writes it.
    pub(crate) fn sql(self) -> String {
        match self {
            KeyPart::Column(column) => column.to_owned(),
            KeyPart::NullAsEmpty(column) => format!("COALESCE({column}, '')"),
            KeyPart::IgnoreCase(column) => format!("{column} COLLATE NOCASE"),
        }
    }
}

/// A column holding the id of a record of `table`, in the rows that meet
/// `when`; one column can point to different tables by row.
#[derive(Debug, Clone, Copy)]
pub struct Reference {
    pub column: &'static str,
    pub table: &'static str,
    pub when: Option<Condition>,
}

impl Reference {
    pub const fn to(column: &'static str, table: &'static str) -> Self {
        Self {
            column,
            table,
            when: None,
        }
    }

    pub const fn when(self, condition: Condition) -> Self {
        Self {
            when: Some(condition),
            ..self
        }
    }
}

/// Rows whose `column` holds one of `values`.
#[derive(Debug, Clone, Copy)]
pub struct Condition {
    pub column: &'static str,
    pub values: &'static [&'static str],
}

impl TableMerge {
    pub const fn new(table: &'static str, noun: &'static str, recognised_by: RecognisedBy) -> Self {
        Self {
            table,
            noun,
            recognised_by,
            references: &[],
            platform_ids: &[],
            label: &[],
            immutable: false,
        }
    }

    pub const fn references(self, references: &'static [Reference]) -> Self {
        Self { references, ..self }
    }

    pub const fn platform_ids(self, platform_ids: &'static [&'static str]) -> Self {
        Self {
            platform_ids,
            ..self
        }
    }

    pub const fn label(self, label: &'static [&'static str]) -> Self {
        Self { label, ..self }
    }

    pub const fn immutable(self) -> Self {
        Self {
            immutable: true,
            ..self
        }
    }

    /// The unique keys this table recognises a record by.
    pub(crate) fn keys(&self) -> &'static [UniqueKey] {
        match self.recognised_by {
            RecognisedBy::Keys(keys) => keys,
            _ => &[],
        }
    }

    /// Every column the declaration names.
    pub(crate) fn named_columns(&self) -> Vec<&'static str> {
        let mut columns: Vec<&'static str> = Vec::new();
        columns.extend(self.references.iter().map(|reference| reference.column));
        columns.extend(
            self.references
                .iter()
                .filter_map(|reference| reference.when.map(|when| when.column)),
        );
        columns.extend(self.platform_ids);
        columns.extend(self.label);
        columns.extend(
            self.keys()
                .iter()
                .flat_map(|key| key.parts.iter().map(|part| part.column())),
        );
        if let RecognisedBy::Pairs { when, key } = self.recognised_by {
            columns.extend(when.map(|when| when.column));
            columns.extend(key);
        }
        columns
    }
}
