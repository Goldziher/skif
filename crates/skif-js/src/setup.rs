//! Per-package analysis settings: configured, default and plugin-provided globs merged once.

use skif_core::HashSet;
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::{Glob, GlobBuilder, GlobMatcher};
use indexmap::IndexMap;
use skif_config::{Config, IgnoreExportsUsedInFile, IgnoreItem, JsWorkspace, PackageJs};
use skif_core::FileSystem;
use skif_js_syntax::{Export, ExportKind, Registration};
use skif_workspace::{Package, PackageId, Workspace, map_ordered};

use crate::ancestors;
use crate::config_check::relative_tsconfig;
use crate::deps::member_dirs;
use crate::globs::{EntryRegexes, Globs, IncompleteScopeGlobs, Names};
use crate::graph::ModuleGraph;
use crate::input::{
    BuildDir, ConfigDependency, CustomElementLibrary, IncompleteCause, IncompleteEntries, IncompleteScope,
    InputRequest, InputSource, PathAlias, RunBinary, WorkspaceInputs, WorkspaceListing,
};
use crate::listed::{self, Listed};
use crate::resolve::join_normalized;
use crate::scripts::{self, ScriptBinary, ScriptFacts};
use crate::tsconfig_files;

const EXTENSIONS: &str = "{js,mjs,cjs,jsx,ts,tsx,mts,cts}";

/// The extensions of project files: scripts and the single-file components the resolver follows.
const PROJECT_EXTENSIONS: &str = "{js,mjs,cjs,jsx,ts,tsx,mts,cts,vue,svelte,astro}";

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

/// Directories that hold tests and end-to-end suites, which tools run without any file importing them. They are
/// entries only through a test-runner plugin; here they are excluded from the project in production mode.
fn test_directories() -> Vec<String> {
    vec![
        format!("{{test,tests,e2e,cypress}}/**/*.{EXTENSIONS}"),
        format!("src/{{test,tests,e2e}}/**/*.{EXTENSIONS}"),
        format!("**/__mocks__/**/*.{EXTENSIONS}"),
    ]
}

fn default_project() -> Vec<String> {
    vec![format!("**/*.{PROJECT_EXTENSIONS}")]
}

/// Test file names, excluded from the project in production mode.
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
    ignore_files: Vec<String>,
    ignore_dependencies: Vec<String>,
    ignore_binaries: Vec<String>,
    ignore_unresolved: Vec<String>,
    ignore_members: Vec<String>,
    include_entry_exports: Option<bool>,
    ignore_exports_used_in_file: Option<IgnoreExportsUsedInFile>,
    paths: IndexMap<String, Vec<String>>,
}

/// The names of the entries of an ignore list, without their reasons.
fn names(items: &[IgnoreItem]) -> Vec<String> {
    items.iter().map(|item| item.name.clone()).collect()
}

/// The directory of `package` relative to its root, `.` for the root package.
fn own_dir(package: &Package) -> String {
    if package.is_root {
        ".".to_owned()
    } else {
        package.dir.to_string_lossy().replace('\\', "/")
    }
}

fn normalize_dir(dir: &str) -> &str {
    let dir = dir.trim_start_matches("./").trim_end_matches('/');
    if dir.is_empty() { "." } else { dir }
}

/// `dir`, a package directory relative to its root (`.` or empty for the root package), as the `[js.workspaces]` keys
/// and `ignore-workspaces` globs of a root match it: joined to `anchor`, the root's directory relative to the directory
/// the keys are written from ([`Workspace::workspace_keys_anchor`]), and unchanged without one.
fn anchored<'a>(anchor: Option<&str>, dir: &'a str) -> Cow<'a, str> {
    let dir = normalize_dir(dir);
    match anchor.map(normalize_dir) {
        None | Some(".") => Cow::Borrowed(dir),
        Some(anchor) if dir == "." => Cow::Owned(anchor.to_owned()),
        Some(anchor) => Cow::Owned(format!("{anchor}/{dir}")),
    }
}

impl Configured {
    /// Applies the `js` keys of an override: the plain keys replace what is there, the `extend-` keys append.
    fn fold(&mut self, js: &PackageJs) {
        let replace = |target: &mut Vec<String>, value: &Option<Vec<String>>| {
            if let Some(value) = value {
                target.clone_from(value);
            }
        };
        let replace_names = |target: &mut Vec<String>, value: &Option<Vec<IgnoreItem>>| {
            if let Some(value) = value {
                *target = names(value);
            }
        };
        replace(&mut self.entry, &js.entry);
        replace(&mut self.project, &js.project);
        replace(&mut self.ignore, &js.ignore);
        replace_names(&mut self.ignore_dependencies, &js.ignore_dependencies);
        replace_names(&mut self.ignore_binaries, &js.ignore_binaries);
        replace_names(&mut self.ignore_unresolved, &js.ignore_unresolved);
        self.ignore_unresolved.extend(names(&js.extend_ignore_unresolved));
        self.ignore.extend(js.extend_ignore.iter().cloned());
        self.ignore_dependencies.extend(names(&js.extend_ignore_dependencies));
        self.ignore_binaries.extend(names(&js.extend_ignore_binaries));
    }
}

impl From<&JsWorkspace> for Configured {
    fn from(workspace: &JsWorkspace) -> Self {
        Self {
            entry: workspace.entry.clone(),
            project: workspace.project.clone(),
            ignore: workspace.ignore.clone(),
            ignore_files: names(&workspace.ignore_files),
            ignore_dependencies: names(&workspace.ignore_dependencies),
            ignore_binaries: names(&workspace.ignore_binaries),
            ignore_unresolved: names(&workspace.ignore_unresolved),
            ignore_members: names(&workspace.ignore_members),
            include_entry_exports: workspace.include_entry_exports,
            ignore_exports_used_in_file: workspace.ignore_exports_used_in_file.clone(),
            paths: workspace.paths.clone(),
        }
    }
}

