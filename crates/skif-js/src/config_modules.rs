//! Finds the modules a tool config imports values from, so the static config evaluator can follow them.

use std::path::{Path, PathBuf};

use serde_json::Value;
use skif_core::FileSystem;
use skif_js_syntax::ModuleLoader;
use skif_workspace::Workspace;

use crate::manifest_paths::{ENTRY_FIELDS, is_out_dir, map_to_source, resolve_exports};
use crate::resolve::{is_code_file, join_normalized};
use crate::specifier::{package_name, subpath};

/// Extensions tried, in order, for a relative import written without one.
const SCRIPT_EXTENSIONS: &[&str] = &["ts", "mts", "cts", "tsx", "js", "mjs", "cjs", "jsx"];

/// The source extensions a TypeScript import with a JavaScript extension stands for (`./base.js` is `./base.ts`).
const EXTENSION_ALIASES: &[(&str, &[&str])] = &[
    ("js", &["ts", "tsx"]),
    ("mjs", &["mts"]),
    ("cjs", &["cts"]),
    ("jsx", &["tsx"]),
];

/// Follows the imports of a config into the files of the workspace: relative imports, and imports of a workspace
/// package through its `exports` (or `main`) mapped back to the source file. Installed packages are not followed:
/// their files are not part of the project and may not be present.
pub(crate) struct ConfigModules<'a> {
    fs: &'a dyn FileSystem,
    workspace: &'a Workspace,
}

impl<'a> ConfigModules<'a> {
    pub(crate) fn new(fs: &'a dyn FileSystem, workspace: &'a Workspace) -> Self {
        Self { fs, workspace }
    }

    /// The source file of a workspace package's export named by `specifier`.
    fn package_module(&self, specifier: &str) -> Option<PathBuf> {
        let name = package_name(specifier)?;
        let package = self
            .workspace
            .all_packages()
            .find(|package| package.name.as_deref() == Some(name.as_str()))?;
        let dir = self.workspace.root.join(&package.dir);
        let manifest = package.manifest.shared_value();
        let rest = subpath(specifier, &name);
        let target = match (manifest.get("exports"), rest) {
            (Some(exports), rest) => {
                let subpath = rest.map_or_else(|| ".".to_owned(), |rest| format!("./{rest}"));
                resolve_exports(&without_types(exports), &subpath, &[])?
            }
            (None, Some(rest)) => rest.to_owned(),
            (None, None) => ENTRY_FIELDS
                .iter()
                .filter(|field| !matches!(**field, "types" | "typings"))
                .find_map(|field| manifest.get(*field).and_then(Value::as_str))
                .unwrap_or("index.js")
                .to_owned(),
        };
        map_to_source(self.fs, &dir, &target).or_else(|| {
            // Compiled with `rootDir: "."`, `dist/a.js` is built from `a.ts` beside the manifest.
            let (first, rest) = target.trim_start_matches("./").split_once('/')?;
            is_out_dir(first).then(|| map_to_source(self.fs, &dir, rest)).flatten()
        })
    }
}

impl ModuleLoader for ConfigModules<'_> {
    fn load(&self, from: &Path, specifier: &str) -> Option<(PathBuf, String)> {
        let path = if specifier.starts_with("./") || specifier.starts_with("../") {
            relative_module(self.fs, &join_normalized(from.parent()?, specifier))?
        } else {
            self.package_module(specifier)?
        };
        let inside =
            path.starts_with(&self.workspace.root) && !path.components().any(|part| part.as_os_str() == "node_modules");
        if !inside || !is_code_file(&path) {
            return None;
        }
        let source = self.fs.read_to_string(&path).ok()?;
        Some((path, source.to_string()))
    }
}

/// The file a relative import of `base` loads: the file itself, its TypeScript source, the file with an extension
/// added, or the directory's index file.
fn relative_module(fs: &dyn FileSystem, base: &Path) -> Option<PathBuf> {
    let extension = base.extension().and_then(|e| e.to_str()).unwrap_or_default();
    let aliases = EXTENSION_ALIASES
        .iter()
        .find(|(written, _)| *written == extension)
        .map(|(_, sources)| *sources)
        .unwrap_or_default();
    let mut candidates = vec![base.to_path_buf()];
    candidates.extend(aliases.iter().map(|alias| base.with_extension(alias)));
    let name = base.file_name()?.to_string_lossy().into_owned();
    candidates.extend(
        SCRIPT_EXTENSIONS
            .iter()
            .map(|ext| base.with_file_name(format!("{name}.{ext}"))),
    );
    candidates.extend(SCRIPT_EXTENSIONS.iter().map(|ext| base.join(format!("index.{ext}"))));
    candidates.into_iter().find(|candidate| fs.is_file(candidate))
}

/// `exports` without its `types` conditions, which name declaration files rather than the module's code.
fn without_types(exports: &Value) -> Value {
    match exports {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(key, _)| !matches!(key.as_str(), "types" | "typings"))
                .map(|(key, value)| (key.clone(), without_types(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_types).collect()),
        other => other.clone(),
    }
}
