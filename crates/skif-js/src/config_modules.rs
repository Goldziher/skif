//! Finds the modules a tool config imports values from, so the static config evaluator can follow them.

use std::path::{Path, PathBuf};

use serde_json::Value;
use skif_core::FileSystem;
use skif_js_syntax::ModuleLoader;
use skif_workspace::Workspace;

use crate::manifest_paths::{ENTRY_FIELDS, is_out_dir, map_to_source, resolve_exports};
use crate::resolve::{is_code_file, join_normalized};
use crate::specifier::{is_builtin, package_name, subpath};

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
/// their files are not part of the project and may not be present. A bare specifier it does not follow counts as an
/// installed package ([`ModuleLoader::is_installed`]) when it is a Node.js builtin, or names no workspace package and
/// a `node_modules` directory at or above the importing file holds it; one that none holds is an alias of a project
/// directory or a package that is not installed, unless there is no `node_modules` directory there at all (a project
/// not installed yet), where nothing tells the two apart and every name of no workspace package counts.
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

    fn is_installed(&self, from: &Path, specifier: &str) -> bool {
        if is_builtin(specifier) {
            return true;
        }
        let Some(name) = package_name(specifier) else {
            return false;
        };
        if self
            .workspace
            .all_packages()
            .any(|package| package.name.as_deref() == Some(name.as_str()))
        {
            return false;
        }
        let mut installs = false;
        for dir in from.ancestors().skip(1) {
            let modules = dir.join("node_modules");
            if !self.fs.is_dir(&modules) {
                continue;
            }
            if self.fs.is_dir(&modules.join(&name)) {
                return true;
            }
            installs = true;
        }
        !installs
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use skif_config::Config;
    use skif_core::MemoryFs;

    use super::*;

    const CONFIG: &str = "/ws/packages/app/vite.config.ts";

    fn workspace(fs: &MemoryFs) -> Workspace {
        Workspace::discover_with_fs(Path::new("/ws"), &Config::from_toml_str("").unwrap(), fs).unwrap()
    }

    fn project() -> MemoryFs {
        MemoryFs::new()
            .with_file("/ws/package.json", r#"{"name":"root","workspaces":["packages/*"]}"#)
            .with_file("/ws/packages/app/package.json", r#"{"name":"app"}"#)
            .with_file(CONFIG, "export default {};")
            .with_file("/ws/packages/tools/package.json", r#"{"name":"@app/tools","main":"dist/index.js"}"#)
    }

    /// A package a `node_modules` directory above the config holds is installed, as is a Node.js builtin; a workspace
    /// package, also one the loader cannot map to a source file, and a name no `node_modules` holds (an alias of a
    /// project directory) are not.
    #[test]
    fn a_package_is_installed_where_a_node_modules_directory_holds_it() {
        let fs = project()
            .with_file("/ws/node_modules/vite/package.json", r#"{"name":"vite"}"#)
            .with_file("/ws/packages/app/node_modules/@vitejs/plugin-react/package.json", "{}");
        let workspace = workspace(&fs);
        let modules = ConfigModules::new(&fs, &workspace);
        let from = Path::new(CONFIG);
        for (specifier, installed) in [
            ("vite", true),
            ("vite/client", true),
            ("@vitejs/plugin-react", true),
            ("node:path", true),
            ("path", true),
            ("@app/tools", false),
            ("@app/alias", false),
            ("lodash", false),
            ("./local", false),
        ] {
            assert_eq!(modules.is_installed(from, specifier), installed, "{specifier}");
        }
    }

    /// With no `node_modules` directory above the config, nothing tells an alias from a package that is not installed
    /// yet: every name of no workspace package counts as installed.
    #[test]
    fn without_node_modules_every_name_of_no_workspace_package_is_installed() {
        let fs = project();
        let workspace = workspace(&fs);
        let modules = ConfigModules::new(&fs, &workspace);
        let from = Path::new(CONFIG);
        for (specifier, installed) in [("vite", true), ("@app/alias", true), ("@app/tools", false)] {
            assert_eq!(modules.is_installed(from, specifier), installed, "{specifier}");
        }
    }
}
