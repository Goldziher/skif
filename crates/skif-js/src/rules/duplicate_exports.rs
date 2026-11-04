use std::collections::BTreeMap;

use skif_core::{Category, Fixability, Location, Related, RuleMeta, Severity, diagnostic};
use skif_js_syntax::{Export, ExportKind, RemovalKind};
use skif_lint::Emitter;

use super::JsRule;
use super::support::module_location;
use crate::analysis::JsAnalysis;

/// Metadata of `duplicate-exports`.
pub static META: RuleMeta = RuleMeta::new(
    "duplicate-exports",
    Category::Js,
    Some(Severity::Warning),
    Fixability::None,
    "One binding is exported under several names.",
);

/// Reports a binding exported twice, such as `export const a = 1; export default a;`.
///
/// Entry modules are skipped (framework route files alias one handler as `GET` and `POST`), and so are groups made of
/// export-clause aliases alone (`export { h as GET, h as POST }`).
pub struct DuplicateExports;

impl JsRule for DuplicateExports {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        for module in analysis.graph.modules() {
            if !module.in_project || module.ignored || module.is_entry {
                continue;
            }
            let mut by_binding: BTreeMap<&str, Vec<&skif_js_syntax::Export>> = BTreeMap::new();
            for export in &module.info.exports {
                if matches!(export.kind, ExportKind::ReExport | ExportKind::StarReExport) {
                    continue;
                }
                if let Some(local) = export.local_name.as_deref() {
                    by_binding.entry(local).or_default().push(export);
                }
            }
            for (local, exports) in by_binding {
                let mut names: Vec<&str> = exports.iter().map(|export| export.name.as_str()).collect();
                names.sort_unstable();
                names.dedup();
                let (Some(first), Some(second)) = (exports.first(), exports.get(1)) else {
                    continue;
                };
                if names.len() < 2 || exports.iter().all(|export| is_clause_alias(export)) {
                    continue;
                }
                let mut builder = diagnostic(
                    &META,
                    format!(
                        "`{local}` is exported as {}",
                        names.iter().map(|n| format!("`{n}`")).collect::<Vec<_>>().join(" and ")
                    ),
                    module_location(module, Some(second.span)),
                )
                .help("keep one export name for the binding")
                .related(Related::new(
                    format!("first exported as `{}` here", first.name),
                    Location::new(
                        module.path.clone(),
                        Some(first.span),
                    ),
                ));
                if let Some(package) = analysis.workspace.package(module.owner) {
                    builder = builder.package(package.display_name());
                }
                out.emit(builder.build());
            }
        }
    }
}

/// Whether the export is a specifier of an `export { local as name }` clause.
fn is_clause_alias(export: &Export) -> bool {
    export.removal.is_some_and(|removal| {
        matches!(removal.kind, RemovalKind::Specifier | RemovalKind::Statement) && !removal.default_expression
    })
}
