//! Finds the modules a tool config imports values from, so the static config evaluator can follow them.

use std::path::{Path, PathBuf};

use serde_json::Value;
use skif_core::FileSystem;
use skif_js_syntax::ModuleLoader;
use skif_workspace::Workspace;

use crate::manifest_paths::{ENTRY_FIELDS, is_out_dir, map_to_source, resolve_exports};
use crate::resolve::{is_code_file, join_normalized};
use crate::specifier::{has_foreign_scheme, is_builtin, package_name, subpath};

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
/// installed package ([`ModuleLoader::is_installed`]) when it is a Node.js builtin or has a scheme (`npm:vite`,
/// `virtual:pwa`), which names a module of a runtime or a tool, or names no workspace package and
/// a `node_modules` directory at or above the importing file holds it; one that none holds is an alias of a project
/// directory or a package that is not installed, unless no `node_modules` directory at or above the file, up to the
/// workspace root, holds a package at all (a project not installed yet, or one holding only the caches of tools such as
/// `.vite`), where nothing tells the two apart and every name of no workspace package counts. Under Yarn Plug'n'Play
/// (a `.pnp.cjs` at the workspace root), the dependencies the importing package or the root declares from a registry
/// are installed, and no other name is.
pub(crate) struct ConfigModules<'a> {
    fs: &'a dyn FileSystem,
    workspace: &'a Workspace,
}

impl<'a> ConfigModules<'a> {
    pub(crate) fn new(fs: &'a dyn FileSystem, workspace: &'a Workspace) -> Self {
        Self { fs, workspace }
    }

    /// Whether the `node_modules` directory `modules` holds a package: an entry whose name does not start with a dot,
    /// unlike the caches tools keep there (`.vite`, `.cache`) and the package manager's own files (`.bin`, `.pnpm`).
    fn holds_packages(&self, modules: &Path) -> bool {
        self.fs.read_dir_shared(modules).is_ok_and(|entries| {
            entries.iter().any(|entry| {
                entry
                    .path
                    .file_name()
                    .is_some_and(|name| !name.to_string_lossy().starts_with('.'))
            })
        })
    }

    /// Whether the package that holds the file `from`, or the root package, declares the dependency `name` with a
    /// specifier of a registry rather than one of a directory of the project (`link:`, `file:`, `portal:`,
    /// `workspace:`, a path).
    fn declares(&self, from: &Path, name: &str) -> bool {
        let root = &self.workspace.root;
        let owner = self
            .workspace
            .all_packages()
            .filter(|package| from.starts_with(root.join(&package.dir)))
            .max_by_key(|package| package.dir.components().count());
        owner
            .into_iter()
            .chain(self.workspace.all_packages().take(1))
            .flat_map(|package| &package.dependencies)
            .any(|entry| &*entry.name == name && !names_project_directory(&entry.specifier))
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
        let path = if is_relative(specifier) {
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
        if is_builtin(specifier) || has_foreign_scheme(specifier) {
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
        let root = &self.workspace.root;
        if PNP_FILES.iter().any(|file| self.fs.is_file(&root.join(file))) {
            return self.declares(from, &name);
        }
        let mut installs = false;
        let mut inside = true;
        for dir in from.ancestors().skip(1) {
            let modules = dir.join("node_modules");
            if self.fs.is_dir(&modules) {
                if self.fs.is_dir(&modules.join(&name)) {
                    return true;
                }
                installs |= inside && self.holds_packages(&modules);
            }
            inside &= dir != root.as_path();
        }
        !installs
    }

    /// A relative import of a file that is there and is not code, or of a declaration file standing for a module that
    /// is not there ([`declaration_module`]).
    fn holds_no_code(&self, from: &Path, specifier: &str) -> bool {
        let Some(dir) = from.parent().filter(|_| is_relative(specifier)) else {
            return false;
        };
        let base = join_normalized(dir, specifier);
        relative_module(self.fs, &base)
            .or_else(|| declaration_module(self.fs, &base))
            .is_some_and(|path| !is_code_file(&path) || is_declaration_file(&path))
    }
}

/// The files at a workspace root that say Yarn Plug'n'Play installs its packages, with no `node_modules` directory.
const PNP_FILES: &[&str] = &[".pnp.cjs", ".pnp.loader.mjs", ".pnp.js"];

/// Whether a dependency specifier names a directory of the project rather than a published package.
fn names_project_directory(specifier: &str) -> bool {
    ["link:", "file:", "portal:", "workspace:", "./", "../", "/"]
        .iter()
        .any(|prefix| specifier.starts_with(prefix))
}

/// Whether `specifier` is a relative import.
fn is_relative(specifier: &str) -> bool {
    specifier.starts_with("./") || specifier.starts_with("../")
}

/// The suffixes of a declaration file.
const DECLARATION_SUFFIXES: &[&str] = &["d.ts", "d.mts", "d.cts"];

/// Whether `path` is a declaration file (`types.d.ts`), which holds types only.
fn is_declaration_file(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()).is_some_and(|name| {
        DECLARATION_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(&format!(".{suffix}")))
    })
}

