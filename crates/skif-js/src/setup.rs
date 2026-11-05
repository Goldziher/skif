//! Per-package analysis settings: configured, default and plugin-provided globs merged once.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use globset::{Glob, GlobBuilder, GlobMatcher};
use indexmap::IndexMap;
use skif_config::{Config, IgnoreExportsUsedInFile, JsWorkspace, PackageJs};
use skif_core::FileSystem;
use skif_js_syntax::{Export, ExportKind};
use skif_workspace::{Package, PackageId, Workspace, map_ordered};

use crate::ancestors;
use crate::globs::{Globs, Names};
use crate::input::{BuildDir, InputRequest, InputSource, PathAlias, WorkspaceInputs};
use crate::listed::{self, Listed};
use crate::scripts::{self, ScriptBinary, ScriptFacts};
use crate::specifier::package_name;

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
    ignore_dependencies: Vec<String>,
    ignore_binaries: Vec<String>,
    ignore_unresolved: Vec<String>,
    ignore_members: Vec<String>,
    include_entry_exports: Option<bool>,
    ignore_exports_used_in_file: Option<IgnoreExportsUsedInFile>,
    paths: IndexMap<String, Vec<String>>,
}

fn normalize_dir(dir: &str) -> &str {
    let dir = dir.trim_start_matches("./").trim_end_matches('/');
    if dir.is_empty() { "." } else { dir }
}

impl Configured {
    /// Applies the `js` keys of an override: the plain keys replace what is there, the `extend-` keys append.
    fn fold(&mut self, js: &PackageJs) {
        let replace = |target: &mut Vec<String>, value: &Option<Vec<String>>| {
            if let Some(value) = value {
                target.clone_from(value);
            }
        };
        replace(&mut self.entry, &js.entry);
        replace(&mut self.project, &js.project);
        replace(&mut self.ignore, &js.ignore);
        replace(&mut self.ignore_dependencies, &js.ignore_dependencies);
        replace(&mut self.ignore_binaries, &js.ignore_binaries);
        replace(&mut self.ignore_unresolved, &js.ignore_unresolved);
        self.ignore_unresolved
            .extend(js.extend_ignore_unresolved.iter().cloned());
        self.ignore.extend(js.extend_ignore.iter().cloned());
        self.ignore_dependencies
            .extend(js.extend_ignore_dependencies.iter().cloned());
        self.ignore_binaries.extend(js.extend_ignore_binaries.iter().cloned());
    }
}

impl From<&JsWorkspace> for Configured {
    fn from(workspace: &JsWorkspace) -> Self {
        Self {
            entry: workspace.entry.clone(),
            project: workspace.project.clone(),
            ignore: workspace.ignore.clone(),
            ignore_dependencies: workspace.ignore_dependencies.clone(),
            ignore_binaries: workspace.ignore_binaries.clone(),
            ignore_unresolved: workspace.ignore_unresolved.clone(),
            ignore_members: workspace.ignore_members.clone(),
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
        Self { keys }
    }

    /// The table of `workspaces` that stands for `dir` (`"."` or empty for the root). `workspaces` is the map the
    /// keys were compiled from.
    pub(crate) fn table<'a>(
        &self,
        workspaces: &'a IndexMap<String, JsWorkspace>,
        dir: &str,
    ) -> Option<&'a JsWorkspace> {
        let dir = if dir.is_empty() { "." } else { dir };
        let position = self
            .keys
            .iter()
            .position(|key| key.dir == dir)
            .or_else(|| self.keys.iter().position(|key| key.selects(dir)))?;
        workspaces.get_index(position).map(|(_, workspace)| workspace)
    }

    /// The keys, as written in `workspaces`, that select none of `dirs`.
    fn unmatched<'a>(&self, workspaces: &'a IndexMap<String, JsWorkspace>, dirs: &[String]) -> Vec<&'a String> {
        workspaces
            .keys()
            .zip(&self.keys)
            .filter(|(_, key)| !dirs.iter().any(|dir| key.selects(dir)))
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
    pub entry: Globs,
    /// Entry globs that point outside the package directory (`../shared/setup.ts`), relative to the workspace root.
    pub external_entry: Globs,
    /// Entry globs from plugins and tool configuration. Unlike the defaults they also match files that belong to a
    /// workspace package nested below this one: a root `playwright.config.ts` names the tests of the packages.
    pub shared_entry: Globs,
    /// The tool configuration of the package could not be fully read, so its entry files are not all known.
    /// Files and dependencies cannot be judged unused then.
    pub entries_incomplete: bool,
    /// A tool configuration elsewhere that could not be evaluated imports from this package, so any of its files may
    /// be named by it: every project file counts as an entry.
    pub credit_all_files: bool,
    /// Modules imported by the package's unreadable tool configuration; see `WorkspaceInputs::config_imports`.
    pub config_imports: Vec<String>,
    pub project: Globs,
    /// Files whose issues are not reported, relative to the package directory.
    pub ignore: Globs,
    /// Dependencies and binaries plugins say are never reported.
    pub ignore_dependencies: Names,
    pub ignore_binaries: Names,
    /// The `ignore-dependencies`, `ignore-binaries`, `ignore-unresolved` and `ignore-members` of this workspace.
    pub user_ignore_dependencies: Listed,
    pub user_ignore_binaries: Listed,
    pub ignore_unresolved: Listed,
    pub ignore_members: Listed,
    /// Whether unused exports of entry files are reported (`include-entry-exports`, workspace over top level).
    pub include_entry_exports: bool,
    /// Which exports used in their own file are not reported.
    pub used_in_file: IgnoreExportsUsedInFile,
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
    /// `[js.workspaces]` keys that select no package.
    pub unmatched_workspace_keys: Vec<String>,
    pub production: bool,
    pub include_entry_exports: bool,
    /// `[js] ignore-exports-used-in-file`.
    pub used_in_file: IgnoreExportsUsedInFile,
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

