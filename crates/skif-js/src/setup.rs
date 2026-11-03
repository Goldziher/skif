//! Per-package analysis settings: configured, default and plugin-provided globs merged once.

use std::path::PathBuf;
use std::sync::Arc;

use globset::Glob;
use skif_config::{Config, Js, JsWorkspace};
use skif_core::FileSystem;
use skif_workspace::{PackageId, Workspace};

use crate::globs::{Globs, Names};
use crate::input::{BuildDir, InputRequest, InputSource, PathAlias, WorkspaceInputs};
use crate::scripts::{self, ScriptFacts};

const EXTENSIONS: &str = "{js,mjs,cjs,jsx,ts,tsx,mts,cts}";

fn default_entry() -> Vec<String> {
    vec![
        format!("{{index,cli,main}}.{EXTENSIONS}"),
        format!("src/{{index,cli,main}}.{EXTENSIONS}"),
    ]
}

/// Ambient declaration files load without being imported.
fn ambient_declarations() -> Vec<String> {
    vec!["**/*.d.{ts,mts,cts}".to_owned()]
}

/// Directories that hold tests and end-to-end suites, which tools run without any file importing them.
fn test_directories() -> Vec<String> {
    vec![
        format!("{{test,tests,e2e,cypress}}/**/*.{EXTENSIONS}"),
        format!("src/{{test,tests,e2e}}/**/*.{EXTENSIONS}"),
        format!("**/__mocks__/**/*.{EXTENSIONS}"),
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
    workspace_override(js, dir)
        .map(|workspace| Configured {
            entry: workspace.entry.clone(),
            project: workspace.project.clone(),
            ignore: workspace.ignore.clone(),
            ignore_dependencies: workspace.ignore_dependencies.clone(),
            ignore_binaries: workspace.ignore_binaries.clone(),
        })
        .unwrap_or_default()
}

/// The `[js.workspaces]` table for `dir`: an exact key wins over a glob key such as `packages/*`.
fn workspace_override<'a>(js: &'a Js, dir: &str) -> Option<&'a JsWorkspace> {
    js.workspaces
        .iter()
        .find(|(key, _)| normalize_dir(key) == dir)
        .or_else(|| {
            js.workspaces.iter().find(|(key, _)| {
                let key = normalize_dir(key);
                key.contains(['*', '?', '[', '{'])
                    && Glob::new(key).is_ok_and(|glob| glob.compile_matcher().is_match(dir))
            })
        })
        .map(|(_, workspace)| workspace)
}

/// The settings of one workspace package.
pub(crate) struct PackageSetup {
    pub id: PackageId,
    /// Absolute package directory.
    pub dir: PathBuf,
    pub entry: Globs,
    /// Entry globs that point outside the package directory (`../shared/setup.ts`), relative to the workspace root.
    pub external_entry: Globs,
    /// The tool configuration of the package could not be fully read, so its entry files are not all known.
    /// Files and dependencies cannot be judged unused then.
    pub entries_incomplete: bool,
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
    /// Tool configuration files, absolute.
    pub config_files: Vec<PathBuf>,
    /// Aliases declared by the package's tool configuration.
    pub aliases: Vec<PathAlias>,
    pub build_dirs: Vec<BuildDir>,
    /// Extra `exports` conditions that select source files.
    pub conditions: Vec<String>,
    /// Directories bare specifiers are looked up in, relative to the package directory.
    pub module_dirs: Vec<String>,
    /// File extensions (with the leading dot) imports may omit, from bundler `resolve.extensions`.
    pub extensions: Vec<String>,
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
    /// `JSDoc` tags that exempt an export from `unused-exports` and `unused-types`.
    pub ignore_tags: Vec<String>,
}

