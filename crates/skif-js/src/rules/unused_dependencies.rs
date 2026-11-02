use skif_core::{Category, DependencyKind, Fixability, RuleMeta, Severity};
use skif_lint::Emitter;

use super::JsRule;
use super::support::report_unused_dependencies;
use crate::analysis::JsAnalysis;

/// Metadata of `unused-dependencies`.
pub static META: RuleMeta = RuleMeta {
    id: "unused-dependencies",
    category: Category::Js,
    default: Some(Severity::Error),
    fixable: Fixability::Unsafe,
    summary: "A package in `dependencies` is never imported, required or run by a script.",
};

/// Reports `dependencies` and `optionalDependencies` that nothing uses.
pub struct UnusedDependencies;

impl JsRule for UnusedDependencies {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        report_unused_dependencies(
            &META,
            &[DependencyKind::Dependencies, DependencyKind::OptionalDependencies],
            analysis,
            out,
        );
    }
}
