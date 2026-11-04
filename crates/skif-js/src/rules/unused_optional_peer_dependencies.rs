use serde_json::Value;
use skif_core::{Category, DependencyKind, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::Emitter;

use super::JsRule;
use super::support::entry_location;
use crate::analysis::JsAnalysis;

/// Metadata of `unused-optional-peer-dependencies`.
pub static META: RuleMeta = RuleMeta::new(
    "unused-optional-peer-dependencies",
    Category::Js,
    Some(Severity::Warning),
    Fixability::None,
    "An optional peer dependency is imported but not installed as a dependency or dev dependency.",
);

/// Reports optional peer dependencies the code imports while the package does not install them itself.
pub struct UnusedOptionalPeerDependencies;

impl JsRule for UnusedOptionalPeerDependencies {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        for (id, package) in analysis.workspace.iter() {
            if analysis.is_skipped(id) {
                continue;
            }
            let Some(meta) = package
                .manifest
                .value()
                .get("peerDependenciesMeta")
                .and_then(Value::as_object)
            else {
                continue;
            };
            for (name, value) in meta {
                let optional = value.get("optional").and_then(Value::as_bool).unwrap_or(false);
                if !optional || !analysis.dependencies.was_imported(id, name) || analysis.ignores_dependency(id, name) {
                    continue;
                }
                let installed = package.dependencies.iter().any(|entry| {
                    entry.name == *name
                        && matches!(
                            entry.kind,
                            DependencyKind::Dependencies
                                | DependencyKind::DevDependencies
                                | DependencyKind::OptionalDependencies
                        )
                });
                let Some(peer) = package
                    .dependencies
                    .iter()
                    .find(|entry| entry.name == *name && entry.kind == DependencyKind::PeerDependencies)
                else {
                    continue;
                };
                if installed {
                    continue;
                }
                out.emit(
                    diagnostic(
                        &META,
                        format!("optional peer dependency `{name}` is imported but not installed"),
                        entry_location(package, peer),
                    )
                    .package(package.display_name())
                    .dependency(name.clone())
                    .help("add it to `devDependencies` so the package can be built and tested")
                    .build(),
                );
            }
        }
    }
}
