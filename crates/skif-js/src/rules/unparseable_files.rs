use std::path::Path;

use skif_core::{Category, Fixability, RuleMeta, Severity, diagnostic, slash};
use skif_lint::Emitter;

use super::JsRule;
use super::support::module_location;
use crate::analysis::JsAnalysis;

/// Metadata of `unparseable-files`. The parser could not recover from a syntax error, so what the file imports and
/// exports is unknown and the other JS rules treat it as empty.
pub static META: RuleMeta = RuleMeta::new(
    "unparseable-files",
    Category::Js,
    Some(Severity::Warning),
    Fixability::None,
    "A file reachable from an entry file has a syntax error the parser could not recover from.",
);

/// Reports files in the module graph whose imports and exports could not be read.
pub struct UnparseableFiles;

/// The message of a file the parser gave up on, with `/` separators on every platform.
fn unparseable_message(path: &Path, error: &str) -> String {
    format!("`{}` could not be parsed: {error}", slash(path))
}

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
                unparseable_message(&module.path, error),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_message_names_the_file_with_forward_slashes() {
        let path = Path::new("packages").join("a").join("broken.ts");
        assert_eq!(
            unparseable_message(&path, "unexpected token"),
            "`packages/a/broken.ts` could not be parsed: unexpected token"
        );
    }
}
