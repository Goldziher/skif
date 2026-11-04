use skif_core::{Category, Fixability, RuleMeta, Severity};
use skif_lint::Emitter;

use super::JsRule;
use super::unused_exports::{Scope, report};
use crate::analysis::JsAnalysis;

/// Metadata of `unused-namespace-exports`. Warning like `unused-exports`, whose findings it takes over for modules
/// that are only read through a namespace.
pub static META: RuleMeta = RuleMeta::new(
    "unused-namespace-exports",
    Category::Js,
    Some(Severity::Warning),
    Fixability::Unsafe,
    "An exported value of a module that is only read through a namespace import is never accessed.",
);

/// Reports exported values that no `ns.member` read of a namespace-only module touches.
pub struct UnusedNamespaceExports;

impl JsRule for UnusedNamespaceExports {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        report(&META, Scope::Namespaces, false, analysis, out);
    }
}
