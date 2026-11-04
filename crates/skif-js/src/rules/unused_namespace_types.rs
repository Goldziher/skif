use skif_core::{Category, Fixability, RuleMeta, Severity};
use skif_lint::Emitter;

use super::JsRule;
use super::unused_exports::{Scope, report};
use crate::analysis::JsAnalysis;

/// Metadata of `unused-namespace-types`. Warning like `unused-types`, whose findings it takes over for modules that
/// are only read through a namespace.
pub static META: RuleMeta = RuleMeta::new(
    "unused-namespace-types",
    Category::Js,
    Some(Severity::Warning),
    Fixability::Unsafe,
    "An exported type of a module that is only read through a namespace import is never accessed.",
);

/// Reports exported interfaces and type aliases that no `ns.Type` read of a namespace-only module touches.
pub struct UnusedNamespaceTypes;

impl JsRule for UnusedNamespaceTypes {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        report(&META, Scope::Namespaces, true, analysis, out);
    }
}