impl Configured {
    /// Applies the `"."` table of the root workspace on top of the top-level `[js]` keys: a list it sets replaces the
    /// top-level one, the ignore lists add to the top-level ones (those apply to every workspace anyway).
    fn fold_root(&mut self, root: Configured) {
        let replace = |target: &mut Vec<String>, value: Vec<String>| {
            if !value.is_empty() {
                *target = value;
            }
        };
        replace(&mut self.entry, root.entry);
        replace(&mut self.project, root.project);
        self.ignore.extend(root.ignore);
        self.ignore_files.extend(root.ignore_files);
        self.ignore_dependencies.extend(root.ignore_dependencies);
        self.ignore_binaries.extend(root.ignore_binaries);
        self.ignore_unresolved.extend(root.ignore_unresolved);
        self.ignore_members.extend(root.ignore_members);
        self.include_entry_exports = root.include_entry_exports.or(self.include_entry_exports);
        self.ignore_exports_used_in_file = root
            .ignore_exports_used_in_file
            .or(self.ignore_exports_used_in_file.take());
        self.paths.extend(root.paths);
    }
}

/// What the configuration sets for the package `name` in `abs_dir`: the root package takes the top-level `[js]` and the
/// `"."` table of `[js.workspaces]`, any other the matching `[js.workspaces]` table (an exact key wins over a glob
/// key), then the `js` of each `[[overrides]]` block that selects the package, in configuration order.
///
/// The top-level `ignore-dependencies`, `ignore-binaries`, `ignore-unresolved` and `ignore-members` apply to every
/// package through the global lists and are not copied into the root's own.
fn configured(
    config: &Config,
    keys: &WorkspaceKeys,
    name: &str,
    abs_dir: &Path,
    dir: &str,
    is_root: bool,
) -> Configured {
    let js = &config.js;
    let mut user = if is_root {
        let mut root = Configured {
            entry: js.entry.clone(),
            project: js.project.clone(),
            ignore: js.ignore.clone(),
            ..Configured::default()
        };
        if let Some(table) = keys.table(&js.workspaces, ".") {
            root.fold_root(Configured::from(table));
        }
        root
    } else {
        keys.table(&js.workspaces, dir)
            .map(Configured::from)
            .unwrap_or_default()
    };
    for scoped in config
        .overrides
        .iter()
        .filter(|scoped| !scoped.js.is_empty() && scoped.applies_to(Some((name, abs_dir)), None))
    {
        user.fold(&scoped.js);
    }
    user
}

/// The `[js.workspaces]` keys, each compiled once: an exact key wins over a glob key such as `packages/*`, and among
/// several glob keys that select a directory the first in file order wins. Like every other glob, `*` stops at `/`.
#[derive(Debug, Clone, Default)]
pub(crate) struct WorkspaceKeys {
    keys: Vec<WorkspaceKey>,
    /// The root's directory relative to the start of the run, when the keys are written relative to the start.
    anchor: Option<String>,
}

#[derive(Debug, Clone)]
struct WorkspaceKey {
    /// The key without a leading `./` or a trailing `/`.
    dir: String,
    /// The compiled glob, for a key that is one.
    glob: Option<GlobMatcher>,
}

impl WorkspaceKey {
    fn selects(&self, dir: &str) -> bool {
        self.dir == dir || (dir != "." && self.glob.as_ref().is_some_and(|glob| glob.is_match(dir)))
    }
}

impl WorkspaceKeys {
    pub(crate) fn new(workspaces: &IndexMap<String, JsWorkspace>) -> Self {
        let keys = workspaces
            .keys()
            .map(|key| {
                let dir = normalize_dir(key);
                let glob = dir
                    .contains(['*', '?', '[', '{'])
                    .then(|| GlobBuilder::new(dir).literal_separator(true).build().ok())
                    .flatten()
                    .map(|glob| glob.compile_matcher());
                WorkspaceKey {
                    dir: dir.to_owned(),
                    glob,
                }
            })
            .collect();
        Self { keys, anchor: None }
    }

    /// The same keys matched from `anchor` (see [`anchored`]).
    #[must_use]
    pub(crate) fn with_anchor(mut self, anchor: Option<&str>) -> Self {
        self.anchor = anchor.map(str::to_owned);
        self
    }

    /// The directory of `package` as the keys select it and findings spell it: `.` for the root package and its
    /// directory otherwise, joined to the anchor (see [`anchored`]).
    pub(crate) fn key_dir(&self, package: &Package) -> String {
        anchored(self.anchor.as_deref(), &own_dir(package)).into_owned()
    }

    /// The key, as written in `workspaces`, whose table configures `package`, as [`Self::table`] picks it.
    pub(crate) fn package_key<'a>(
        &self,
        workspaces: &'a IndexMap<String, JsWorkspace>,
        package: &Package,
    ) -> Option<&'a String> {
        self.key(workspaces, &own_dir(package))
    }

    /// The table of `workspaces` that stands for `dir` (`"."` or empty for the root). `workspaces` is the map the
    /// keys were compiled from.
    pub(crate) fn table<'a>(
        &self,
        workspaces: &'a IndexMap<String, JsWorkspace>,
        dir: &str,
    ) -> Option<&'a JsWorkspace> {
        self.entry(workspaces, dir).map(|(_, workspace)| workspace)
    }

    /// The key, as written in `workspaces`, whose table stands for `dir`, as [`Self::table`] picks it.
    pub(crate) fn key<'a>(&self, workspaces: &'a IndexMap<String, JsWorkspace>, dir: &str) -> Option<&'a String> {
        self.entry(workspaces, dir).map(|(key, _)| key)
    }

    fn entry<'a>(
        &self,
        workspaces: &'a IndexMap<String, JsWorkspace>,
        dir: &str,
    ) -> Option<(&'a String, &'a JsWorkspace)> {
        let dir = anchored(self.anchor.as_deref(), dir);
        let dir: &str = &dir;
        let position = self
            .keys
            .iter()
            .position(|key| key.dir == dir)
            .or_else(|| self.keys.iter().position(|key| key.selects(dir)))?;
        workspaces.get_index(position)
    }

    /// The keys, as written in `workspaces`, that select none of `dirs`.
    fn unmatched<'a>(&self, workspaces: &'a IndexMap<String, JsWorkspace>, dirs: &[String]) -> Vec<&'a String> {
        workspaces
            .keys()
            .zip(&self.keys)
            .filter(|(_, key)| {
                !dirs
                    .iter()
                    .any(|dir| key.selects(&anchored(self.anchor.as_deref(), dir)))
            })
            .map(|(text, _)| text)
            .collect()
    }
}