/// `pattern`, written relative to the package directory `dir`, relative to the workspace root instead. `None` when it
/// leaves the workspace. A leading `!` is kept.
fn workspace_relative(dir: &str, pattern: &str) -> Option<String> {
    let (negation, body) = pattern.strip_prefix('!').map_or(("", pattern), |rest| ("!", rest));
    let mut parts: Vec<&str> = Vec::new();
    for segment in dir.split('/').chain(body.split('/')) {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(format!("{negation}{}", parts.join("/")))
}

/// Options of `unused-exports`, which `unused-types` shares.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct UnusedExportsOptions {
    /// `JSDoc` tags that exempt an export.
    pub ignore_tags: Vec<String>,
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
        let ignore_tags = config
            .rule("unused-exports")
            .and_then(|rule| skif_lint::parse_options::<UnusedExportsOptions>(&rule.options).ok())
            .map(|options| options.ignore_tags)
            .unwrap_or_default();
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

                let mut entry = if user.entry.is_empty() {
                    default_entry()
                } else {
                    user.entry
                };
                entry.extend(ambient_declarations());
                if !production {
                    entry.extend(test_files());
                    entry.extend(test_directories());
                    entry.extend(plugin.entry);
                }
                entry.extend(plugin.production_entry);

                let mut project = if user.project.is_empty() {
                    default_project()
                } else {
                    user.project
                };
                project.extend(plugin.project);
                if production {
                    project.extend(
                        test_files()
                            .into_iter()
                            .chain(test_directories())
                            .map(|glob| format!("!{glob}")),
                    );
                }

                let mut ignore = user.ignore;
                ignore.extend(plugin.ignore);
                let mut ignore_dependencies = user.ignore_dependencies;
                ignore_dependencies.extend(plugin.ignore_dependencies);
                let mut ignore_binaries = user.ignore_binaries;
                ignore_binaries.extend(plugin.ignore_binaries);

                let mut referenced_dependencies = plugin.dependencies;
                referenced_dependencies.extend(scripts.dependencies.iter().cloned());
                let (outside, entry): (Vec<String>, Vec<String>) = entry
                    .into_iter()
                    .partition(|pattern| pattern.trim_start_matches('!').starts_with("../"));
                let external_entry = Globs::new(
                    outside
                        .iter()
                        .filter_map(|pattern| workspace_relative(&dir_name, pattern)),
                );
                Some(PackageSetup {
                    id,
                    dir: workspace.root.join(&package.dir),
                    entry: Globs::new(entry),
                    external_entry,
                    entries_incomplete: plugin.entries_incomplete,
                    project: Globs::new(project),
                    ignore: Globs::new(ignore),
                    ignore_dependencies: Names::new(ignore_dependencies),
                    ignore_binaries: Names::new(ignore_binaries),
                    referenced_dependencies,
                    referenced_binaries: plugin.binaries,
                    scripts,
                    config_files: plugin.config_files,
                    aliases: plugin.aliases,
                    build_dirs: plugin.build_dirs,
                    conditions: plugin.conditions,
                    module_dirs: plugin.module_dirs,
                    extensions: plugin.extensions,
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
            ignore_tags,
        }
    }

    pub(crate) fn package(&self, id: PackageId) -> Option<&PackageSetup> {
        self.packages.get(id.index()).and_then(Option::as_ref)
    }

    /// Whether the dependency `name` of package `id` is on an ignore list.
    pub(crate) fn ignores_dependency(&self, id: PackageId, name: &str) -> bool {
        self.global_ignore_dependencies.is_match(name)
            || self
                .package(id)
                .is_some_and(|setup| setup.ignore_dependencies.is_match(name))
    }

    /// Whether the binary `name` of package `id` is on an ignore list.
    pub(crate) fn ignores_binary(&self, id: PackageId, name: &str) -> bool {
        self.global_ignore_binaries.is_match(name)
            || self
                .package(id)
                .is_some_and(|setup| setup.ignore_binaries.is_match(name))
    }
}

#[cfg(test)]
mod tests {
    use super::workspace_relative;

    #[test]
    fn patterns_outside_the_package_become_workspace_relative() {
        assert_eq!(
            workspace_relative("apps/a", "../../shared/setup.ts").as_deref(),
            Some("shared/setup.ts")
        );
        assert_eq!(workspace_relative("a", "!../b/**").as_deref(), Some("!b/**"));
        assert_eq!(workspace_relative("a", "../../escape.ts"), None);
    }
}
