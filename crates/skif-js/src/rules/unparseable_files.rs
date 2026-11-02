use skif_core::{Category, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::Emitter;

use super::JsRule;
use super::support::module_location;
use crate::analysis::JsAnalysis;

/// Metadata of `unparseable-files`. The parser could not recover from a syntax error, so what the file imports and
/// exports is unknown and the other JS rules treat it as empty.
pub static META: RuleMeta = RuleMeta {
    id: "unparseable-files",
    category: Category::Js,
    default: Some(Severity::Warning),
    fixable: Fixability::None,
    summary: "A file reachable from an entry file has a syntax error the parser could not recover from.",
};

/// Reports files in the module graph whose imports and exports could not be read.
pub struct UnparseableFiles;

impl JsRule for UnparseableFiles {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        for module in analysis.graph.modules() {
            let Some(error) = &module.parse_error else { continue };
            if module.ignored || analysis.is_skipped(module.owner) {
                continue;
            }
            let span = module.info.parse_diagnostics.first().map(|issue| issue.span);
            let mut builder = diagnostic(
                &META,
                format!("`{}` could not be parsed: {error}", module.path.display()),
                module_location(module, span),
            )
            .help("fix the syntax error, or add the file to `ignore`; its imports and exports are not analysed");
            if let Some(package) = analysis.workspace.package(module.owner) {
                builder = builder.package(package.display_name());
            }
            out.emit(builder.build());
        }
    }
}
