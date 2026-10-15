use mascate_platform::RecognisedBy::SingleRow;
use mascate_platform::{ModuleMerge, TableMerge};

/// How this module's tables come together in a Merge Import (ADR 0032).
pub const MERGE: ModuleMerge = ModuleMerge {
    module: "finance",
    tables: &[
        TableMerge::new("finance_tax_settings", "Alíquota de imposto", SingleRow),
        TableMerge::new(
            "finance_volume_settings",
            "Limite de faturamento",
            SingleRow,
        ),
    ],
};
