//! Merge Import (ADR 0032): bringing another Mascate database into this
//! one record by record. Each module declares its tables; the Platform
//! runs one algorithm over all of them: compare first, then merge.

mod apply;
mod compare;
mod coverage;
mod declarations;

pub(crate) use apply::CREATE_MERGES;
pub use apply::{AppliedMerge, FollowUp};
pub use compare::{
    ConflictSide, LeftOut, LeftOutReason, MergeConflict, MergeError, MergeImport, MergeReport,
    RecordsReport, Side,
};
pub use coverage::{MergeGap, merge_gaps};
pub use declarations::{
    Condition, KeyPart, ModuleMerge, RecognisedBy, Reference, TableMerge, UniqueKey,
};
