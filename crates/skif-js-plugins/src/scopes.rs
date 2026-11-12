//! Nx projects as scopes of the package that owns them (ADR 0109).
//!
//! An Nx project is a directory with a `project.json`. In an integrated workspace it has no `package.json`: its
//! dependencies are listed by the package that holds it, usually the root, and its tools (`vite.config.ts`,
//! `eslint.config.mjs`, `jest.config.ts`, `tsconfig.json`) are configured in its own directory. Each such project is a
//! scope: the plugins run there as they run for a package, and what they find is rebased onto the owning package.

use std::path::PathBuf;

use indexmap::IndexMap;
use skif_config::PluginConfig;

use crate::files::find_files_within;
use crate::plugin::{IncompleteScope, PluginContext, PluginOutput, ScopedEntryRegex};
use crate::support::{join_rel, path_within};

/// How deep below a package [`NxProjects::find`] looks for a `project.json`: as deep as skif-js walks for sources, as
/// a project may lie deeper than the configs the plugins look for.
const PROJECT_DEPTH: usize = 48;

/// The Nx projects below a package, and where the package lies in the Nx workspace (ADR 0109).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct NxProjects {
    /// The package directory relative to the Nx workspace root, `/`-separated; `""` for the root package. The paths of
    /// a `project.json` are relative to the workspace root.
    pub path: String,
    /// The directory of every `project.json` below the package, those of packages below it included, relative to the
    /// package directory and sorted; `""` for the package's own. Nx's `getJestProjects()` returns their Jest configs.
    pub all: Vec<String>,
    /// The directories of `all` whose `project.json` the `nx` plugin reads for the package: its own (`""`) and its
    /// `scopes`. A project of a package below it is that package's to read.
    pub dirs: Vec<String>,
    /// The project directories with no `package.json` in them or between them and the package directory: the
    /// package lists their dependencies, and every enabled plugin also runs in them.
    pub scopes: Vec<String>,
}

impl NxProjects {
    /// Every `project.json` below the package of `ctx` (outside `node_modules`), when the `nx` plugin is enabled
    /// there; none otherwise. The package is taken to be the Nx workspace root. Nothing is left out by `.gitignore`
    /// files or as build output: [`NxProjects::from_dirs`] takes the projects a file walk with those rules found.
    #[must_use]
    pub fn find(ctx: &PluginContext<'_>) -> Self {
        Self::from_dirs(ctx, "", || Self::found_dirs(ctx))
    }

    /// The projects [`Self::find`] lists, each with whether the package of `ctx` owns it, as
    /// [`from_dirs`](Self::from_dirs) takes them.
    pub(crate) fn found_dirs(ctx: &PluginContext<'_>) -> Vec<(String, bool)> {
        project_dirs(ctx)
            .into_iter()
            .map(|dir| {
                let owned = !has_own_manifest(ctx, &dir);
                (dir, owned)
            })
            .collect()
    }

    /// The projects of the package of `ctx`, which lies at `path` in the Nx workspace, when the `nx` plugin is enabled
    /// there; none otherwise, without calling `projects`. `projects` gives the directory of each `project.json` below
    /// the package, relative to its directory (`""` for its own), and whether the package owns it: no other package's
    /// `package.json` lies in the directory or between it and the package directory.
    #[must_use]
    pub fn from_dirs(ctx: &PluginContext<'_>, path: &str, projects: impl FnOnce() -> Vec<(String, bool)>) -> Self {
        if !crate::resolve::is_plugin_enabled(ctx, crate::plugins::nx::ID) {
            return Self::default();
        }
        Self::listed(path, projects())
    }

    /// The projects `projects` of a package at `path` in which the `nx` plugin is enabled; see
    /// [`from_dirs`](Self::from_dirs).
    pub(crate) fn listed(path: &str, mut projects: Vec<(String, bool)>) -> Self {
        projects.sort();
        projects.dedup();
        let dirs: Vec<String> = projects
            .iter()
            .filter(|(dir, owned)| dir.is_empty() || *owned)
            .map(|(dir, _)| dir.clone())
            .collect();
        Self {
            path: path.to_owned(),
            all: projects.into_iter().map(|(dir, _)| dir).collect(),
            scopes: dirs.iter().filter(|dir| !dir.is_empty()).cloned().collect(),
            dirs,
        }
    }
}

/// The directories, relative to the package directory and sorted, that hold a `project.json`; `""` for the package's
/// own.
pub(crate) fn project_dirs(ctx: &PluginContext<'_>) -> Vec<String> {
    find_files_within(
        ctx.fs,
        ctx.globs,
        ctx.dir,
        &["**/project.json".to_owned()],
        PROJECT_DEPTH,
    )
    .iter()
    .map(|path| path.rsplit_once('/').map_or("", |(dir, _)| dir).to_owned())
    .collect()
}