/// The declaration file a relative import of `base` names when no module is there: `base.d.ts` (`./types`), the
/// declaration file of a script extension written (`./types.js` for `types.d.ts`), or the directory's `index.d.ts`.
fn declaration_module(fs: &dyn FileSystem, base: &Path) -> Option<PathBuf> {
    let name = base.file_name()?.to_string_lossy().into_owned();
    let script = base
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| SCRIPT_EXTENSIONS.contains(&extension));
    let stem = base.file_stem()?.to_string_lossy().into_owned();
    let mut candidates = Vec::new();
    for suffix in DECLARATION_SUFFIXES {
        if script {
            candidates.push(base.with_file_name(format!("{stem}.{suffix}")));
        }
        candidates.push(base.with_file_name(format!("{name}.{suffix}")));
        candidates.push(base.join(format!("index.{suffix}")));
    }
    candidates.into_iter().find(|candidate| fs.is_file(candidate))
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
            .with_file(
                "/ws/packages/tools/package.json",
                r#"{"name":"@app/tools","main":"dist/index.js"}"#,
            )
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
            ("npm:vite@5", true),
            ("@app/tools", false),
            ("@app/alias", false),
            ("lodash", false),
            ("./local", false),
        ] {
            assert_eq!(modules.is_installed(from, specifier), installed, "{specifier}");
        }
    }

    /// A `node_modules` directory that holds only the caches of tools (`.vite`, `.cache`) holds no package, and one
    /// above the workspace root says nothing of what the workspace installs: as with none, every name of no workspace
    /// package is installed, and a package such a directory holds still is.
    #[test]
    fn caches_and_node_modules_above_the_root_hold_no_install_of_the_workspace() {
        let fs = project()
            .with_file("/ws/node_modules/.vite/deps/_metadata.json", "{}")
            .with_file("/ws/packages/app/node_modules/.cache/babel/x.json", "{}")
            .with_file("/node_modules/left-pad/package.json", "{}");
        let discovered = workspace(&fs);
        let modules = ConfigModules::new(&fs, &discovered);
        let from = Path::new(CONFIG);
        for (specifier, installed) in [
            ("vite", true),
            ("vitest/config", true),
            ("astro/config", true),
            ("left-pad", true),
            ("@app/alias", true),
            ("@app/tools", false),
        ] {
            assert_eq!(modules.is_installed(from, specifier), installed, "{specifier}");
        }
        let with_vite = fs.with_file("/ws/node_modules/vite/package.json", "{}");
        let discovered = workspace(&with_vite);
        let modules = ConfigModules::new(&with_vite, &discovered);
        for (specifier, installed) in [("vite", true), ("@app/alias", false), ("left-pad", true)] {
            assert_eq!(modules.is_installed(from, specifier), installed, "{specifier}");
        }
    }

    /// Under Yarn Plug'n'Play, a dependency the importing package or the root declares from a registry is installed;
    /// one linked from a directory of the project and an undeclared name (an alias) are not.
    #[test]
    fn under_plug_n_play_a_declared_dependency_is_installed() {
        let fs = MemoryFs::new()
            .with_file(
                "/ws/package.json",
                r#"{"name":"root","workspaces":["packages/*"],"devDependencies":{"vite":"^5.0.0"}}"#,
            )
            .with_file(
                "/ws/packages/app/package.json",
                r#"{"name":"app","dependencies":{"@vitejs/plugin-react":"^4.0.0","@app/linked":"link:../linked"}}"#,
            )
            .with_file(CONFIG, "export default {};")
            .with_file(
                "/ws/packages/tools/package.json",
                r#"{"name":"@app/tools","main":"dist/index.js"}"#,
            )
            .with_file(
                "/ws/packages/other/package.json",
                r#"{"name":"other","dependencies":{"lodash":"^4.0.0"}}"#,
            )
            .with_file("/ws/.pnp.cjs", "")
            .with_file("/ws/node_modules/.cache/x.json", "{}");
        let workspace = workspace(&fs);
        let modules = ConfigModules::new(&fs, &workspace);
        let from = Path::new(CONFIG);
        for (specifier, installed) in [
            ("vite", true),
            ("vite/client", true),
            ("@vitejs/plugin-react", true),
            ("node:path", true),
            ("lodash", false),
            ("@app/alias", false),
            ("@app/linked", false),
            ("@app/tools", false),
        ] {
            assert_eq!(modules.is_installed(from, specifier), installed, "{specifier}");
        }
    }

    /// A relative import of a declaration file, JSON, a stylesheet or an asset holds no code; one of a code file, of
    /// nothing, or a bare specifier is not known to.
    #[test]
    fn a_relative_import_of_a_file_that_is_not_code_holds_no_code() {
        let fs = project()
            .with_file("/ws/packages/app/types.d.ts", "export interface A {}")
            .with_file("/ws/packages/app/decl/index.d.ts", "export interface B {}")
            .with_file("/ws/packages/app/x.css", ".a {}")
            .with_file("/ws/packages/app/logo.svg", "<svg></svg>")
            .with_file("/ws/packages/app/code.ts", "export const a = 1;");
        let workspace = workspace(&fs);
        let modules = ConfigModules::new(&fs, &workspace);
        let from = Path::new(CONFIG);
        for (specifier, holds_none) in [
            ("./types", true),
            ("./types.js", true),
            ("./decl", true),
            ("./package.json", true),
            ("./x.css", true),
            ("./logo.svg", true),
            ("./code", false),
            ("./missing", false),
            ("vite", false),
        ] {
            assert_eq!(modules.holds_no_code(from, specifier), holds_none, "{specifier}");
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
