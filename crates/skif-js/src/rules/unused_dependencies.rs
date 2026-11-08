use skif_core::{Category, DependencyKind, Fixability, RuleMeta, Severity};
use skif_lint::Emitter;

use super::JsRule;
use super::support::report_unused_dependencies;
use crate::analysis::JsAnalysis;

/// Metadata of `unused-dependencies`.
pub static META: RuleMeta = RuleMeta::new(
    "unused-dependencies",
    Category::Js,
    Some(Severity::Error),
    Fixability::Unsafe,
    "A package in `dependencies` is never imported, required or run by a script; `optionalDependencies` are not checked.",
);

/// Reports `dependencies` that nothing uses.
///
/// `optionalDependencies` are not checked, as knip does not: they are mostly platform-specific packages that a
/// loader resolves by a computed name, so no source names them.
pub struct UnusedDependencies;

impl JsRule for UnusedDependencies {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        report_unused_dependencies(&META, &[DependencyKind::Dependencies], analysis, out);
    }
}
