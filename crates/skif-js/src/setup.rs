//! Per-package analysis settings: configured, default and plugin-provided globs merged once.

use std::path::PathBuf;
use std::sync::Arc;

use skif_config::{Config, Js};
use skif_core::FileSystem;
use skif_workspace::{PackageId, Workspace};

use crate::globs::{Globs, Names};
use crate::input::{InputRequest, InputSource, WorkspaceInputs};
use crate::scripts::{self, ScriptFacts};

const EXTENSIONS: &str = "{js,mjs,cjs,jsx,ts,tsx,mts,cts}";

fn default_entry() -> Vec<String> {
    vec![
        format!("{{index,cli,main}}.{EXTENSIONS}"),
        format!("src/{{index,cli,main}}.{EXTENSIONS}"),
    ]
}

fn default_project() -> Vec<String> {
    vec![format!("**/*.{EXTENSIONS}")]
}

fn test_files() -> Vec<String> {
    vec![
        format!("**/*.{{test,spec}}.{EXTENSIONS}"),
        format!("**/__tests__/**/*.{EXTENSIONS}"),
    ]
}

/// What the user configured for one package.
#[derive(Default)]
struct Configured {
    entry: Vec<String>,
    project: Vec<String>,
    ignore: Vec<String>,
    ignore_dependencies: Vec<String>,
    ignore_binaries: Vec<String>,
}

fn normalize_dir(dir: &str) -> &str {
    dir.trim_start_matches("./").trim_end_matches('/')
}

fn configured(js: &Js, dir: &str, is_root: bool) -> Configured {
    if is_root {
        return Configured {
            entry: js.entry.clone(),
            project: js.project.clone(),
            ignore: js.ignore.clone(),
            ignore_dependencies: js.ignore_dependencies.clone(),
            ignore_binaries: js.ignore_binaries.clone(),
        };
    }
    js.workspaces
        .iter()
        .find(|(key, _)| normalize_dir(key) == dir)
        .map(|(_, workspace)| Configured {
            entry: workspace.entry.clone(),
            project: workspace.project.clone(),
            ignore: workspace.ignore.clone(),
            ignore_dependencies: workspace.ignore_dependencies.clone(),
            ignore_binaries: workspace.ignore_binaries.clone(),
        })
        .unwrap_or_default()
}

/// The settings of one workspace package.
pub(crate) struct PackageSetup {
    pub id: PackageId,
    /// Absolute package directory.
    pub dir: PathBuf,
    pub entry: Globs,
    pub project: Globs,
    /// Files whose issues are not reported, relative to the package directory.
    pub ignore: Globs,
    pub ignore_dependencies: Names,
    pub ignore_binaries: Names,
    /// Packages referenced by plugins and by scripts.
    pub referenced_dependencies: Vec<String>,
    /// Binaries run by plugins.
    pub referenced_binaries: Vec<String>,
    pub scripts: ScriptFacts,
}

/// Settings for the whole workspace.
pub(crate) struct Setup {
    /// Indexed by package; `None` for packages excluded with `ignore-packages`.
    pub packages: Vec<Option<PackageSetup>>,
    /// `[js] ignore`, relative to the workspace root.
    pub global_ignore: Globs,
    /// Root-level `ignore-dependencies` and `ignore-binaries` apply to every package.
    pub global_ignore_dependencies: Names,
    pub global_ignore_binaries: Names,
    pub production: bool,
    pub include_entry_exports: bool,
}

impl Setup {
    pub(crate) fn new(
        workspace: &Workspace,
        config: &Config,
        fs: &Arc<dyn FileSystem>,
        sources: &[Arc<dyn InputSource>],
    ) -> Self {
        let js = &config.js;
        let production = js.production;
        let packages = workspace
            .iter()
            .map(|(id, package)| {
                if workspace.is_ignored(id) {
                    return None;
                }
                let dir_name = package.dir.to_string_lossy().replace('\\', "/");
                let user = configured(js, &dir_name, package.is_root);
                let mut plugin = WorkspaceInputs::default();
                let request = InputRequest {
                    workspace,
                    package,
                    fs: fs.as_ref(),
                };
                for source in sources {
                    plugin.extend(source.inputs(&request));
                }
                let scripts = scripts::analyze(package);

                let mut entry = if user.entry.is_empty() { default_entry() } else { user.entry };
                if !production {
                    entry.extend(test_files());
                    entry.extend(plugin.entry);
                }
                entry.extend(plugin.production_entry);

                let mut project = if user.project.is_empty() { default_project() } else { user.project };
                project.extend(plugin.project);
                if production {
                    project.extend(test_files().into_iter().map(|glob| format!("!{glob}")));
                }

                let mut ignore = user.ignore;
                ignore.extend(plugin.ignore);
                let mut ignore_dependencies = user.ignore_dependencies;
                ignore_dependencies.extend(plugin.ignore_dependencies);
                let mut ignore_binaries = user.ignore_binaries;
                ignore_binaries.extend(plugin.ignore_binaries);

                let mut referenced_dependencies = plugin.dependencies;
                referenced_dependencies.extend(scripts.dependencies.iter().cloned());
                Some(PackageSetup {
                    id,
                    dir: workspace.root.join(&package.dir),
                    entry: Globs::new(entry),
                    project: Globs::new(project),
                    ignore: Globs::new(ignore),
                    ignore_dependencies: Names::new(ignore_dependencies),
                    ignore_binaries: Names::new(ignore_binaries),
                    referenced_dependencies,
                    referenced_binaries: plugin.binaries,
                    scripts,
                })
            })
            .collect();
        Self {
            packages,
            global_ignore: Globs::new(&js.ignore),
            global_ignore_dependencies: Names::new(&js.ignore_dependencies),
            global_ignore_binaries: Names::new(&js.ignore_binaries),
            production,
            include_entry_exports: js.include_entry_exports,
        }
    }

    pub(crate) fn package(&self, id: PackageId) -> Option<&PackageSetup> {
        self.packages.get(id.index()).and_then(Option::as_ref)
    }

    /// Whether the dependency `name` of package `id` is on an ignore list.
    pub(crate) fn ignores_dependency(&self, id: PackageId, name: &str) -> bool {
        self.global_ignore_dependencies.is_match(name)
            || self.package(id).is_some_and(|setup| setup.ignore_dependencies.is_match(name))
    }

    /// Whether the binary `name` of package `id` is on an ignore list.
    pub(crate) fn ignores_binary(&self, id: PackageId, name: &str) -> bool {
        self.global_ignore_binaries.is_match(name) || self.package(id).is_some_and(|setup| setup.ignore_binaries.is_match(name))
    }
}
