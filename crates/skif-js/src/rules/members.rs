//! Shared by the enum and namespace member rules.

use skif_core::{Fix, FixSafety, RuleMeta, diagnostic};
use skif_js_syntax::{Export, ExportKind, Member};
use skif_lint::Emitter;

use super::support::{delete_range, module_location};
use crate::analysis::JsAnalysis;

/// Reports members of exports of `kind` that are used nowhere.
///
/// The export itself must be used; unused exports are reported by `unused-exports`. The tags that exempt an export
/// (`@public`, `@beta`, `@alpha`, `@alias`, `@internal` in production mode and the `ignore-tags` of `unused-exports`)
/// exempt the whole enum or namespace when on the export, and one member when on the member. A member read in the
/// declaring file (`E.A`, `E["A"]`, `const { A } = E`, a namespace member's unshadowed bare name inside the namespace)
/// counts as a use. Using the whole enum there (`Object.values(E)`, `E[key]`, a spread, a bare reference, `typeof E`,
/// `${E}` in a template literal type) uses every member of it (ADR 0018); a type annotation `x: E` uses none.
///
/// The fix deletes the member ([`Member::removal`]); a member whose name another member mentions gets none.
pub(super) fn report(
    meta: &'static RuleMeta,
    kind: ExportKind,
    members_of: fn(&Export) -> &[Member],
    noun: &str,
    analysis: &JsAnalysis<'_>,
    out: &mut Emitter<'_>,
) {
    for module in analysis.graph.modules() {
        if !analysis.reports_exports_of(module) {
            continue;
        }
        for export in module.info.exports.iter().filter(|export| export.kind == kind) {
            if analysis.is_tagged_public(export) || !analysis.exports.is_used(module.id, &export.name) {
                continue;
            }
            for member in members_of(export) {
                if analysis.exempts_tags(&member.tags) || analysis.member_in_use(module, export, member) {
                    continue;
                }
                let mut builder = diagnostic(
                    meta,
                    format!("{noun} member `{}.{}` is never used", export.name, member.name),
                    module_location(module, Some(member.span)),
                )
                .help("remove the member, or tag it `@public`");
                if let Some(package) = analysis.workspace.package(module.owner) {
                    builder = builder.package(package.display_name());
                }
                if let Some((start, end)) = member.removal {
                    builder = builder.fix(Fix::new(
                        format!("Remove the {noun} member `{}`", member.name),
                        FixSafety::Unsafe,
                        vec![delete_range(module, start, end)],
                    ));
                }
                out.emit(builder.build());
            }
        }
    }
}
