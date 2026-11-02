use skif_core::{Category, Fixability, RuleMeta, Severity};
use skif_lint::Emitter;

use super::JsRule;
use super::unused_exports::report;
use crate::analysis::JsAnalysis;

/// Metadata of `unused-types`.
pub static META: RuleMeta = RuleMeta {
    id: "unused-types",
    category: Category::Js,
    default: Some(Severity::Error),
    fixable: Fixability::Unsafe,
    summary: "An exported type or interface is not imported by any other module.",
};

/// Reports exported interfaces and type aliases that no module imports.
pub struct UnusedTypes;

impl JsRule for UnusedTypes {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        report(&META, true, analysis, out);
    }
}
