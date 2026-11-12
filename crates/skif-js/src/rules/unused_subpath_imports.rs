use skif_core::{
    Category, Diagnostic, Fix, FixOp, FixSafety, Fixability, JsonAction, PathSeg, RuleMeta, Severity, diagnostic,
};
use skif_lint::Emitter;
use skif_workspace::Package;

use super::JsRule;
use super::support::manifest_key_location;
use crate::analysis::JsAnalysis;
use crate::subpaths::{imports_map, used_import_keys};

/// Metadata of `unused-subpath-imports`, knip's `subpaths` issue type for `imports`.
pub static META: RuleMeta = RuleMeta::new(
    "unused-subpath-imports",
    Category::Js,
    Some(Severity::Warning),
    Fixability::Unsafe,
    "A `package.json` `imports` entry is not imported by any file of its package.",
);

/// Reports the `imports` entries (`"#utils": "./src/utils.ts"`) of a `package.json` that no `#` specifier of the
/// package's files selects. A pattern entry (`#utils/*`) is used when any specifier matches it, and an exact entry
/// wins over a pattern that matches too, as in Node.js. A `null` entry, which hides a subpath, is never reported.
///
/// Production mode reports nothing, because test files, which may be the only importers, are not analysed then. A
/// package left out of the analysis, or whose entry files are not all known, is not judged. The fix removes the entry;
/// it is unsafe because a script or tool may name the specifier where skif does not read it.
pub struct UnusedSubpathImports;

impl JsRule for UnusedSubpathImports {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, analysis: &JsAnalysis<'_>, out: &mut Emitter<'_>) {
        if analysis.production() {
            return;
        }
        let used = used_import_keys(analysis.workspace, &analysis.graph);
        for (id, package) in analysis.workspace.iter() {
            if analysis.is_skipped(id) || analysis.entries_incomplete(id) {
                continue;
            }
            let Some(imports) = imports_map(package) else {
                continue;
            };
            let used_here = used.get(&id);
            for (key, value) in imports {
                let in_use = used_here.is_some_and(|keys| keys.contains(key.as_str()));
                if key.starts_with('#') && !value.is_null() && !in_use {
                    out.emit(unused_import(package, key));
                }
            }
        }
    }
}

fn unused_import(package: &Package, key: &str) -> Diagnostic {
    let pointer = [PathSeg::Key("imports".to_owned()), PathSeg::Key(key.to_owned())];
    diagnostic(
        &META,
        format!("`{key}` in `imports` is never imported by its package"),
        manifest_key_location(package, &pointer),
    )
    .package(package.display_name())
    .package_id(package.id)
    .help("remove the entry, or add an `[[ignore]]` entry when a script or tool imports it")
    .fix(Fix::new(
        format!("Remove `{key}` from `imports`"),
        FixSafety::Unsafe,
        vec![FixOp::json(
            package.manifest.path.clone(),
            pointer.to_vec(),
            JsonAction::Remove,
        )],
    ))
    .build()
}