/// The `paths` map of a configuration as module aliases: `alias/*` becomes the prefix `alias` pointing at the target
/// directory, one alias per target.
fn path_aliases(paths: &IndexMap<String, Vec<String>>) -> Vec<PathAlias> {
    let trim = |pattern: &str| {
        let pattern = pattern
            .strip_suffix("/*")
            .or_else(|| pattern.strip_suffix('*'))
            .unwrap_or(pattern);
        pattern.trim_start_matches("./").to_owned()
    };
    paths
        .iter()
        .flat_map(|(alias, targets)| {
            let find = trim(alias);
            targets.iter().map(move |target| {
                let target = trim(target);
                PathAlias {
                    find: find.clone(),
                    target: Some(if target.is_empty() { ".".to_owned() } else { target }),
                }
            })
        })
        .collect()
}

/// Whether `setting` covers the kind of `export`.
pub(crate) fn ignores_in_file_use(setting: &IgnoreExportsUsedInFile, export: &Export) -> bool {
    match setting {
        IgnoreExportsUsedInFile::All(all) => *all,
        IgnoreExportsUsedInFile::Kinds(kinds) => match export.kind {
            ExportKind::Value => kinds.variable,
            ExportKind::Function => kinds.function,
            ExportKind::Class => kinds.class,
            ExportKind::Enum => kinds.enumeration,
            ExportKind::Namespace => kinds.namespace,
            ExportKind::Type => kinds.type_alias || kinds.interface,
            _ => false,
        },
        _ => false,
    }
}

/// The settings of one workspace package.
pub(crate) struct PackageSetup {
    pub id: PackageId,
    /// Absolute package directory.
    pub dir: PathBuf,
    /// The `[js.workspaces]` key, as written, whose table configures the package; `None` when no table does.
    pub table_key: Option<String>,
    pub entry: Globs,
    /// Entry globs that point outside the package directory (`../shared/setup.ts`), relative to the workspace root.
    pub external_entry: Globs,
    /// Entry globs from plugins and tool configuration. Unlike the defaults they also match files that belong to a
    /// workspace package nested below this one: a root `playwright.config.ts` names the tests of the packages.
    pub shared_entry: Globs,
    /// The entry globs plugins provide inside the package, for the `entry-redundant` hint of `stale-js-config`.
    pub plugin_entry: Globs,
    /// The plugin entries that name one file rather than a glob (a workflow step's `node build/report.ts`), absolute.
    /// One that is no file of this analysis may belong to another root of a multi-root run.
    pub plugin_entry_files: Vec<PathBuf>,
    /// The tool configuration of the package could not be fully read, so its entry files are not all known.
    /// Files and dependencies cannot be judged unused then.
    pub entries_incomplete: bool,
    /// Why the entry files are incomplete; not empty when they are.
    pub incomplete_reasons: Vec<IncompleteEntries>,
    /// When the entry files are incomplete, the globs (relative to the package directory) of the only files that may
    /// be unknown entries; `None` when they may be anywhere. The matching project files are treated as entries and
    /// every other file is judged unused as usual (ADR 0065).
    pub incomplete_scope: Option<IncompleteScopeGlobs>,
    /// The module (relative to the workspace root) and 1-based line of a computed `import()` or `require()` in the
    /// package, which may load any of its files: the narrowed scope a config gave then leaves the package unjudged
    /// as a whole (see [`Self::unjudged_package_wide`]).
    pub widened_by: Option<(PathBuf, u32)>,
    pub project: Globs,
    /// Files whose issues are not reported, relative to the package directory.
    pub ignore: Globs,
    /// Files never reported unused, relative to the package directory (`ignore-files` of the workspace).
    pub ignore_files: Globs,
    /// Dependencies and binaries plugins say are never reported.
    pub ignore_dependencies: Names,
    pub ignore_binaries: Names,
    /// The `ignore-dependencies`, `ignore-binaries`, `ignore-unresolved` and `ignore-members` of this workspace.
    pub user_ignore_dependencies: Listed,
    pub user_ignore_binaries: Listed,
    pub ignore_unresolved: Listed,
    pub ignore_members: Listed,
    /// The `entry`, `project` and `ignore` patterns the configuration gives this package (no plugin or default
    /// ones), for the configuration hints of `stale-js-config`.
    pub user_patterns: UserPatterns,
    /// Whether unused exports of entry files are reported (`include-entry-exports`, workspace over top level).
    pub include_entry_exports: bool,
    /// Which exports used in their own file are not reported.
    pub used_in_file: IgnoreExportsUsedInFile,
    /// Packages referenced by plugins and by scripts.
    pub referenced_dependencies: Vec<String>,
    /// The packages that the tool configs plugins read name themselves, with the config file.
    pub config_dependencies: Vec<ConfigDependency>,
    /// Binaries run by plugins.
    pub referenced_binaries: Vec<String>,
    /// The binaries that commands in the files plugins read run, with the file.
    pub run_binaries: Vec<RunBinary>,
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
    /// Custom element libraries of the enabled plugins, whose registered classes are used.
    pub custom_elements: Vec<CustomElementLibrary>,
}

impl PackageSetup {
    /// Whether `rel` (relative to the package directory) may be an entry that a config of the package hides, so it
    /// is treated as an entry; never when the hidden entries may be anywhere.
    pub(crate) fn in_incomplete_scope(&self, rel: &str) -> bool {
        self.entries_incomplete && self.incomplete_scope.as_ref().is_some_and(|scope| scope.is_match(rel))
    }

    /// Whether no file of the package can be judged unused: its entries are incomplete and may be anywhere, or a
    /// computed load may reach any of its files.
    pub(crate) fn unjudged_package_wide(&self) -> bool {
        self.entries_incomplete && (self.incomplete_scope.is_none() || self.widened_by.is_some())
    }

