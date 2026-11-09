use skif_core::{Category, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::Emitter;

use super::JsRule;
use super::support::{module_location, unresolved_imports};
use crate::analysis::JsAnalysis;

/// Metadata of `unresolved-imports`.
pub static META: RuleMeta = RuleMeta::new(
    "unresolved-imports",
    Category::Js,
    Some(Severity::Error),
    Fixability::None,
    "An import specifier does not resolve to a file or a package.",
);

/// Reports relative, aliased and `#` specifiers that point nowhere. A test-runner mock that points nowhere is
/// `stale-mocks`'s.
pub struct UnresolvedImports;

impl JsRule for UnresolvedImports {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        for (module, import) in unresolved_imports(analysis, false) {
            let mut builder = diagnostic(
                &META,
                format!("cannot resolve `{}`", import.specifier),
                module_location(module, Some(import.span)),
            )
            .help("check the path, the `tsconfig` paths, the bundler aliases and the package `exports`");
            if let Some(package) = analysis.workspace.package(module.owner) {
                builder = builder.package(package.display_name());
            }
            out.emit(builder.build());
        }
    }
}
