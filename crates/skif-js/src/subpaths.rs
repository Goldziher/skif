//! Which `imports` and `exports` entries of the workspace's `package.json` files the analysed code imports, for
//! `unused-subpath-imports` and `unused-subpath-exports`.
//!
//! An import uses the entry Node.js would resolve its specifier through ([`select_key`]): an exact key, else the
//! pattern key (`#utils/*`, `./features/*`) that matches it, whichever conditions the entry's value has. A `#`
//! specifier uses the `imports` of the package that owns the importing file; a bare specifier naming a workspace
//! package, its own included, uses that package's `exports`.

use std::path::Path;

use serde_json::{Map, Value};
use skif_core::{HashMap, HashSet};
use skif_js_syntax::ImportKind;
use skif_workspace::{Package, PackageId, Workspace};

use crate::graph::{Module, ModuleGraph};
use crate::manifest_paths::{collect_strings, select_key};
use crate::resolve::is_code_file;
use crate::specifier::{package_name, subpath};

/// The keys of the entries each package's code uses, by package.
pub(crate) type UsedKeys<'w> = HashMap<PackageId, HashSet<&'w str>>;

/// The `imports` field of `package`, when it is an object.
pub(crate) fn imports_map(package: &Package) -> Option<&Map<String, Value>> {
    package.manifest.value().get("imports")?.as_object()
}

/// The subpath entries of an `exports` field as Node.js reads them: the `.`-keys of an object of subpaths, or `.` for
/// any other value (a path, an array, an object of conditions).
pub(crate) fn export_entries(exports: &Value) -> Vec<(&str, &Value)> {
    match subpath_map(exports) {
        Some(map) => map
            .iter()
            .filter(|(key, _)| key.starts_with('.'))
            .map(|(key, value)| (key.as_str(), value))
            .collect(),
        None => vec![(".", exports)],
    }
}

/// Whether an `exports` entry publishes a module, as knip judges it: one of its paths has no extension or that of a
/// file skif parses. `./package.json` and `./styles.css` publish none.
pub(crate) fn publishes_module(value: &Value) -> bool {
    let mut paths = Vec::new();
    collect_strings(value, &mut paths);
    paths.iter().any(|path| {
        let path = Path::new(path);
        path.extension().is_none() || is_code_file(path)
    })
}

/// The `imports` keys each package uses: those the `#` specifiers of its own files select. Files of other projects
/// ([`Module::foreign`]) and outside the root follow the `imports` of their own packages, so they use none.
pub(crate) fn used_import_keys<'w>(workspace: &'w Workspace, graph: &ModuleGraph) -> UsedKeys<'w> {
    let mut used = UsedKeys::default();
    for module in graph.modules() {
        if module.foreign || !module.abs.starts_with(&workspace.root) {
            continue;
        }
        let Some(imports) = workspace.package(module.owner).and_then(imports_map) else {
            continue;
        };
        for specifier in specifiers(module).filter(|specifier| specifier.starts_with('#')) {
            let specifier = specifier.split('?').next().unwrap_or(specifier);
            if let Some(selected) = select_key(imports, specifier) {
                used.entry(module.owner).or_default().insert(selected.key);
            }
        }
    }
    used
}

/// The `exports` keys each package uses: those the bare specifiers naming it select, from any file of the graph.
pub(crate) fn used_export_keys<'w>(workspace: &'w Workspace, graph: &ModuleGraph) -> UsedKeys<'w> {
    let mut used = UsedKeys::default();
    for module in graph.modules() {
        for specifier in specifiers(module) {
            let Some(name) = package_name(specifier) else {
                continue;
            };
            let Some(target) = workspace.package_by_name(&name) else {
                continue;
            };
            let Some(exports) = target.manifest.value().get("exports") else {
                continue;
            };
            let key = subpath(specifier, &name).map_or_else(|| ".".to_owned(), |rest| format!("./{rest}"));
            if let Some(selected) = select_export(exports, &key) {
                used.entry(target.id).or_default().insert(selected);
            }
        }
    }
    used
}

/// The key of `exports` that the subpath `key` (`.` or `./x`) selects.
fn select_export<'m>(exports: &'m Value, key: &str) -> Option<&'m str> {
    match subpath_map(exports) {
        Some(map) => select_key(map, key).map(|selected| selected.key),
        None => (key == ".").then_some("."),
    }
}

/// An `exports` object whose keys are subpaths, as opposed to conditions.
fn subpath_map(exports: &Value) -> Option<&Map<String, Value>> {
    exports
        .as_object()
        .filter(|map| map.keys().any(|key| key.starts_with('.')))
}

/// The specifiers a module imports as modules: every kind but the globs, which name files rather than specifiers.
fn specifiers(module: &Module) -> impl Iterator<Item = &str> {
    module
        .info
        .imports
        .iter()
        .filter(|import| !matches!(import.kind, ImportKind::ImportMetaGlob | ImportKind::SourceGlob))
        .map(|import| import.specifier.as_str())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn an_object_of_subpaths_lists_its_dot_keys_and_any_other_value_is_the_root() {
        let subpaths = json!({ ".": "./a.js", "./b": "./b.js" });
        let keys: Vec<&str> = export_entries(&subpaths).into_iter().map(|(key, _)| key).collect();
        assert_eq!(keys, [".", "./b"]);
        for root in [
            json!("./a.js"),
            json!(["./a.js"]),
            json!({ "import": "./a.mjs", "require": "./a.cjs" }),
        ] {
            let keys: Vec<&str> = export_entries(&root).into_iter().map(|(key, _)| key).collect();
            assert_eq!(keys, ["."], "{root}");
        }
    }

    #[test]
    fn only_entries_with_a_module_path_publish_a_module() {
        assert!(publishes_module(&json!("./dist/a.js")));
        assert!(publishes_module(&json!("./dist/*")));
        assert!(publishes_module(&json!({ "types": "./a.d.ts", "default": "./a.css" })));
        assert!(!publishes_module(&json!("./package.json")));
        assert!(!publishes_module(&json!({ "style": "./a.css", "default": "./a.css" })));
        assert!(!publishes_module(&Value::Null));
    }

    #[test]
    fn a_subpath_selects_the_export_key_node_resolves_it_through() {
        let exports = json!({ ".": "./i.js", "./x/*": "./x/*.js", "./x/y": "./y.js" });
        assert_eq!(select_export(&exports, "."), Some("."));
        assert_eq!(select_export(&exports, "./x/y"), Some("./x/y"));
        assert_eq!(select_export(&exports, "./x/z"), Some("./x/*"));
        assert_eq!(select_export(&exports, "./w"), None);
        assert_eq!(select_export(&json!("./i.js"), "."), Some("."));
        assert_eq!(select_export(&json!("./i.js"), "./x"), None);
    }
}
