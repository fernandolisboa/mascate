//! What Listing Copy reads from Mercado Livre (#17): a category's limits on
//! a listing's text and what buyers search most in it (ADR 0028).

use mascate_kernel::PlatformError;
use mascate_marketing::{CategoryRules, CopyChannel};

use super::answers::{CategoryAnswer, Trend};
use super::sales_channel::path_id;
use super::{MercadoLivre, SITE};

/// Mercado Livre's limits where a category leaves them out: the title's,
/// in most categories, and the description's.
const DEFAULT_RULES: CategoryRules = CategoryRules {
    max_title_chars: 60,
    max_description_chars: 50_000,
};

impl CopyChannel for MercadoLivre {
    fn category_rules(&self, category: &str) -> Result<CategoryRules, PlatformError> {
        let found: CategoryAnswer =
            self.get(&format!("/categories/{}", path_id(category)?), &[])?;
        Ok(CategoryRules {
            max_title_chars: found
                .settings
                .max_title_length
                .unwrap_or(DEFAULT_RULES.max_title_chars),
            max_description_chars: found
                .settings
                .max_description_length
                .unwrap_or(DEFAULT_RULES.max_description_chars),
        })
    }

    fn trends(&self, category: &str) -> Result<Vec<String>, PlatformError> {
        let found: Vec<Trend> = self.get(&format!("/trends/{SITE}/{}", path_id(category)?), &[])?;
        Ok(found.into_iter().map(|trend| trend.keyword).collect())
    }
}