/// `path`, relative to the directory of the package at `package` in the Nx workspace, without a detour through
/// directories above the package that leads back into it: `../../packages/b/run.mjs` in `packages/b` is `run.mjs`.
pub(crate) fn within_package(package: &str, path: &str) -> String {
    if package.is_empty() || !path.starts_with("..") {
        return path.to_owned();
    }
    path_within(&join_rel(package, path), package)
        .filter(|inside| !inside.is_empty())
        .unwrap_or_else(|| path.to_owned())
}

/// Whether `dir`, or a directory between it and the package directory, holds a `package.json`: the project belongs
/// to that package.
fn has_own_manifest(ctx: &PluginContext<'_>, dir: &str) -> bool {
    let mut current = String::new();
    dir.split('/').any(|part| {
        current = join_rel(&current, part);
        ctx.fs.is_file(&ctx.dir.join(&current).join("package.json"))
    })
}

/// What every scope of a package shares: no scripts (they belong to the owning package) and only the switches of the
/// plugin table, since the globs a table gives are relative to the owning package.
pub(crate) struct ScopeShared {
    scripts: IndexMap<String, String>,
    plugins: IndexMap<String, PluginConfig>,
}

impl ScopeShared {
    pub(crate) fn new(owner: &PluginContext<'_>) -> Self {
        Self {
            scripts: IndexMap::new(),
            plugins: switches(owner.plugins),
        }
    }

    /// The context the plugins run in for the scope whose absolute directory is `dir`, of the package `owner`: the
    /// project directory as the package directory and the owning package's dependencies.
    pub(crate) fn context<'a>(&'a self, owner: &PluginContext<'a>, dir: &'a std::path::Path) -> PluginContext<'a> {
        PluginContext {
            dir,
            is_root: false,
            package_name: None,
            dependencies: owner.dependencies,
            inherited_dependencies: owner.inherited_dependencies,
            scripts: &self.scripts,
            fs: owner.fs,
            plugins: &self.plugins,
            modules: owner.modules,
            nx: None,
            globs: owner.globs,
            sources: None,
        }
    }
}

/// The absolute directory of the scope `dir` of the package `owner`.
pub(crate) fn scope_dir(owner: &PluginContext<'_>, dir: &str) -> PathBuf {
    crate::support::join_lexically(owner.dir, dir)
}

/// The plugin table reduced to its switches: `true`, `false`, or the `enabled` of a table.
fn switches(plugins: &IndexMap<String, PluginConfig>) -> IndexMap<String, PluginConfig> {
    plugins
        .iter()
        .filter_map(|(id, setting)| {
            let enabled = match setting {
                PluginConfig::Enabled(enabled) => Some(*enabled),
                PluginConfig::Options(options) => options.enabled,
                _ => None,
            };
            enabled.map(|enabled| (id.clone(), PluginConfig::Enabled(enabled)))
        })
        .collect()
}

/// `output`, which a plugin produced for the scope in `scope`, with its paths, globs and expressions made relative to
/// the owning package. Entries a config of the scope hides lie in the scope, unless the plugin bounds them further.
pub(crate) fn rebase(mut output: PluginOutput, scope: &str) -> PluginOutput {
    let globs = |list: &mut Vec<String>| {
        for glob in list.iter_mut() {
            *glob = rebase_glob(scope, glob);
        }
    };
    globs(&mut output.entries);
    globs(&mut output.project);
    globs(&mut output.production_entries);
    globs(&mut output.build_sources);
    globs(&mut output.production_build_sources);
    globs(&mut output.ignore);
    globs(&mut output.module_dirs);
    // An expression is matched against the path below the scope, which the regex crate cannot express in the
    // expression itself without look-around.
    let regexes = std::mem::take(&mut output.entry_regexes);
    output
        .scoped_entry_regexes
        .extend(regexes.into_iter().map(|regex| ScopedEntryRegex::new(scope, regex)));
    for alias in &mut output.aliases {
        if let Some(target) = &mut alias.target {
            *target = join_rel(scope, target);
        }
    }
    for dir in &mut output.build_dirs {
        dir.out_dir = join_rel(scope, &dir.out_dir);
        dir.root_dir = join_rel(scope, &dir.root_dir);
    }
    for run in &mut output.run_binaries {
        if let Some(dir) = &mut run.dir {
            *dir = join_rel(scope, dir);
        }
    }
    for dependency in &mut output.referenced_dependencies {
        if let Some(dir) = &mut dependency.dir {
            *dir = join_rel(scope, dir);
        }
    }
    for reason in &mut output.incomplete_reasons {
        reason.scope = match &reason.scope {
            IncompleteScope::Within(within) => {
                IncompleteScope::Within(within.iter().map(|glob| rebase_glob(scope, glob)).collect())
            }
            _ => IncompleteScope::Within(vec![format!("{scope}/**")]),
        };
    }
    output
}