    /// Whether the file `rel` (relative to the package directory) and its exports can be judged unused: the package's
    /// entries are known, or the entries its configs hide lie in a narrowed scope `rel` is outside of (ADR 0065,
    /// ADR 0081).
    pub(crate) fn judges(&self, rel: &str) -> bool {
        !self.unjudged_package_wide() && !self.in_incomplete_scope(rel)
    }
}

/// The `entry`, `project` and `ignore` patterns a package is configured with, as written.
#[derive(Debug, Clone, Default)]
pub(crate) struct UserPatterns {
    pub entry: Vec<String>,
    pub project: Vec<String>,
    pub ignore: Vec<String>,
    pub ignore_files: Vec<String>,
}

/// Settings for the whole workspace.
#[allow(
    clippy::struct_excessive_bools,
    reason = "each bool is an independent switch of the analysis"
)]
pub(crate) struct Setup {
    /// Indexed by package; `None` for packages excluded with `ignore-packages`.
    pub packages: Vec<Option<PackageSetup>>,
    /// `[js] ignore`, relative to the workspace root.
    pub global_ignore: Globs,
    /// `[js] ignore-files`, relative to the workspace root.
    pub global_ignore_files: Globs,
    /// Root-level `ignore-dependencies`, `ignore-binaries`, `ignore-unresolved` and `ignore-members` apply to every
    /// package.
    pub global_ignore_dependencies: Listed,
    pub global_ignore_binaries: Listed,
    pub global_ignore_unresolved: Listed,
    pub global_ignore_members: Listed,
    /// `[js] ignore-workspaces`, matched against package directories.
    pub ignore_workspaces: Listed,
    /// Packages the JavaScript analysis skips because `ignore-workspaces` names them.
    pub js_ignored: HashSet<PackageId>,
    /// Every `[js.workspaces]` key, as written, and whether it selects no package.
    pub workspace_key_verdicts: Vec<(String, bool)>,
    /// The `[js.workspaces]` keys, matched from the workspace's [`Workspace::workspace_keys_anchor`].
    pub workspace_keys: WorkspaceKeys,
    pub production: bool,
    /// `[js] tsconfig`, absolute: the TypeScript configuration of the root package in place of its `tsconfig.json`.
    pub root_tsconfig: Option<PathBuf>,
    /// knip's strict mode (`--strict`): each package uses only what its own manifest lists, production code only its
    /// `dependencies` and `peerDependencies`, and type-only imports use no package.
    pub isolate_workspaces: bool,
    pub include_entry_exports: bool,
    /// `[js] include-fixtures`: files in fixture directories are reported as unused like any other.
    pub include_fixtures: bool,
    /// `[js] ignore-exports-used-in-file`.
    pub used_in_file: IgnoreExportsUsedInFile,
    /// `JSDoc` tags that exempt an export from `unused-exports` and `unused-types`.
    pub ignore_tags: Vec<String>,
    /// `JSDoc` tags an export must carry to be reported at all; empty reports every export (knip's `+tag`).
    pub include_tags: Vec<String>,
    /// `[js.cycles]`.
    pub cycles: skif_config::JsCycles,
    /// `[js] ignore-global-binaries`: binaries assumed to be installed globally are never reported unlisted.
    pub ignore_global_binaries: bool,
    /// `.gitignore` files apply: not `--no-gitignore`.
    pub gitignore: bool,
}

/// The TypeScript configuration of the package in `dir`: `root_tsconfig` (`[js] tsconfig`) for the root package,
/// `tsconfig.json` otherwise.
pub(crate) fn tsconfig_path(root_tsconfig: Option<&Path>, dir: &Path, is_root: bool) -> PathBuf {
    match root_tsconfig {
        Some(config) if is_root => config.to_path_buf(),
        _ => dir.join("tsconfig.json"),
    }
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

/// A workspace named by a script (`yarn workspace web`, `pnpm --filter "@scope/*"`, `npm -w packages/web`), read
/// once: it selects a package by name, by name pattern or by directory. Exclusions (`!web`) select nothing.
struct WorkspaceTarget<'a> {
    /// The target without the `...` of a dependency or dependent selector; `None` for an exclusion.
    name: Option<&'a str>,
    /// The directory the target names, when it is written as a path.
    dir: Option<&'a str>,
    /// The compiled name pattern, when the target is one.
    pattern: Option<GlobMatcher>,
}

impl<'a> WorkspaceTarget<'a> {
    fn new(target: &'a str) -> Self {
        let target = target.trim_start_matches("...").trim_end_matches("...");
        if target.starts_with('!') {
            return Self {
                name: None,
                dir: None,
                pattern: None,
            };
        }
        let is_path = target.starts_with('.') || (target.contains('/') && !target.starts_with('@'));
        Self {
            name: Some(target),
            dir: is_path.then(|| normalize_dir(target)),
            pattern: target
                .contains(['*', '?', '[', '{'])
                .then(|| Glob::new(target).ok())
                .flatten()
                .map(|glob| glob.compile_matcher()),
        }
    }

    /// Whether the target is `package`, whose directory relative to the workspace root, `/`-separated, is `dir`.
    fn names(&self, package: &Package, dir: &str) -> bool {
        let Some(target) = self.name else {
            return false;
        };
        let name = package.name.as_deref();
        name == Some(target)
            || self.dir == Some(dir)
            || name.is_some_and(|name| self.pattern.as_ref().is_some_and(|pattern| pattern.is_match(name)))
    }
}

