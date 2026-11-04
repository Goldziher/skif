use std::collections::HashSet;

use skif_core::{Category, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::Emitter;

use super::JsRule;
use super::support::{is_fixture, module_location};
use crate::analysis::JsAnalysis;

/// Metadata of `unlisted-dependencies`.
pub static META: RuleMeta = RuleMeta::new(
    "unlisted-dependencies",
    Category::Js,
    Some(Severity::Error),
    Fixability::None,
    "A file imports a package that neither its `package.json` nor any `package.json` above it lists.",
);

/// Reports imports of packages the manifest does not list, once per file and package.
pub struct UnlistedDependencies;

impl JsRule for UnlistedDependencies {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        let mut seen = HashSet::new();
        for unlisted in &analysis.dependencies.unlisted {
            let module = analysis.graph.module(unlisted.module);
            if module.ignored
                || is_fixture(module)
                || analysis.ignores_dependency(unlisted.package, &unlisted.name)
                || analysis.declares_module(&unlisted.name)
            {
                continue;
            }
            if !seen.insert((unlisted.package, unlisted.name.as_str(), unlisted.module)) {
                continue;
            }
            let Some(package) = analysis.workspace.package(unlisted.package) else {
                continue;
            };
            out.emit(
                diagnostic(
                    &META,
                    format!("`{}` is imported but not listed in `package.json`", unlisted.name),
                    module_location(module, unlisted.span),
                )
                .package(package.display_name())
                .dependency(unlisted.name.clone())
                .help(format!(
                    "add `{}` to the dependencies of `{}`",
                    unlisted.name,
                    package.display_name()
                ))
                .build(),
            );
        }
    }
}
