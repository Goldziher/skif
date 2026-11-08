use skif_core::{Category, Fixability, RuleMeta, Severity};
use skif_js_syntax::ExportKind;
use skif_lint::Emitter;

use super::JsRule;
use super::members::report;
use crate::analysis::JsAnalysis;

/// Metadata of `unused-namespace-members`.
pub static META: RuleMeta = RuleMeta::new(
    "unused-namespace-members",
    Category::Js,
    Some(Severity::Warning),
    Fixability::Unsafe,
    "An exported member of a TypeScript namespace is never read.",
);

/// Reports exported members of used TypeScript namespaces that no module reads.
pub struct UnusedNamespaceMembers;

impl JsRule for UnusedNamespaceMembers {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        report(
            &META,
            ExportKind::Namespace,
            |export| export.namespace_members.as_slice(),
            "namespace",
            analysis,
            out,
        );
    }
}
