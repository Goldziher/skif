use skif_core::{Category, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::Emitter;

use super::JsRule;
use super::support::module_location;
use crate::analysis::JsAnalysis;
use crate::graph::Target;

/// Metadata of `unresolved-imports`.
pub static META: RuleMeta = RuleMeta {
    id: "unresolved-imports",
    category: Category::Js,
    default: Some(Severity::Error),
    fixable: Fixability::None,
    summary: "An import specifier does not resolve to a file or a package.",
};

/// Reports relative, aliased and `#` specifiers that point nowhere.
pub struct UnresolvedImports;

impl JsRule for UnresolvedImports {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        for module in analysis.graph.modules() {
            if module.ignored || analysis.is_skipped(module.owner) {
                continue;
            }
            let package = analysis.workspace.package(module.owner);
            for resolved in module
                .imports
                .iter()
                .filter(|resolved| resolved.target == Target::Unresolved)
            {
                let Some(import) = module.info.imports.get(resolved.import) else {
                    continue;
                };
                if analysis.ignores_dependency(module.owner, &import.specifier) {
                    continue;
                }
                let mut builder = diagnostic(
                    &META,
                    format!("cannot resolve `{}`", import.specifier),
                    module_location(module, Some(import.span)),
                )
                .help("check the path, the `tsconfig` paths, the bundler aliases and the package `exports`");
                if let Some(package) = package {
                    builder = builder.package(package.display_name());
                }
                out.emit(builder.build());
            }
        }
    }
}
