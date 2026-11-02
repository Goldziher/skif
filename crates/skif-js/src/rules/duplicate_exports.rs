use std::collections::BTreeMap;

use skif_core::{Category, Fixability, Location, Related, RuleMeta, Severity, diagnostic};
use skif_js_syntax::ExportKind;
use skif_lint::Emitter;

use super::JsRule;
use super::support::module_location;
use crate::analysis::JsAnalysis;

/// Metadata of `duplicate-exports`.
pub static META: RuleMeta = RuleMeta {
    id: "duplicate-exports",
    category: Category::Js,
    default: Some(Severity::Warning),
    fixable: Fixability::None,
    summary: "One binding is exported under several names.",
};

/// Reports a binding exported twice, such as `export { a, a as default }`.
pub struct DuplicateExports;

impl JsRule for DuplicateExports {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        for module in analysis.graph.modules() {
            if !module.in_project || module.ignored {
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
                if names.len() < 2 {
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
                .related(Related {
                    message: format!("first exported as `{}` here", first.name),
                    location: Location {
                        path: module.path.clone(),
                        span: Some(first.span),
                    },
                });
                if let Some(package) = analysis.workspace.package(module.owner) {
                    builder = builder.package(package.display_name());
                }
                out.emit(builder.build());
            }
        }
    }
}