/// Whether a workspace named by a script (`yarn workspace web`, `pnpm --filter "@scope/*"`, `npm -w packages/web`)
/// is `package`: by name, by name pattern or by directory. Exclusions (`!web`) select nothing.
fn names_package(target: &str, package: &skif_workspace::Package) -> bool {
    let target = target.trim_start_matches("...").trim_end_matches("...");
    if target.starts_with('!') {
        return false;
    }
    if package.name.as_deref() == Some(target) {
        return true;
    }
    if target.starts_with('.') || (target.contains('/') && !target.starts_with('@')) {
        let dir = package.dir.to_string_lossy().replace('\\', "/");
        if normalize_dir(target) == dir {
            return true;
        }
    }
    package.name.as_deref().is_some_and(|name| {
        target.contains(['*', '?', '[', '{'])
            && Glob::new(target).is_ok_and(|glob| glob.compile_matcher().is_match(name))
    })
}

/// Credits the binaries a package's scripts run in other, named workspaces to those workspaces, as binaries a listed
/// provider is used for and that are never unlisted: whether the name is a binary or a script of the target is
/// not known from the caller.
fn forward_workspace_binaries(packages: &mut [Option<PackageSetup>], workspace: &Workspace) {
    let mut credits: Vec<(PackageId, ScriptBinary)> = Vec::new();
    for setup in packages.iter().flatten() {
        for forwarded in &setup.scripts.forwarded {
            for (id, target) in workspace.iter() {
                let runs_script = target
                    .manifest
                    .value()
                    .get("scripts")
                    .and_then(|scripts| scripts.get(&forwarded.binary.name))
                    .is_some();
                if !runs_script && forwarded.targets.iter().any(|name| names_package(name, target)) {
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

/// The packages an unreadable tool configuration imports from: a shared config helper may name files and packages
/// that only it knows, so the package that defines it is incomplete and all its files count as entries.
fn credit_config_helpers(packages: &mut [Option<PackageSetup>], workspace: &Workspace) {
    let mut credited: Vec<PackageId> = Vec::new();
    for setup in packages.iter().flatten() {
        for module in &setup.config_imports {
            let target = if Path::new(module).is_absolute() {
                workspace
                    .iter()
                    .filter(|(_, package)| module_inside(module, &workspace.root.join(&package.dir)))
                    .max_by_key(|(_, package)| package.dir.components().count())
                    .map(|(id, _)| id)
            } else {
                let name = package_name(module);
                workspace
                    .iter()
                    .find(|(_, package)| name.is_some() && package.name == name)
                    .map(|(id, _)| id)
            };
            if let Some(id) = target
                && id != setup.id
            {
                credited.push(id);
            }
        }
    }
    for id in credited {
        if let Some(Some(setup)) = packages.get_mut(id.index()) {
            setup.entries_incomplete = true;
            setup.credit_all_files = true;
        }
    }
}

/// Whether the path `module` lies below `dir`.
fn module_inside(module: &str, dir: &Path) -> bool {
    Path::new(module).starts_with(dir)
}

/// Whether an entry glob from a plugin may match files of nested workspace packages. A glob that starts with `**/`
/// is a per-package default (every package has its own test files) and one that leaves the package is handled as
/// an external entry.
fn is_shared_pattern(pattern: &str) -> bool {
    let body = pattern.trim_start_matches('!').trim_start_matches("./");
    !body.starts_with("**/") && !body.starts_with("../") && !body.starts_with('/')
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
        sequential: bool,
    ) -> Self {
        let js = &config.js;
        let production = js.production;
        let ancestor_dependencies = ancestors::external_dependencies(fs.as_ref(), &workspace.root);
        let ignore_tags = config
            .rule("unused-exports")
            .and_then(|rule| skif_lint::parse_options::<UnusedExportsOptions>(&rule.options).ok())
            .map(|options| options.ignore_tags)
            .unwrap_or_default();
        let ignore_workspaces = Listed::directories(&js.ignore_workspaces, production);
        let workspace_keys = WorkspaceKeys::new(&js.workspaces);
        let js_ignored: HashSet<PackageId> = workspace
            .iter()
            .filter(|(_, package)| !package.is_root)
            .filter(|(_, package)| ignore_workspaces.is_match(&package.dir.to_string_lossy().replace('\\', "/")))
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
        let unmatched_workspace_keys: Vec<String> = workspace_keys
            .unmatched(&js.workspaces, &package_dirs)
            .into_iter()
            .cloned()
            .collect();
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
            let request = InputRequest {
                workspace,
                package,
                fs: fs.as_ref(),
                ancestor_dependencies: &ancestor_dependencies,
            };
            for source in sources {
                plugin.extend(source.inputs(&request));
            }
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
            if !production {
                entry.extend(plugin.entry);
            }
            entry.extend(plugin.production_entry);

            let mut project = listed::user_globs(&user.project, production, &default_project());
            if production {
                project.extend(listed::production_exclusions(&plugin.project));
                project.extend(listed::non_production_entries(&user.entry));
                project.extend(
                    test_files()
                        .into_iter()
                        .chain(test_directories())
                        .map(|glob| format!("!{glob}")),
                );
            } else {
                project.extend(plugin.project);
            }

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
                entry: Globs::new(entry),
                external_entry,
                shared_entry: Globs::new(shared_entry),
                entries_incomplete: plugin.entries_incomplete,
                credit_all_files: false,
                config_imports: plugin.config_imports,
                project: Globs::new(project),
                ignore: Globs::new(ignore),
                ignore_dependencies: Names::new(ignore_dependencies),
                ignore_binaries: Names::new(ignore_binaries),
                user_ignore_dependencies: Listed::new(&user.ignore_dependencies, production),
                user_ignore_binaries: Listed::new(&user.ignore_binaries, production),
                ignore_unresolved: Listed::new(&user.ignore_unresolved, production),
                ignore_members: Listed::new(&user.ignore_members, production),
                include_entry_exports: user.include_entry_exports.unwrap_or(js.include_entry_exports),
                used_in_file: user
                    .ignore_exports_used_in_file
                    .unwrap_or_else(|| js.ignore_exports_used_in_file.clone()),
                referenced_dependencies,
                referenced_binaries: plugin.binaries,
                scripts,
                config_files: plugin.config_files,
                aliases,
                build_dirs: plugin.build_dirs,
                conditions: plugin.conditions,
                module_dirs: plugin.module_dirs,
                extensions: plugin.extensions,
            })
        });
        forward_workspace_binaries(&mut packages, workspace);
        credit_config_helpers(&mut packages, workspace);
        Self {
            packages,
            global_ignore: Globs::new(&js.ignore),
            global_ignore_dependencies: Listed::new(&js.ignore_dependencies, production),
            global_ignore_binaries: Listed::new(&js.ignore_binaries, production),
            global_ignore_unresolved: Listed::new(&js.ignore_unresolved, production),
            global_ignore_members: Listed::new(&js.ignore_members, production),
            ignore_workspaces,
            js_ignored,
            unmatched_workspace_keys,
            production,
            include_entry_exports: js.include_entry_exports,
            used_in_file: js.ignore_exports_used_in_file.clone(),
            ignore_tags,
        }
    }

    pub(crate) fn package(&self, id: PackageId) -> Option<&PackageSetup> {
        self.packages.get(id.index()).and_then(Option::as_ref)
    }

    /// Marks the packages whose published entry points could not be mapped to source files.
    pub(crate) fn mark_entries_incomplete(&mut self, ids: &[PackageId]) {
        for id in ids {
            if let Some(Some(package)) = self.packages.get_mut(id.index()) {
                package.entries_incomplete = true;
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
}

#[cfg(test)]
mod tests {
    use super::{is_shared_pattern, workspace_relative};

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
}
