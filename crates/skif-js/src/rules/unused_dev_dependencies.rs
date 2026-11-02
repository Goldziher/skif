use skif_core::{Category, DependencyKind, Fixability, RuleMeta, Severity};
use skif_lint::Emitter;

use super::JsRule;
use super::support::report_unused_dependencies;
use crate::analysis::JsAnalysis;

/// Metadata of `unused-dev-dependencies`.
pub static META: RuleMeta = RuleMeta {
    id: "unused-dev-dependencies",
    category: Category::Js,
    default: Some(Severity::Error),
    fixable: Fixability::Unsafe,
    summary: "A package in `devDependencies` is never imported, required or run by a script.",
};

/// Reports `devDependencies` that nothing uses. Production mode does not look at them.
pub struct UnusedDevDependencies;

impl JsRule for UnusedDevDependencies {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        if analysis.production() {
            return;
        }
        report_unused_dependencies(&META, &[DependencyKind::DevDependencies], analysis, out);
    }
}