/// Folds `from`, the rebased output of a scope, into `into`, the output of the same plugin for the owning package.
pub(crate) fn merge(into: &mut PluginOutput, from: PluginOutput) {
    let lists = [
        (&mut into.entries, from.entries),
        (&mut into.project, from.project),
        (&mut into.production_entries, from.production_entries),
        (&mut into.build_sources, from.build_sources),
        (&mut into.production_build_sources, from.production_build_sources),
        (&mut into.entry_regexes, from.entry_regexes),
        (&mut into.referenced_binaries, from.referenced_binaries),
        (&mut into.provided_binaries, from.provided_binaries),
        (&mut into.ignore, from.ignore),
        (&mut into.conditions, from.conditions),
        (&mut into.virtual_modules, from.virtual_modules),
        (&mut into.module_dirs, from.module_dirs),
        (&mut into.extensions, from.extensions),
    ];
    for (list, items) in lists {
        extend_unique(list, items);
    }
    extend_unique(&mut into.scoped_entry_regexes, from.scoped_entry_regexes);
    extend_unique(&mut into.config_files, from.config_files);
    extend_unique(&mut into.run_binaries, from.run_binaries);
    extend_unique(&mut into.aliases, from.aliases);
    extend_unique(&mut into.build_dirs, from.build_dirs);
    extend_unique(&mut into.incomplete_reasons, from.incomplete_reasons);
    for reference in from.referenced_dependencies {
        match into.referenced_dependencies.iter_mut().find(|existing| {
            existing.name == reference.name && existing.from == reference.from && existing.dir == reference.dir
        }) {
            Some(existing) => existing.merge(&reference),
            None => into.referenced_dependencies.push(reference),
        }
    }
    into.custom_elements = into.custom_elements.or(from.custom_elements);
    into.entries_incomplete |= from.entries_incomplete;
}

fn extend_unique<T: PartialEq>(list: &mut Vec<T>, items: Vec<T>) {
    for item in items {
        if !list.contains(&item) {
            list.push(item);
        }
    }
}

/// A glob relative to the scope `scope`, relative to the owning package; a negation stays one.
fn rebase_glob(scope: &str, glob: &str) -> String {
    match glob.strip_prefix('!') {
        Some(negated) => format!("!{}", rebase_glob(scope, negated)),
        None => join_rel(scope, glob),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_are_rebased_onto_the_scope_and_negations_stay_negations() {
        assert_eq!(rebase_glob("apps/web", "src/**/*.ts"), "apps/web/src/**/*.ts");
        assert_eq!(rebase_glob("apps/web", "./index.html"), "apps/web/index.html");
        assert_eq!(
            rebase_glob("apps/web", "!**/*.stories.tsx"),
            "!apps/web/**/*.stories.tsx"
        );
        assert_eq!(rebase_glob("apps/web", "../../jest.preset.js"), "jest.preset.js");
    }

    #[test]
    fn a_detour_through_the_workspace_root_is_taken_out_of_a_path() {
        assert_eq!(within_package("packages/b", "../../packages/b/run.mjs"), "run.mjs");
        assert_eq!(within_package("packages/b", "../../tools/r.mjs"), "../../tools/r.mjs");
        assert_eq!(within_package("packages/b", "../c/x.mjs"), "../c/x.mjs");
        assert_eq!(within_package("packages/b", "src/x.mjs"), "src/x.mjs");
        assert_eq!(within_package("", "../x.mjs"), "../x.mjs");
    }

    /// A scope's expressions keep their text and are moved out of `entry_regexes`, with the scope beside them.
    #[test]
    fn entry_expressions_keep_their_scope() {
        let output = PluginOutput {
            entry_regexes: vec![r".*\.spec\.ts$".to_owned(), "^/e2e/".to_owned()],
            ..PluginOutput::default()
        };
        let rebased = rebase(output, "apps/web");
        assert_eq!(rebased.entry_regexes, Vec::<String>::new());
        assert_eq!(
            rebased.scoped_entry_regexes,
            [
                ScopedEntryRegex::new("apps/web", r".*\.spec\.ts$"),
                ScopedEntryRegex::new("apps/web", "^/e2e/"),
            ]
        );
    }
}
