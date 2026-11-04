//! Shared by the enum and namespace member rules.

use skif_core::{RuleMeta, diagnostic};
use skif_js_syntax::{Export, ExportKind, Member};
use skif_lint::Emitter;

use super::support::module_location;
use crate::analysis::JsAnalysis;

/// Reports members of exports of `kind` that are used nowhere.
///
/// The export itself must be used; unused exports are reported by `unused-exports`. A member read in the declaring
/// file (`E.A`) counts as a use. An export that the file uses as a whole (`Object.values(E)`, `E[key]`, a spread,
/// passed as a value) is skipped, because any of its members may be read.
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
            if export.members_escape_in_file
                || export.tags.iter().any(|tag| tag == "public")
                || !analysis.exports.is_used(module.id, &export.name)
            {
                continue;
            }
            for member in members_of(export) {
                if member.tags.iter().any(|tag| tag == "public")
                    || export.members_read_in_file.contains(&member.name)
                    || analysis.exports.is_member_used(module.id, &export.name, &member.name)
                {
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
                out.emit(builder.build());
            }
        }
    }
}