/// Credits the binaries a package's scripts run in other, named workspaces to those workspaces, as binaries a listed
/// provider is used for and that are never unlisted: whether the name is a binary or a script of the target is
/// not known from the caller.
fn forward_workspace_binaries(packages: &mut [Option<PackageSetup>], workspace: &Workspace) {
    let mut credits: Vec<(PackageId, ScriptBinary)> = Vec::new();
    // The directories and targets are read once, not once for every forwarded binary and package.
    let dirs = package_dirs(workspace);
    for setup in packages.iter().flatten() {
        for forwarded in &setup.scripts.forwarded {
            let targets: Vec<WorkspaceTarget<'_>> = forwarded
                .targets
                .iter()
                .map(|target| WorkspaceTarget::new(target))
                .collect();
            for ((id, target), (_, dir)) in workspace.iter().zip(&dirs) {
                let runs_script = target
                    .manifest
                    .value()
                    .get("scripts")
                    .and_then(|scripts| scripts.get(&forwarded.binary.name))
                    .is_some();
                if !runs_script && targets.iter().any(|selector| selector.names(target, dir)) {
                    credits.push((id, forwarded.binary.clone()));
                }
            }
        }
    }
    for (id, binary) in credits {
        if let Some(Some(setup)) = packages.get_mut(id.index())
            && !setup.scripts.binaries.contains(&binary)
            && !setup.scripts.ephemeral_binaries.contains(&binary)
        {
            setup.scripts.ephemeral_binaries.push(binary);
        }
    }
}

/// Credits what a package's scripts run in another directory (`cd sub && rspack`) to the workspace package that
/// directory is in, as [`forward_workspace_binaries`] credits a named workspace: a listed provider there is used and
/// nothing is unlisted. A directory in the package itself, outside every package or outside the workspace leaves the
/// command where it was written: judged as a command of the package, unless it also targets another workspace
/// (`pnpm -C dir exec`).
fn relocate_script_commands(packages: &mut [Option<PackageSetup>], workspace: &Workspace) {
    let dirs = package_dirs(workspace);
    let owner = |from: PackageId, dir: &str| package_in_dir(&dirs, from, dir);
    let mut credits: Vec<(PackageId, ScriptBinary)> = Vec::new();
    let mut dependencies: Vec<(PackageId, String)> = Vec::new();
    for setup in packages.iter_mut().flatten() {
        let from = setup.id;
        for relocated in std::mem::take(&mut setup.scripts.relocated_binaries) {
            let target = owner(from, &relocated.dir);
            if target != from {
                credits.push((target, relocated.binary));
            } else if !relocated.filtered {
                let list = if relocated.ephemeral {
                    &mut setup.scripts.ephemeral_binaries
                } else {
                    &mut setup.scripts.binaries
                };
                if !list.contains(&relocated.binary) {
                    list.push(relocated.binary);
                }
            }
        }
        for relocated in std::mem::take(&mut setup.scripts.relocated_dependencies) {
            dependencies.push((owner(from, &relocated.dir), relocated.name));
        }
    }
    for (id, binary) in credits {
        if let Some(Some(setup)) = packages.get_mut(id.index())
            && !setup.scripts.binaries.contains(&binary)
            && !setup.scripts.ephemeral_binaries.contains(&binary)
        {
            setup.scripts.ephemeral_binaries.push(binary);
        }
    }
    for (id, name) in dependencies {
        if let Some(Some(setup)) = packages.get_mut(id.index())
            && !setup.referenced_dependencies.contains(&name)
        {
            setup.referenced_dependencies.push(name);
        }
    }
}

/// Every package with its workspace-relative directory, `/`-separated, for [`package_in_dir`].
pub(crate) fn package_dirs(workspace: &Workspace) -> Vec<(PackageId, String)> {
    workspace
        .iter()
        .map(|(id, package)| (id, package.dir.to_string_lossy().replace('\\', "/")))
        .collect()
}

/// The innermost package of `dirs` ([`package_dirs`]) that holds `dir`, a directory relative to the package `from`;
/// `from` itself when `dir` leaves the workspace root.
pub(crate) fn package_in_dir(dirs: &[(PackageId, String)], from: PackageId, dir: &str) -> PackageId {
    let base = dirs
        .iter()
        .find(|(id, _)| *id == from)
        .map_or("", |(_, base)| base.as_str());
    let Some(target) = join_dir(base, dir) else {
        return from;
    };
    dirs.iter()
        .filter(|(_, candidate)| contains_dir(candidate, &target))
        .max_by_key(|(_, candidate)| candidate.len())
        .map_or(from, |(id, _)| *id)
}

