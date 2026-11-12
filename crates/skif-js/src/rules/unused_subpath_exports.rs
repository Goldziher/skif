use skif_core::{Category, Diagnostic, Fixability, PathSeg, RuleMeta, diagnostic};
use skif_lint::Emitter;
use skif_workspace::{Package, PackageId};

use super::JsRule;
use super::support::manifest_key_location;
use crate::analysis::JsAnalysis;
use crate::subpaths::{export_entries, publishes_module, used_export_keys};

/// Metadata of `unused-subpath-exports`, knip's `subpaths` issue type for `exports` (`includeSubpathExports`). Off by
/// default, as in knip: a published package's subpaths are usually imported outside the workspace.
pub static META: RuleMeta = RuleMeta::new(
    "unused-subpath-exports",
    Category::Js,
    None,
    Fixability::None,
    "A `package.json` `exports` subpath is not imported by any workspace package.",
);

/// Reports the `exports` subpaths (`"./feature": "./dist/feature.js"`) of a workspace package that no file of the
/// workspace imports by the package's name (`pkg/feature`; `pkg` itself for `.`), the package's own files included.
/// A pattern subpath (`./features/*`) is used when any import matches it. A subpath that publishes no module
/// (`./package.json`, `./styles.css`) or is `null` is never reported.
///
/// Production mode reports nothing, as for `unused-subpath-imports`. A package is not judged when it is left out of
/// the analysis or its entry files are not all known, when a package of either kind lists it as a dependency (its
/// imports are unknown), or when another root of a multi-root run imports one of its files. There is no fix: a subpath
/// is public API that code outside the workspace may import.
pub struct UnusedSubpathExports;

impl JsRule for UnusedSubpathExports {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        if analysis.production() {
            return;
        }
        let used = used_export_keys(analysis.workspace, &analysis.graph);
        let unknown_importers: Vec<&Package> = analysis
            .workspace
            .iter()
            .filter(|(id, _)| unknown_imports(analysis, *id))
            .map(|(_, package)| package)
            .collect();
        for (id, package) in analysis.workspace.iter() {
            let Some(exports) = package.manifest.value().get("exports") else {
                continue;
            };
            let listed_by_unknown = package.name.as_deref().is_some_and(|name| {
                unknown_importers
                    .iter()
                    .any(|importer| importer.dependencies.iter().any(|entry| &*entry.name == name))
            });
            if unknown_imports(analysis, id) || listed_by_unknown || analysis.imported_by_other_roots.contains(&id) {
                continue;
            }
            let used_here = used.get(&id);
            for (key, value) in export_entries(exports) {
                let in_use = used_here.is_some_and(|keys| keys.contains(key));
                if !value.is_null() && publishes_module(value) && !in_use {
                    out.emit(unused_export(package, key));
                }
            }
        }
    }
}

/// Whether the imports of package `id` are not all known: it is left out of the analysis or its entry files are not
/// all known.
fn unknown_imports(analysis: &JsAnalysis<'_>, id: PackageId) -> bool {
    analysis.is_skipped(id) || analysis.entries_incomplete(id)
}

/// The finding of subpath `key`, located at its key, or at `exports` when the field has no subpath keys (`.` of a path
/// or of an object of conditions).
fn unused_export(package: &Package, key: &str) -> Diagnostic {
    let pointer = [PathSeg::Key("exports".to_owned()), PathSeg::Key(key.to_owned())];
    diagnostic(
        &META,
        format!("`{key}` in `exports` is never imported by a workspace package"),
        manifest_key_location(package, &pointer),
    )
    .package(package.display_name())
    .package_id(package.id)
    .help("remove the subpath if no code outside the workspace imports it, or add an `[[ignore]]` entry")
    .build()
}
