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

/// Reports optional peer dependencies the code imports while neither the package nor a package containing it installs
/// them.
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
                // Only the configuration's ignore lists silence the finding: a plugin reference that may be absent,
                // such as a package the bundler keeps external, does not install the package for the tests.
                if !optional
                    || !analysis.dependencies.was_imported(id, name)
                    || analysis.user_ignores_dependency(id, name)
                    || analysis
                        .dependencies
                        .installs_for_development(analysis.workspace, id, name)
                {
                    continue;
                }
                let Some(peer) = package
                    .dependencies
                    .iter()
                    .find(|entry| &*entry.name == name.as_str() && entry.kind == DependencyKind::PeerDependencies)
                else {
                    continue;
                };
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