/// `dir` joined onto the workspace-relative directory `base`, `/`-separated with `.` and `..` resolved; `None` when it
/// leaves the workspace root.
fn join_dir(base: &str, dir: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in base.split('/').chain(dir.split('/')) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// Whether the workspace-relative directory `dir` is `ancestor` or below it; every directory is below the root, `""`.
fn contains_dir(ancestor: &str, dir: &str) -> bool {
    ancestor.is_empty()
        || dir
            .strip_prefix(ancestor)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Whether an entry glob from a plugin may match files of nested workspace packages. A glob that starts with `**/`
/// is a per-package default (every package has its own test files) and one that leaves the package is handled as
/// an external entry.
fn is_shared_pattern(pattern: &str) -> bool {
    let body = pattern.trim_start_matches('!').trim_start_matches("./");
    !body.starts_with("**/") && !body.starts_with("../") && !body.starts_with('/')
}

/// Whether an entry pattern names one path rather than a glob or a negation.
fn is_literal_path(pattern: &str) -> bool {
    !pattern.is_empty() && !pattern.contains(['*', '?', '[', ']', '{', '}', '!'])
}

/// Options of `unused-exports`, which `unused-types` shares.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub(crate) struct UnusedExportsOptions {
    /// `JSDoc` tags that exempt an export.
    pub ignore_tags: Vec<String>,
    /// `JSDoc` tags that limit the report to the exports carrying one of them.
    pub include_tags: Vec<String>,
}

impl Setup {
    pub(crate) fn new(
        workspace: &Workspace,
        config: &Config,
        fs: &Arc<dyn FileSystem>,
        sources: &[Arc<dyn InputSource>],
        sequential: bool,
    ) -> Self {
        let js = &config.js;
        let production = js.production;
        let ancestor_dependencies = ancestors::external_dependencies(fs.as_ref(), &workspace.root);
        let UnusedExportsOptions {
            ignore_tags,
            include_tags,
        } = config
            .rule("unused-exports")
            .and_then(|rule| skif_lint::parse_options::<UnusedExportsOptions>(&rule.options).ok())
            .unwrap_or_default();
        let ignore_workspaces = Listed::directories(&js.ignore_workspaces, production);
        let workspace_keys = WorkspaceKeys::new(&js.workspaces).with_anchor(workspace.workspace_keys_anchor());
        let js_ignored: HashSet<PackageId> = workspace
            .iter()
            .filter(|(_, package)| !package.is_root)
            .filter(|(_, package)| ignore_workspaces.is_match(&workspace_keys.key_dir(package)))
            .map(|(id, _)| id)
            .collect();
        let package_dirs: Vec<String> = workspace
            .iter()
            .map(|(_, package)| {
                if package.is_root {
                    ".".to_owned()
                } else {
                    package.dir.to_string_lossy().replace('\\', "/")
                }
            })
            .collect();
        let unmatched = workspace_keys.unmatched(&js.workspaces, &package_dirs);
        let workspace_key_verdicts: Vec<(String, bool)> = js
            .workspaces
            .keys()
            .map(|key| (key.clone(), unmatched.contains(&key)))
            .collect();
        let root_tsconfig = js
            .tsconfig
            .as_deref()
            .and_then(relative_tsconfig)
            .map(|path| join_normalized(&workspace.root, &path));
        let named_members = if js.use_tsconfig_files {
            member_dirs(workspace)
        } else {
            Vec::new()
        };
        let listing = WorkspaceListing::new(!js.no_gitignore, sequential);
        let members: Vec<(PackageId, &Package)> = workspace.iter().collect();
        let mut packages: Vec<Option<PackageSetup>> = map_ordered(&members, sequential, |&(id, package)| {
            if workspace.is_ignored(id) || js_ignored.contains(&id) {
                return None;
            }
            let dir_name = package.dir.to_string_lossy().replace('\\', "/");
            let abs_dir = workspace.root.join(&package.dir);
            let match_dir = if package.is_root {
                workspace.root.as_path()
            } else {
                abs_dir.as_path()
            };
            let user = configured(
                config,
                &workspace_keys,
                package.display_name(),
                match_dir,
                &dir_name,
                package.is_root,
            );
            let mut plugin = WorkspaceInputs::default();
            let workspace_key = workspace_keys.key_dir(package);
            let table_key = workspace_keys.package_key(&js.workspaces, package).cloned();
            let request = InputRequest {
                workspace,
                package,
                fs: fs.as_ref(),
                ancestor_dependencies: &ancestor_dependencies,
                workspace_key: &workspace_key,
                listing: &listing,
            };
            for source in sources {
                plugin.extend(source.inputs(&request));
            }
            // Entry regular expressions are test entries, which production mode does not use.
            let entry_regex = if production {
                None
            } else {
                EntryRegexes::new(&plugin.entry_regex, &plugin.scoped_entry_regex)
            };
            let scripts = scripts::analyze(package);

            let mut entry = listed::user_globs(&user.entry, production, &default_entry());
            entry.extend(ambient_declarations());
            let mut shared_sources: Vec<&String> = plugin.production_entry.iter().collect();
            if !production {
                shared_sources.extend(plugin.entry.iter());
            }
            let shared_entry: Vec<String> = shared_sources
                .into_iter()
                .filter(|pattern| is_shared_pattern(pattern))
                .cloned()
                .collect();
            let mut plugin_entry: Vec<String> = plugin.production_entry.clone();
            if !production {
                plugin_entry.extend(plugin.entry.iter().cloned());
                entry.extend(plugin.entry);
            }
            entry.extend(plugin.production_entry);
            let plugin_entry_files: Vec<PathBuf> = plugin_entry
                .iter()
                .filter(|pattern| is_literal_path(pattern))
                .map(|pattern| join_normalized(&abs_dir, pattern))
                .collect();
            plugin_entry.retain(|pattern| !pattern.trim_start_matches('!').starts_with("../"));

            let tsconfig_project = js
                .use_tsconfig_files
                .then(|| {
                    let config = tsconfig_path(root_tsconfig.as_deref(), &abs_dir, package.is_root);
                    tsconfig_files::project_globs(fs.as_ref(), &named_members, &config, &abs_dir)
                })
                .flatten();
            let from_tsconfig = tsconfig_project.is_some();
            let (mut project, mut named_files) = match tsconfig_project {
                Some(selected) => (selected.patterns, selected.files),
                None => (
                    listed::user_globs(&user.project, production, &default_project()),
                    Vec::new(),
                ),
            };
            if production {
                let mut exclusions = listed::production_exclusions(&plugin.project);
                exclusions.extend(listed::non_production_entries(&user.entry));
                exclusions.extend(
                    test_files()
                        .into_iter()
                        .chain(test_directories())
                        .map(|glob| format!("!{glob}")),
                );
                // Production mode leaves out tests and the like even when a `tsconfig` names them in `files`.
                if !named_files.is_empty() {
                    named_files.extend(exclusions.iter().cloned());
                }
                project.extend(exclusions);
            } else if !from_tsconfig {
                // knip adds no plugin project globs to the files a `tsconfig` selects either.
                project.extend(plugin.project);
            }

            let user_patterns = UserPatterns {
                entry: user.entry.clone(),
                project: user.project.clone(),
                ignore: user.ignore.clone(),
                ignore_files: user.ignore_files.clone(),
            };
            let mut ignore = user.ignore;
            ignore.extend(plugin.ignore);
            let mut ignore_dependencies = plugin.ignore_dependencies;
            ignore_dependencies.extend(plugin.virtual_modules);
            let ignore_binaries = plugin.ignore_binaries;
            let mut aliases = path_aliases(&user.paths);
            aliases.extend(plugin.aliases);

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
                dir: abs_dir,
                table_key,
                entry: Globs::new(entry).with_regexes(entry_regex.clone()),
                external_entry,
                shared_entry: Globs::new(shared_entry),
                plugin_entry: Globs::new(plugin_entry).with_regexes(entry_regex),
                plugin_entry_files,
                entries_incomplete: plugin.entries_incomplete,
                incomplete_scope: match &plugin.incomplete_scope {
                    IncompleteScope::Within(patterns) if plugin.entries_incomplete => {
                        Some(IncompleteScopeGlobs::new(patterns.clone()))
                    }
                    _ => None,
                },
                widened_by: None,
                incomplete_reasons: if plugin.entries_incomplete && plugin.incomplete_reasons.is_empty() {
                    vec![IncompleteEntries::new(IncompleteCause::Unspecified)]
                } else {
                    plugin.incomplete_reasons
                },
                project: Globs::new(project).or(Globs::new(named_files)),
                ignore: Globs::new(ignore),
                ignore_files: Globs::new(&user.ignore_files),
                ignore_dependencies: Names::new(ignore_dependencies),
                ignore_binaries: Names::new(ignore_binaries),
                user_ignore_dependencies: Listed::new(&user.ignore_dependencies, production),
                user_ignore_binaries: Listed::new(&user.ignore_binaries, production),
                ignore_unresolved: Listed::new(&user.ignore_unresolved, production),
                ignore_members: Listed::new(&user.ignore_members, production),
                user_patterns,
                include_entry_exports: user.include_entry_exports.unwrap_or(js.include_entry_exports),
                used_in_file: user
                    .ignore_exports_used_in_file
                    .unwrap_or_else(|| js.ignore_exports_used_in_file.clone()),
                referenced_dependencies,
                config_dependencies: plugin.config_dependencies,
                referenced_binaries: plugin.binaries,
                run_binaries: plugin.run_binaries,
                scripts,
                config_files: plugin.config_files,
                aliases,
                build_dirs: plugin.build_dirs,
                conditions: plugin.conditions,
                module_dirs: plugin.module_dirs,
                extensions: plugin.extensions,
                custom_elements: plugin.custom_elements,
            })
        });
        forward_workspace_binaries(&mut packages, workspace);
        relocate_script_commands(&mut packages, workspace);
        Self {
            packages,
            global_ignore: Globs::new(&js.ignore),
            global_ignore_files: Globs::new(&js.ignore_files),
            global_ignore_dependencies: Listed::new(&js.ignore_dependencies, production),
            global_ignore_binaries: Listed::new(&js.ignore_binaries, production),
            global_ignore_unresolved: Listed::new(&js.ignore_unresolved, production),
            global_ignore_members: Listed::new(&js.ignore_members, production),
            ignore_workspaces,
            js_ignored,
            workspace_key_verdicts,
            workspace_keys,
            production,
            root_tsconfig,
            isolate_workspaces: js.isolate_workspaces,
            include_entry_exports: js.include_entry_exports,
            include_fixtures: js.include_fixtures,
            used_in_file: js.ignore_exports_used_in_file.clone(),
            ignore_tags,
            include_tags,
            cycles: js.cycles.clone(),
            ignore_global_binaries: js.ignore_global_binaries,
            gitignore: !js.no_gitignore,
        }
    }

    /// The TypeScript configuration of `package`: `[js] tsconfig` for the root, `tsconfig.json` otherwise.
    pub(crate) fn tsconfig_of(&self, workspace: &Workspace, package: &Package) -> PathBuf {
        tsconfig_path(
            self.root_tsconfig.as_deref(),
            &workspace.root.join(&package.dir),
            package.is_root,
        )
    }

    pub(crate) fn package(&self, id: PackageId) -> Option<&PackageSetup> {
        self.packages.get(id.index()).and_then(Option::as_ref)
    }

    /// Records, for each incomplete package with a narrowed scope, the first module that loads a module by a computed
    /// path: the scope bounds what its configs may hide, but such a load may reach any file of the package, so none
    /// can be judged unused. The files of the scope stay entries.
    pub(crate) fn widen_incomplete_scopes(&mut self, graph: &ModuleGraph) {
        for module in graph
            .modules()
            .iter()
            .filter(|module| module.in_project && !module.foreign)
        {
            let Some(span) = module.info.unresolved_dynamic.first() else {
                continue;
            };
            if let Some(Some(package)) = self.packages.get_mut(module.owner.index())
                && package.incomplete_scope.is_some()
                && package.widened_by.is_none()
            {
                package.widened_by = Some((module.path.clone(), span.start_line));
            }
        }
    }

    /// Marks the packages whose entries the manifest leaves unknown, for the cause found for each.
    pub(crate) fn mark_entries_incomplete(&mut self, unknown: &[(PackageId, IncompleteCause)]) {
        for &(id, cause) in unknown {
            if let Some(Some(package)) = self.packages.get_mut(id.index()) {
                package.entries_incomplete = true;
                package.incomplete_scope = None;
                package
                    .incomplete_reasons
                    .push(IncompleteEntries::new(cause).with_file(package.dir.join("package.json")));
            }
        }
    }

    /// Whether the dependency `name` of package `id` is on an ignore list. Every list is consulted, so each entry that
    /// matches is recorded.
    pub(crate) fn ignores_dependency(&self, id: PackageId, name: &str) -> bool {
        let global = self.global_ignore_dependencies.is_match(name);
        let package = self.package(id).is_some_and(|setup| {
            let user = setup.user_ignore_dependencies.is_match(name);
            setup.ignore_dependencies.is_match(name) || user
        });
        global || package
    }

    /// Whether the binary `name` of package `id` is on an ignore list.
    pub(crate) fn ignores_binary(&self, id: PackageId, name: &str) -> bool {
        let global = self.global_ignore_binaries.is_match(name);
        let package = self.package(id).is_some_and(|setup| {
            let user = setup.user_ignore_binaries.is_match(name);
            setup.ignore_binaries.is_match(name) || user
        });
        global || package
    }

    /// Whether the import specifier `specifier` in package `id` is on an `ignore-unresolved` list.
    pub(crate) fn ignores_unresolved(&self, id: PackageId, specifier: &str) -> bool {
        let global = self.global_ignore_unresolved.is_match(specifier);
        let package = self
            .package(id)
            .is_some_and(|setup| setup.ignore_unresolved.is_match(specifier));
        global || package
    }

    /// Whether the enum or namespace member `name` in package `id` is on an `ignore-members` list.
    pub(crate) fn ignores_member(&self, id: PackageId, name: &str) -> bool {
        self.global_ignore_members.is_match(name)
            || self
                .package(id)
                .is_some_and(|setup| setup.ignore_members.is_match(name))
    }

    /// Whether unused exports of entry files owned by package `id` are reported.
    pub(crate) fn includes_entry_exports(&self, id: PackageId) -> bool {
        self.package(id)
            .map_or(self.include_entry_exports, |setup| setup.include_entry_exports)
    }

    /// Whether an export of package `id` that is used in its own file is left unreported.
    pub(crate) fn ignores_in_file_use(&self, id: PackageId, export: &Export) -> bool {
        let setting = self.package(id).map_or(&self.used_in_file, |setup| &setup.used_in_file);
        ignores_in_file_use(setting, export)
    }

    /// Whether `export` of a module of package `id` is a registered custom element (ADR 0062):
    /// `customElements.define` always counts, a decorator or base class only when it comes from the library of a
    /// plugin enabled for the package.
    pub(crate) fn is_registered(&self, id: PackageId, export: &Export) -> bool {
        let libraries = self
            .package(id)
            .map_or(&[][..], |setup| setup.custom_elements.as_slice());
        export.registrations.iter().any(|registration| match registration {
            Registration::Define => true,
            Registration::Decorator { module, name } => libraries
                .iter()
                .any(|library| library.decorator == *name && library.is_module(module)),
            Registration::BaseClassDefine { module, name } => libraries
                .iter()
                .any(|library| library.base_class.as_deref() == Some(name.as_str()) && library.is_module(module)),
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use indexmap::IndexMap;
    use skif_config::{Config, JsWorkspace};
    use skif_core::MemoryFs;
    use skif_workspace::Workspace;

    use super::{WorkspaceKeys, WorkspaceTarget, anchored, is_shared_pattern, package_dirs, workspace_relative};

    fn tables(keys: &[&str]) -> IndexMap<String, JsWorkspace> {
        keys.iter()
            .map(|key| ((*key).to_owned(), JsWorkspace::default()))
            .collect()
    }

    #[test]
    fn package_directories_are_joined_to_the_anchor() {
        for (anchor, dir, expected) in [
            (None, "packages/a", "packages/a"),
            (None, "", "."),
            (Some("."), "packages/a", "packages/a"),
            (Some("tools/sub"), ".", "tools/sub"),
            (Some("tools/sub"), "", "tools/sub"),
            (Some("./tools/sub/"), "packages/a", "tools/sub/packages/a"),
        ] {
            assert_eq!(anchored(anchor, dir), expected, "{anchor:?} {dir}");
        }
    }

    #[test]
    fn without_an_anchor_keys_select_from_the_root() {
        let workspaces = tables(&[".", "tools/sub", "packages/*"]);
        let keys = WorkspaceKeys::new(&workspaces);
        assert_eq!(keys.key(&workspaces, ".").map(String::as_str), Some("."));
        assert_eq!(
            keys.key(&workspaces, "packages/a").map(String::as_str),
            Some("packages/*")
        );
        assert_eq!(
            keys.unmatched(&workspaces, &[".".to_owned()]),
            ["tools/sub", "packages/*"]
        );
    }

    #[test]
    fn anchored_keys_select_the_directories_they_name_below_the_start() {
        let workspaces = tables(&[".", "tools/sub", "tools/sub/packages/*", "tools/*", "apps/*"]);
        let keys = WorkspaceKeys::new(&workspaces).with_anchor(Some("tools/sub"));
        // The exact key of the root's directory wins over the glob `tools/*`; `"."` is the start, not this root.
        assert_eq!(keys.key(&workspaces, ".").map(String::as_str), Some("tools/sub"));
        assert_eq!(keys.key(&workspaces, "").map(String::as_str), Some("tools/sub"));
        assert_eq!(
            keys.key(&workspaces, "packages/a").map(String::as_str),
            Some("tools/sub/packages/*")
        );
        assert_eq!(keys.key(&workspaces, "apps/web"), None);
        assert_eq!(
            keys.unmatched(&workspaces, &[".".to_owned(), "packages/a".to_owned()]),
            [".", "apps/*"]
        );
    }

    #[test]
    fn only_rooted_plugin_globs_are_shared_with_nested_packages() {
        assert!(is_shared_pattern("apps/web/playwright/**/*.ts"));
        assert!(is_shared_pattern("!apps/web/legacy/**"));
        assert!(!is_shared_pattern("**/*.test.ts"));
        assert!(!is_shared_pattern("../shared/setup.ts"));
    }

    #[test]
    fn patterns_outside_the_package_become_workspace_relative() {
        assert_eq!(
            workspace_relative("apps/a", "../../shared/setup.ts").as_deref(),
            Some("shared/setup.ts")
        );
        assert_eq!(workspace_relative("a", "!../b/**").as_deref(), Some("!b/**"));
        assert_eq!(workspace_relative("a", "../../escape.ts"), None);
    }

    #[test]
    #[allow(clippy::unwrap_used)]
    fn script_targets_select_packages_by_name_pattern_and_directory() {
        let fs = MemoryFs::new()
            .with_file("/ws/package.json", r#"{"name":"r","workspaces":["packages/*"]}"#)
            .with_file("/ws/packages/web/package.json", r#"{"name":"@app/web"}"#)
            .with_file("/ws/packages/api/package.json", r#"{"name":"api"}"#);
        let config = Config::from_toml_str("").unwrap();
        let workspace = Workspace::discover_with_fs(Path::new("/ws"), &config, &fs).unwrap();
        let dirs = package_dirs(&workspace);
        let selected = |target: &str| {
            let target = WorkspaceTarget::new(target);
            let mut names: Vec<&str> = workspace
                .iter()
                .zip(&dirs)
                .filter(|((_, package), (_, dir))| target.names(package, dir))
                .filter_map(|((_, package), _)| package.name.as_deref())
                .collect();
            names.sort_unstable();
            names
        };
        assert_eq!(selected("api"), ["api"]);
        assert_eq!(selected("...api..."), ["api"]);
        assert_eq!(selected("@app/*"), ["@app/web"]);
        assert_eq!(selected("./packages/web/"), ["@app/web"]);
        assert_eq!(selected("packages/api"), ["api"]);
        assert!(selected("!api").is_empty());
        assert!(selected("@app/web/").is_empty());
    }
}
