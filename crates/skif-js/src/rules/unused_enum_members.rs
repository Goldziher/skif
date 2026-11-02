use skif_core::{Category, Fixability, RuleMeta, Severity};
use skif_js_syntax::ExportKind;
use skif_lint::Emitter;

use super::JsRule;
use super::members::report;
use crate::analysis::JsAnalysis;

/// Metadata of `unused-enum-members`.
pub static META: RuleMeta = RuleMeta {
    id: "unused-enum-members",
    category: Category::Js,
    default: Some(Severity::Warning),
    fixable: Fixability::None,
    summary: "A member of an exported enum is never read.",
};

/// Reports members of used enums that no module reads.
pub struct UnusedEnumMembers;

impl JsRule for UnusedEnumMembers {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        report(
            &META,
            ExportKind::Enum,
            |export| export.enum_members.as_slice(),
            "enum",
            analysis,
            out,
        );
    }
}
