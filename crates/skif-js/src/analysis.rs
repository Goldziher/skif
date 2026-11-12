//! The analysis the rules read: module graph, export usage and dependency usage.

use std::cell::OnceCell;
use std::collections::BTreeSet;

use skif_core::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use indexmap::IndexMap;
use skif_config::Config;
use skif_core::{FileSystem, IncompletePackage, IncompleteScope, Span, timing};
use skif_js_syntax::{Export, ExportKind, Member, ParseOptions, parse_cached};
use skif_workspace::{IndexCache, IndexedFs, PackageId, TrackedFiles, Workspace, map_ordered};

use crate::config_hints::{ConfigHint, HintInputs, config_hints};
use crate::deps::{DependencyUsage, ForeignMapping};
use crate::discover::{Candidate, discover_with, relative, take_file_links};
use crate::entries::{Disk, entry_files};
use crate::fixture::in_fixture_directory;
use crate::globs::Globs;
use crate::graph::{self, Inputs, Module, ModuleGraph, Target};
use crate::input::InputSource;
use crate::resolve::{ModuleResolver, is_installed};
use crate::setup::{Setup, WorkspaceKeys};
use crate::shared_modules::ModuleParser;
use crate::specifier::package_name;
use crate::usage::ExportUsage;

/// A `JSDoc` tag that exempts an export or member that is used anyway, so the tag does nothing (knip's tag hint).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnneededTag<'m> {
    /// The export name, or `Export.member` for an enum or namespace member.
    pub identifier: String,
    /// Location of the tagged export or member.
    pub span: Span,
    /// The tag name, without `@`.
    pub tag: &'m str,
}

/// A configured `entry`, `project`, `ignore` or `ignore-files` pattern the analysis judged, and whether it is empty:
/// it matches no file the analysis knows (knip's `entry-empty`, `project-empty` and `ignore` hints) or, for
/// `ignore-files`, hides none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JudgedPattern {
    /// The configuration key, as written, such as `js.entry` or `js.workspaces."packages/*".project`.
    pub key: String,
    pub pattern: String,
    pub empty: bool,
}

/// A project file nothing reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnusedFile {
    /// Path relative to the workspace root.
    pub path: PathBuf,
    /// The package that owns the file.
    pub owner: PackageId,
}

/// Everything the JS rules need, built once per lint run.
pub struct JsAnalysis<'a> {
    /// The workspace analysed.
    pub workspace: &'a Workspace,
    /// Modules reachable from the entry files.
    pub graph: ModuleGraph,
    /// Which exports other modules use.
    pub exports: ExportUsage,
    /// Which packages and binaries the code and scripts use.
    pub dependencies: DependencyUsage,
    /// Project files outside the graph.
    pub unused_files: Vec<UnusedFile>,
    ambient_modules: HashSet<String>,
    /// The discovered source files outside the graph, relative to the workspace root with `/` separators, so
    /// `explain-usage` can tell a file the analysis never looked at from one it left out.
    pub(crate) outside_graph: HashSet<String>,
    pub(crate) setup: Setup,
    /// The ids of the rules that run in this pass; `None` when every rule is assumed to run.
    pub(crate) enabled_rules: Option<HashSet<String>>,
    /// The configuration file, relative to the workspace root, that configuration findings point at.
    pub(crate) config_path: Option<PathBuf>,
    /// The configured `entry`, `project` and `ignore` patterns, each empty when it matches no file.
    pub(crate) judged_patterns: Vec<JudgedPattern>,
    /// The `ignore-files` patterns that could be judged, each empty when it hides no file `unused-files` would report.
    pub(crate) judged_ignore_files: Vec<JudgedPattern>,
    /// The other configuration hints: redundant and top-level patterns, `project` extensions, missing package entries.
    pub(crate) config_hints: Vec<ConfigHint>,
    /// Files plugins name as entries that are no file of this analysis, absolute; see [`Self::other_project_files`].
    other_root_entries: Vec<PathBuf>,
    /// The packages that own a file another root of a multi-root run imports, which may import their `exports`
    /// subpaths: `unused-subpath-exports` does not judge them.
    pub(crate) imported_by_other_roots: HashSet<PackageId>,
    /// Whether tracked directories were listed from the git index (`[js] discovery = "git"`), which can miss an
    /// untracked file that uses a file reported as unused; `unused-files` then offers no fix that deletes it.
    pub(crate) listed_from_index: bool,
    /// The names of the members that nothing can import or run; see [`Self::only_orders_tasks`].
    task_only_members: HashSet<String>,
    /// The positions of the unused exports of each module, by module index, computed once for the rules that ask:
    /// see [`Self::unused_exports`].
    unused_export_memo: Vec<OnceLock<Vec<usize>>>,
}

/// The exports of one module by name, each name's exports in source order.
struct ExportsByName<'m>(HashMap<&'m str, Vec<&'m Export>>);

impl<'m> ExportsByName<'m> {
    fn of(exports: &'m [Export]) -> Self {
        let mut by_name: HashMap<&'m str, Vec<&'m Export>> = HashMap::default();
        for export in exports {
            by_name.entry(export.name.as_str()).or_default().push(export);
        }
        Self(by_name)
    }

    fn named(&self, name: &str) -> &[&'m Export] {
        self.0.get(name).map_or(&[], Vec::as_slice)
    }
}

/// The exports of a module by name, built on first use: most exports appear in no signature and never need it.
type LazyExportsByName<'m> = OnceCell<ExportsByName<'m>>;

/// The `package.json` fields that name a module or a binary of a package.
const MODULE_ENTRY_FIELDS: [&str; 7] = ["main", "module", "exports", "bin", "browser", "types", "typings"];

/// The names of the members whose manifest has none of the [`MODULE_ENTRY_FIELDS`] and that own none of
/// `candidates`: a tooling, asset or style package, which a dependency can only list to order tasks.
fn task_only_members(workspace: &Workspace, candidates: &[Candidate]) -> HashSet<String> {
    let with_sources: HashSet<PackageId> = candidates.iter().map(|candidate| candidate.owner).collect();
    workspace
        .packages()
        .iter()
        .filter(|package| !with_sources.contains(&package.id))
        .filter(|package| {
            let manifest = package.manifest.value();
            MODULE_ENTRY_FIELDS.iter().all(|field| manifest.get(field).is_none())
        })
        .filter_map(|package| package.name.clone())
        .collect()
}

/// What one run of the analysis shares with the other runs of its host.
#[derive(Clone, Copy)]
pub(crate) struct Caches<'c> {
    /// The parsed git indexes, so that the roots of one run share them.
    pub indexes: &'c IndexCache,
    /// How this run parses through the cache of parse results, when there is one.
    pub modules: Option<&'c ModuleParser<'c>>,
    /// Files, relative to the workspace root, that other roots of the run import as entries.
    pub extra_entries: &'c [PathBuf],
}

/// Names declared with `declare module 'name'` by graph modules and by declaration files nothing imports.
fn collect_ambient_modules(
    fs: &dyn FileSystem,
    graph: &ModuleGraph,
    candidates: &[Candidate],
    (sequential, modules): (bool, Option<&ModuleParser<'_>>),
) -> HashSet<String> {
    let mut names: HashSet<String> = graph
        .modules()
        .iter()
        .flat_map(|module| module.info.ambient_modules.iter().cloned())
        .collect();
    let declaration_files: Vec<&Path> = candidates
        .iter()
        .filter(|candidate| candidate.rel.ends_with(".d.ts") && graph.id_of(&candidate.abs).is_none())
        .map(|candidate| candidate.abs.as_path())
        .collect();
    let options = ParseOptions {
        in_file_references: false,
        member_accesses: false,
    };
    let found = map_ordered(&declaration_files, sequential, |path| {
        fs.read_to_string(path)
            .ok()
            .filter(|source| source.contains("declare module"))
            .and_then(|source| {
                modules
                    .map_or_else(
                        || parse_cached(None, path, &source, &options),
                        |modules| modules.parse(path, &source, options).0,
                    )
                    .ok()
            })
            .map(|info| info.ambient_modules)
            .unwrap_or_default()
    });
    names.extend(found.into_iter().flatten());
    names
}

/// Records on every unlisted import whose specifier the `paths` of another package's TypeScript configuration map to
/// a source file of the workspace, but not those of the configuration the importing file follows, which config and
/// pattern map it. The patterns of all packages are indexed once, and only when an import is unlisted.
///
/// An import keeps the `package.json` advice when that is the fix: it resolved to a module of the graph, names a
/// workspace package or one a manifest lists, or names an installed package.
fn mark_foreign_paths(resolver: &ModuleResolver, graph: &ModuleGraph, dependencies: &mut DependencyUsage) {
    if dependencies.unlisted.is_empty() {
        return;
    }
    let foreign = resolver.package_paths();
    let marks: Vec<(usize, ForeignMapping)> = dependencies
        .unlisted
        .iter()
        .enumerate()
        .filter_map(|(index, unlisted)| {
            let importer = &graph.module(unlisted.module).abs;
            let unresolved = matches!(unlisted.target, None | Some(Target::External(_) | Target::Unresolved));
            if !unresolved
                || dependencies.known_package(&unlisted.name)
                || resolver.installed(importer, &unlisted.name)
                || resolver.ts_paths_map(importer, &unlisted.specifier)
            {
                return None;
            }
            let (config, pattern) = foreign.lookup(&unlisted.specifier, |config, pattern| {
                resolver.maps_to_source(config, pattern, &unlisted.specifier)
            })?;
            Some((
                index,
                ForeignMapping {
                    config: config.to_owned(),
                    pattern: pattern.to_owned(),
                },
            ))
        })
        .collect();
    for (index, mark) in marks {
        dependencies.unlisted[index].foreign_paths = Some(mark);
    }
}

/// Directories [`on_disk`] does not enter unless the pattern names them: installed packages and git internals are
/// never what a project pattern means, and walking them would make the check as slow as a dependency install.
const UNWALKED_DIRS: [&str; 2] = ["node_modules", ".git"];

/// Whether a file on disk below `base`, gitignored or not and of any extension, matches `pattern` (relative to
/// `base`).
///
/// It settles a pattern that matched none of the files the analysis knows before calling it empty: those are source
/// files, so `ignore = ["dist/**"]` (gitignored), `ignore = ["**/*.json"]` and `entry = ["index.html"]` would
/// otherwise be reported although they match files. Only such patterns reach it, so the walk is rare; it starts at
/// the literal directory prefix of the pattern, does not follow symbolic links to directories and skips
/// [`UNWALKED_DIRS`] unless the pattern names them.
pub(crate) fn on_disk(fs: &dyn FileSystem, base: &Path, pattern: &str) -> bool {
    let body = pattern.trim().trim_start_matches("./").trim_end_matches('!');
    let globs = Globs::new([body]);
    if globs.is_empty() {
        return false;
    }
    let is_glob = |part: &str| part.contains(['*', '?', '[', '{']);
    let parts: Vec<&str> = body.split('/').collect();
    if !parts.iter().any(|part| is_glob(part)) {
        return fs.is_file(&base.join(body));
    }
    let prefix: PathBuf = parts.iter().take_while(|part| !is_glob(part)).collect();
    let mut stack = vec![base.join(prefix)];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs.read_dir_shared(&dir) else {
            continue;
        };
        for entry in entries.iter() {
            let Some(rel) = relative(&entry.path, base) else {
                continue;
            };
            if entry.is_file && globs.matches_any(&rel) {
                return true;
            }
            let unwalked = entry
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| UNWALKED_DIRS.contains(&name) && !parts.contains(&name));
            if entry.is_dir && !entry.is_symlink && !unwalked {
                stack.push(entry.path.clone());
            }
        }
    }
    false
}

/// The configuration key a package's pattern of the list `name` is written under: the `[js.workspaces]` key as
/// written (so a glob key such as `packages/*` is reported once, as the user wrote it), or the package directory for
/// a pattern that comes from elsewhere (an override), both as [`WorkspaceKeys::key_dir`] spells them.
pub(crate) fn workspace_key(
    config: &Config,
    keys: &WorkspaceKeys,
    package: &skif_workspace::Package,
    name: &str,
) -> String {
    let dir = keys.key_dir(package);
    let written = keys
        .package_key(&config.js.workspaces, package)
        .map_or(dir.as_str(), String::as_str);
    format!("js.workspaces.\"{written}\".{name}")
}

/// Patterns judged per package, keyed by where they are written: a pattern is empty only when it matches in none of
/// the packages its key selects.
#[derive(Default)]
struct Judged {
    matched: IndexMap<(String, String), bool>,
}

impl Judged {
    /// Records `pattern` under `key`, calling `matches` only while no package has matched it yet.
    fn judge(&mut self, key: String, pattern: &str, matches: impl FnOnce() -> bool) {
        let slot = self.matched.entry((key, pattern.to_owned())).or_insert(false);
        if !*slot {
            *slot = matches();
        }
    }

    fn verdicts(self) -> Vec<JudgedPattern> {
        self.matched
            .into_iter()
            .map(|((key, pattern), matched)| JudgedPattern {
                key,
                pattern,
                empty: !matched,
            })
            .collect()
    }
}

/// The configured `entry`, `project` and `ignore` patterns, each empty when it matches no file: none of the files the
/// analysis knows (the discovered source files and the modules of the graph) and no file on disk ([`on_disk`]).
/// Negated patterns, patterns that leave the package (`../`) and the defaults are not judged. A known file only
/// reaches a package's patterns when the package owns it, as for entries; a pattern of a glob `[js.workspaces]` key is
/// empty when it matches in none of the packages the key selects.
fn judged_patterns(
    workspace: &Workspace,
    config: &Config,
    setup: &Setup,
    candidates: &[Candidate],
    graph: &ModuleGraph,
    disk: &dyn FileSystem,
) -> Vec<JudgedPattern> {
    // The known files are listed, and grouped by owner, only when a pattern is judged against them.
    let module_rels: OnceCell<Vec<String>> = OnceCell::new();
    let owners: OnceCell<HashMap<PackageId, Vec<&Path>>> = OnceCell::new();
    let owners = || {
        owners.get_or_init(|| {
            group_by_owner(
                candidates
                    .iter()
                    .map(|candidate| (candidate.owner, candidate.abs.as_path()))
                    .chain(
                        graph
                            .modules()
                            .iter()
                            .map(|module| (module.owner, module.abs.as_path())),
                    ),
            )
        })
    };
    let judged = |pattern: &&String| {
        let body = pattern.trim();
        !body.is_empty() && !body.starts_with('!') && !body.starts_with("../")
    };
    let mut found = Judged::default();
    for pattern in config.js.ignore.iter().filter(judged) {
        found.judge("js.ignore".to_owned(), pattern, || {
            let globs = Globs::new([pattern]);
            let module_rels = module_rels.get_or_init(|| {
                graph
                    .modules()
                    .iter()
                    .map(|module| module.path.to_string_lossy().replace('\\', "/"))
                    .collect()
            });
            candidates.iter().any(|candidate| globs.matches_any(&candidate.rel))
                || module_rels.iter().any(|rel| globs.matches_any(rel))
                || on_disk(disk, &workspace.root, pattern)
        });
    }
    let keys = &setup.workspace_keys;
    for (id, package) in workspace.iter() {
        let Some(package_setup) = setup.package(id) else {
            continue;
        };
        let owned: OnceCell<Vec<String>> = OnceCell::new();
        let owned = || owned.get_or_init(|| relative_to(owners().get(&id).map(Vec::as_slice), &package_setup.dir));
        let user = &package_setup.user_patterns;
        for (name, patterns, top_level) in [
            ("entry", &user.entry, &config.js.entry),
            ("project", &user.project, &config.js.project),
            ("ignore", &user.ignore, &config.js.ignore),
        ] {
            for pattern in patterns.iter().filter(judged) {
                let from_top_level = package.is_root && top_level.contains(pattern);
                if from_top_level && name == "ignore" {
                    continue;
                }
                let key = if from_top_level {
                    format!("js.{name}")
                } else {
                    workspace_key(config, keys, package, name)
                };
                found.judge(key, pattern, || {
                    let globs = Globs::new([pattern]);
                    owned().iter().any(|rel| globs.matches_any(rel)) || on_disk(disk, &package_setup.dir, pattern)
                });
            }
        }
    }
    found.verdicts()
}

/// The paths of `files` grouped by the package that owns them, each group in the order of `files`.
fn group_by_owner<'f>(files: impl IntoIterator<Item = (PackageId, &'f Path)>) -> HashMap<PackageId, Vec<&'f Path>> {
    let mut groups: HashMap<PackageId, Vec<&'f Path>> = HashMap::default();
    for (owner, path) in files {
        groups.entry(owner).or_default().push(path);
    }
    groups
}

/// The paths of `group` below `dir`, relative to it and `/`-separated, in order; the others are left out.
fn relative_to(group: Option<&[&Path]>, dir: &Path) -> Vec<String> {
    group
        .unwrap_or_default()
        .iter()
        .filter_map(|path| relative(path, dir))
        .collect()
}

/// The `ignore-files` patterns, each empty when it hides none of the files that would be reported unused (knip's
/// `ignoreFiles` hint) and matches no file on disk either ([`on_disk`]), so that a pattern for generated or
/// non-source files is not reported. A pattern of a glob `[js.workspaces]` key is judged across the packages the key
/// selects; one of a package whose entry files are not all known is not judged.
fn judged_ignore_files(
    workspace: &Workspace,
    config: &Config,
    setup: &Setup,
    unreached: &[&Candidate],
    disk: &dyn FileSystem,
) -> Vec<JudgedPattern> {
    let mut found = Judged::default();
    let all_judged = workspace
        .iter()
        .all(|(id, _)| setup.package(id).is_none_or(|package| !package.entries_incomplete));
    let ignore_files = config.js.ignore_files.iter().filter(|_| all_judged);
    for pattern in ignore_files.map(|item| item.name.as_str()) {
        let globs = Globs::new([pattern]);
        if globs.is_empty() {
            continue;
        }
        found.judge("js.ignore-files".to_owned(), pattern, || {
            unreached.iter().any(|candidate| globs.matches_any(&candidate.rel))
                || on_disk(disk, &workspace.root, pattern)
        });
    }
    let keys = &setup.workspace_keys;
    let mut incomplete = HashSet::default();
    let owners: OnceCell<HashMap<PackageId, Vec<&Path>>> = OnceCell::new();
    let owners = || {
        owners.get_or_init(|| {
            group_by_owner(
                unreached
                    .iter()
                    .map(|candidate| (candidate.owner, candidate.abs.as_path())),
            )
        })
    };
    for (id, package) in workspace.iter() {
        let Some(package_setup) = setup.package(id) else {
            continue;
        };
        let owned: OnceCell<Vec<String>> = OnceCell::new();
        let owned = || owned.get_or_init(|| relative_to(owners().get(&id).map(Vec::as_slice), &package_setup.dir));
        for pattern in &package_setup.user_patterns.ignore_files {
            let globs = Globs::new([pattern]);
            if globs.is_empty() {
                continue;
            }
            let key = workspace_key(config, keys, package, "ignore-files");
            if package_setup.entries_incomplete {
                // Its unreached files are not known, so the pattern may hide one: it is not judged anywhere.
                incomplete.insert((key, pattern.clone()));
                continue;
            }
            found.judge(key, pattern, || {
                owned().iter().any(|rel| globs.matches_any(rel)) || on_disk(disk, &package_setup.dir, pattern)
            });
        }
    }
    found.matched.retain(|judged, _| !incomplete.contains(judged));
    found.verdicts()
}

/// `fs` with the files tracked by git listed from the git index when the configuration selects it (see
/// [`TrackedFiles::for_discovery`]), every directory modified since the index was written listed from disk. `None`
/// when the directories are walked.
fn overlay(
    fs: &Arc<dyn FileSystem>,
    workspace: &Workspace,
    config: &Config,
    cache: &IndexCache,
) -> Option<Arc<dyn FileSystem>> {
    let tracked = TrackedFiles::for_discovery(fs.as_ref(), &workspace.root, config, cache)?;
    Some(Arc::new(IndexedFs::new(Arc::clone(fs), Arc::new(tracked))))
}

/// The file system of `workspace` for the discovery `config` selects: the git index overlay when it applies, `fs`
/// itself otherwise.
///
/// This is the file system the analysis lists and reads through. It is exposed for tools that want to measure or
/// reuse the listing.
#[must_use]
pub fn discovery_file_system(fs: &Arc<dyn FileSystem>, workspace: &Workspace, config: &Config) -> Arc<dyn FileSystem> {
    overlay(fs, workspace, config, &IndexCache::default()).unwrap_or_else(|| Arc::clone(fs))
}

impl<'a> JsAnalysis<'a> {
    /// Discovers files, builds the graph and computes usage.
    ///
    /// `sources` supply plugin-provided entries, project files, ignores and references. With `sequential`, parsing
    /// runs on the calling thread and rayon is not used; otherwise it runs on the current rayon pool.
    #[must_use]
    pub fn build(
        workspace: &'a Workspace,
        config: &Config,
        fs: &Arc<dyn FileSystem>,
        sources: &[Arc<dyn InputSource>],
        sequential: bool,
    ) -> Self {
        Self::build_cached(
            workspace,
            config,
            fs,
            sources,
            sequential,
            &Caches {
                indexes: &IndexCache::default(),
                modules: None,
                extra_entries: &[],
            },
        )
    }

    /// Like [`build`](Self::build), reading the git index and parse results through `caches`, with the extra entries
    /// other roots of the run import.
    pub(crate) fn build_cached(
        workspace: &'a Workspace,
        config: &Config,
        fs: &Arc<dyn FileSystem>,
        sources: &[Arc<dyn InputSource>],
        sequential: bool,
        caches: &Caches<'_>,
    ) -> Self {
        let Caches {
            indexes: cache,
            modules,
            extra_entries,
        } = *caches;
        let phase = |name: &str| format!("js {name} {}", workspace.root.display());
        let indexed = timing::phase(phase("git index"), || overlay(fs, workspace, config, cache));
        let listed_from_index = indexed.is_some();
        // The configuration hints look past the index, at gitignored files too.
        let disk = Arc::clone(fs);
        let fs = indexed.as_ref().unwrap_or(fs);
        let mut setup = timing::phase(phase("setup (plugins, configs)"), || {
            Setup::new(workspace, config, fs, sources, sequential)
        });
        let mut candidates = timing::phase(phase("discover"), || {
            discover_with(
                fs.as_ref(),
                workspace,
                sequential,
                !config.js.no_gitignore,
                &setup.compilers,
            )
        });
        let task_only_members = task_only_members(workspace, &candidates);
        if !setup.js_ignored.is_empty() {
            candidates.retain(|candidate| !setup.js_ignored.contains(&candidate.owner));
        }
        let links = take_file_links(fs.as_ref(), &workspace.root, &mut candidates);
        let found = timing::phase(phase("entries"), || {
            let published = Disk {
                fs: disk.as_ref(),
                gitignore: !config.js.no_gitignore,
            };
            entry_files(
                fs.as_ref(),
                workspace,
                &setup,
                &candidates,
                (extra_entries, sequential),
                published,
            )
        });
        setup.mark_entries_incomplete(&found.unknown);
        let entries = found.files;
        let other_root_entries = found.other_roots;
        let config_hints = {
            let entry_set: HashSet<PathBuf> = entries.iter().cloned().collect();
            config_hints(&HintInputs {
                workspace,
                config,
                setup: &setup,
                candidates: &candidates,
                entries: &entry_set,
                provided: &found.provided,
                disk: disk.as_ref(),
            })
        };
        let resolver = timing::phase(phase("resolver"), || {
            ModuleResolver::new(Arc::clone(fs), workspace, &setup)
        });
        let inputs = Inputs {
            fs: fs.as_ref(),
            resolver: &resolver,
            root: &workspace.root,
            candidates: &candidates,
            links: &links,
            setup: &setup,
            sequential,
            modules,
        };
        let graph = timing::phase(phase("graph (read, parse, resolve)"), || {
            graph::build(&inputs, &entries)
        });
        graph::report_timings(&workspace.root);
        setup.widen_incomplete_scopes(&graph);
        let exports = timing::phase(phase("exports"), || {
            ExportUsage::compute_by(&graph, |owner| setup.includes_entry_exports(owner))
        });
        let mut dependencies = timing::phase(phase("dependencies"), || {
            DependencyUsage::compute(workspace, &setup, &graph, fs.as_ref(), sequential)
        });
        mark_foreign_paths(&resolver, &graph, &mut dependencies);

        let ambient_modules = collect_ambient_modules(fs.as_ref(), &graph, &candidates, (sequential, modules));
        let outside_graph: HashSet<String> = candidates
            .iter()
            .filter(|candidate| graph.id_of(&candidate.abs).is_none())
            .map(|candidate| candidate.rel.clone())
            .collect();
        let unreached: Vec<&Candidate> = candidates
            .iter()
            .filter(|candidate| graph.id_of(&candidate.abs).is_none())
            .filter(|candidate| !candidate.rel.ends_with(".d.ts"))
            .filter(|candidate| {
                let Some(package) = setup.package(candidate.owner) else {
                    return false;
                };
                let Some(in_package) = relative(&candidate.abs, &package.dir) else {
                    return false;
                };
                package.judges(&in_package)
                    && package.project.is_match(&in_package)
                    && !package.ignore.matches_any(&in_package)
                    && !setup.global_ignore.matches_any(&candidate.rel)
                    && (setup.include_fixtures || !in_fixture_directory(Path::new(&candidate.rel)))
            })
            // The index may still list a file deleted since it was written; one `stat` per unused file settles it.
            .filter(|candidate| !listed_from_index || fs.kind(&candidate.abs).is_file)
            .collect();
        let ignores_file = |candidate: &Candidate| {
            setup.global_ignore_files.matches_any(&candidate.rel)
                || setup.package(candidate.owner).is_some_and(|package| {
                    relative(&candidate.abs, &package.dir).is_some_and(|rel| package.ignore_files.matches_any(&rel))
                })
        };
        let judged_patterns = judged_patterns(workspace, config, &setup, &candidates, &graph, disk.as_ref());
        let judged_ignore_files = judged_ignore_files(workspace, config, &setup, &unreached, disk.as_ref());
        let unused_files = unreached
            .iter()
            .filter(|candidate| !ignores_file(candidate))
            .map(|candidate| UnusedFile {
                path: PathBuf::from(&candidate.rel),
                owner: candidate.owner,
            })
            .collect();
        let unused_export_memo = graph.modules().iter().map(|_| OnceLock::new()).collect();
        let imported_by_other_roots = extra_entries
            .iter()
            .filter_map(|entry| graph.id_of(&workspace.root.join(entry)))
            .map(|id| graph.module(id).owner)
            .collect();
        Self {
            workspace,
            graph,
            exports,
            dependencies,
            unused_files,
            ambient_modules,
            outside_graph,
            setup,
            enabled_rules: None,
            config_path: None,
            judged_patterns,
            judged_ignore_files,
            config_hints,
            other_root_entries,
            imported_by_other_roots,
            listed_from_index,
            task_only_members,
            unused_export_memo,
        }
    }

    /// Whether `name` is a workspace member that nothing can import or run: its `package.json` names no `main`,
    /// `module`, `exports`, `bin`, `browser`, `types` or `typings`, and it has no source file. A dependency on such a
    /// member (a tooling, asset or style package) only orders tasks, as Turborepo's `^build` and Nx's task
    /// dependencies do, so it is never reported unused.
    #[must_use]
    pub(crate) fn only_orders_tasks(&self, name: &str) -> bool {
        self.task_only_members.contains(name)
    }

    /// Whether the project declares the module that `specifier` imports with `declare module`, so importing it needs
    /// no dependency: the specifier itself, the package it names (`declare module 'pkg'` also covers `pkg/sub`), or a
    /// pattern with one `*` and a literal start that matches it (`declare module 'pkg/*'`). A declared subpath
    /// (`declare module 'pkg/data.json'`) declares that module only, not the rest of the package.
    #[must_use]
    pub fn declares_module(&self, specifier: &str) -> bool {
        if self.ambient_modules.contains(specifier) {
            return true;
        }
        if package_name(specifier).is_some_and(|package| self.ambient_modules.contains(&package)) {
            return true;
        }
        self.ambient_modules.iter().any(|declared| {
            declared.split_once('*').is_some_and(|(prefix, suffix)| {
                !prefix.is_empty()
                    && !suffix.contains('*')
                    && specifier.len() >= prefix.len() + suffix.len()
                    && specifier.starts_with(prefix)
                    && specifier.ends_with(suffix)
            })
        })
    }

    /// Whether the project declares the module an import of package `name` written as `specifier` reaches, as
    /// [`Self::declares_module`] judges it. A specifier an alias leads into another package (`react` to
    /// `preact/compat`) does not tell which module of `name` it reaches, so a declaration of `name` or of any module
    /// inside it (`declare module 'preact/compat'`) covers it.
    #[must_use]
    pub fn declares_import(&self, specifier: &str, name: &str) -> bool {
        if self.declares_module(specifier) {
            return true;
        }
        package_name(specifier).as_deref() != Some(name)
            && self
                .ambient_modules
                .iter()
                .any(|declared| package_name(declared).as_deref() == Some(name))
    }

    /// Whether `[js] production` is on.
    #[must_use]
    pub fn production(&self) -> bool {
        self.setup.production
    }

    /// Whether knip's strict mode (`skif --strict`) is on: each package uses only its own manifest, required peer
    /// dependencies count as production dependencies and type-only imports use no package.
    #[must_use]
    pub fn strict(&self) -> bool {
        self.setup.isolate_workspaces
    }

    /// Whether dependency `name` of package `id` is on an `ignore-dependencies` list.
    #[must_use]
    pub fn ignores_dependency(&self, id: PackageId, name: &str) -> bool {
        self.setup.ignores_dependency(id, name)
    }

    /// Whether the import specifier `specifier` of package `id` is on an `ignore-unresolved` list, or on an
    /// `ignore-dependencies` list (kept so specifiers a plugin declares virtual stay silent).
    #[must_use]
    pub fn ignores_unresolved(&self, id: PackageId, specifier: &str) -> bool {
        let listed = self.setup.ignores_unresolved(id, specifier);
        self.setup.ignores_dependency(id, specifier) || listed
    }

    /// Whether the enum or namespace member `name` of package `id` is on an `ignore-members` list.
    #[must_use]
    pub fn ignores_member(&self, id: PackageId, name: &str) -> bool {
        self.setup.ignores_member(id, name)
    }

    /// Whether the entry files of package `id` are not fully known, so its dependencies are not judged. Its files and
    /// exports that may be unknown entries are not judged either: all of them, or those of its narrowed scope
    /// ([`Self::judges_exports_of`]).
    #[must_use]
    pub fn entries_incomplete(&self, id: PackageId) -> bool {
        self.setup.package(id).is_some_and(|package| package.entries_incomplete)
    }

    /// The files of other projects that this workspace uses, absolute and sorted.
    ///
    /// Below the root are the files of projects of their own ([`Module::foreign`], such as a nested root) that a
    /// module of this workspace imports, that a script runs, or that a plugin names as an entry (a workflow step's
    /// `node build/report.ts`). Only the files its own code reaches directly: what those import is for the owning
    /// project to follow, so that a multi-root run makes these, and only these, its entries.
    ///
    /// Outside the root are every module the graph reached there (`../tools/detect.ts` and what it imports in
    /// turn), as the graph follows them, and the files a plugin names there; installed code is left out. A multi-root
    /// run counts them as used by the root that owns them, which was linted without this code.
    #[must_use]
    pub fn other_project_files(&self) -> Vec<PathBuf> {
        let root = &self.workspace.root;
        let mut files = BTreeSet::new();
        for module in self.graph.modules() {
            if !module.abs.starts_with(root) {
                if !is_installed(&module.abs) {
                    files.insert(module.abs.clone());
                }
                continue;
            }
            if module.foreign {
                if module.is_entry {
                    files.insert(module.abs.clone());
                }
                continue;
            }
            for import in &module.imports {
                let targets = match &import.target {
                    Target::Module(id) => std::slice::from_ref(id),
                    Target::Modules(ids) => ids.as_slice(),
                    _ => &[],
                };
                for target in targets.iter().map(|id| self.graph.module(*id)) {
                    if target.foreign && target.abs.starts_with(root) {
                        files.insert(target.abs.clone());
                    }
                }
            }
        }
        files.extend(self.other_root_entries.iter().cloned());
        files.into_iter().collect()
    }

    /// The analysed packages whose entry files are not all known, with every reason and the scope left unjudged
    /// (the whole package, or the globs the unknown entries can match); reason files are relative to the workspace
    /// root with `/` separators. In workspace order.
    #[must_use]
    pub fn incomplete_packages(&self) -> Vec<IncompletePackage> {
        let root = &self.workspace.root;
        self.workspace
            .iter()
            .filter_map(|(id, package)| {
                let setup = self.setup.package(id).filter(|setup| setup.entries_incomplete)?;
                let reasons = setup
                    .incomplete_reasons
                    .iter()
                    .cloned()
                    .map(|mut reason| {
                        reason.file = reason
                            .file
                            .map(|path| relative(&path, root).map_or(path, PathBuf::from));
                        reason
                    })
                    .collect();
                let scope = match &setup.incomplete_scope {
                    Some(scope) if !setup.unjudged_package_wide() => IncompleteScope::Within(scope.patterns.clone()),
                    _ => IncompleteScope::Package,
                };
                Some(
                    IncompletePackage::new(package.display_name().to_owned(), package.dir.clone(), reasons)
                        .with_scope(scope),
                )
            })
            .collect()
    }

    /// Whether the package is excluded from analysis (`ignore-packages`).
    #[must_use]
    pub fn is_skipped(&self, id: PackageId) -> bool {
        self.setup.package(id).is_none()
    }

    /// Whether a module's own issues are reported: it is a project file, not ignored, not a public entry, not in a
    /// fixture directory unless `[js] include-fixtures` is set, and no unknown entry of its package may use its
    /// exports ([`Self::judges_exports_of`]).
    #[must_use]
    pub fn reports_exports_of(&self, module: &Module) -> bool {
        module.in_project
            && !module.ignored
            && (!module.is_entry || self.setup.includes_entry_exports(module.owner))
            && self.judges_exports_of(module)
            && (self.setup.include_fixtures || !in_fixture_directory(&module.path))
    }

    /// Whether the exports of `module` can be judged although the entry files of its package may be incomplete
    /// (ADR 0081). In a package whose entries are known, always. In a package unjudged as a whole, never: a file no
    /// known entry reaches may be an entry and import the module. In a package whose incomplete entries lie in a
    /// narrowed scope, every file the scope matches is an entry of the graph already (outside the package's `project`
    /// globs and in the packages nested below it too), so each import an unknown entry can make is counted: only a
    /// module in the scope, which may itself be such an entry, is not judged.
    #[must_use]
    pub fn judges_exports_of(&self, module: &Module) -> bool {
        let Some(package) = self.setup.package(module.owner) else {
            return true;
        };
        relative(&module.abs, &package.dir).map_or(!package.entries_incomplete, |rel| package.judges(&rel))
    }

    /// Exports of `module` that no other module uses, values and types alike, in source order.
    ///
    /// Exports tagged `@public`, `@beta`, `@alpha` or `@alias` (and `@internal` in production mode, and the
    /// `ignore-tags` of `unused-exports`) are not listed, unless `include-tags` names the tag.
    ///
    /// Several rules ask for the same module, so the answer for a module of [`Self::graph`] is computed once.
    #[must_use]
    pub fn unused_exports<'m>(&self, module: &'m Module) -> Vec<&'m Export> {
        let exports = &module.info.exports;
        let memo = self.unused_export_memo.get(module.id.index()).filter(|_| {
            self.graph
                .modules()
                .get(module.id.index())
                .is_some_and(|own| std::ptr::eq(own, module))
        });
        let computed;
        let positions = if let Some(memo) = memo {
            skif_core::get_or_compute(memo, || self.unused_export_positions(module))
        } else {
            computed = self.unused_export_positions(module);
            &computed
        };
        positions.iter().map(|&position| &exports[position]).collect()
    }

    /// The positions in `module.info.exports` of [`Self::unused_exports`].
    fn unused_export_positions(&self, module: &Module) -> Vec<usize> {
        if !self.reports_exports_of(module) {
            return Vec::new();
        }
        let by_name = LazyExportsByName::new();
        module
            .info
            .exports
            .iter()
            .enumerate()
            .filter(|(_, export)| !(export.kind == ExportKind::StarReExport && export.name == "*"))
            .filter(|(_, export)| export.name != "export=")
            .filter(|(_, export)| !self.is_tagged_public(export))
            .filter(|(_, export)| !(export.referenced_in_file && self.setup.ignores_in_file_use(module.owner, export)))
            .filter(|(_, export)| !self.exports.is_used(module.id, &export.name))
            .filter(|(_, export)| !self.setup.is_registered(module.owner, export))
            .filter(|&(_, export)| !self.signature_use(module, &by_name, export, &mut HashSet::default()))
            .map(|(position, _)| position)
            .collect()
    }

    /// Whether `export` appears in the signature of another export of its module that is itself used.
    pub(crate) fn in_used_signature(&self, module: &Module, export: &Export) -> bool {
        self.signature_use(module, &LazyExportsByName::new(), export, &mut HashSet::default())
    }

    /// [`Self::in_used_signature`] over the exports of `module` by name, skipping the names in `visited`.
    fn signature_use<'m>(
        &self,
        module: &'m Module,
        by_name: &LazyExportsByName<'m>,
        export: &'m Export,
        visited: &mut HashSet<&'m str>,
    ) -> bool {
        export.referenced_in.iter().any(|name| {
            let containers = by_name
                .get_or_init(|| ExportsByName::of(&module.info.exports))
                .named(name);
            let registered = containers
                .iter()
                .any(|container| self.setup.is_registered(module.owner, container));
            if registered || self.exports.is_used(module.id, name) {
                return true;
            }
            visited.insert(name.as_str())
                && containers
                    .iter()
                    .any(|&container| self.signature_use(module, by_name, container, visited))
        })
    }

    pub(crate) fn is_tagged_public(&self, export: &Export) -> bool {
        self.exempts_tags(&export.tags)
    }

    /// Whether `JSDoc` tags `tags` (of an export, or of an enum or namespace member) keep it out of the report: an
    /// always-exempting tag, an excluding tag ([`Self::excluding_tags`]), or, with `include-tags`, none of those.
    pub(crate) fn exempts_tags(&self, tags: &[String]) -> bool {
        self.always_exempt(tags) || self.excluding_tags(tags).next().is_some() || self.lacks_included_tag(tags)
    }

    /// Whether `tags` carries a tag that always exempts (`public`, `beta`, `alpha` or `alias`) and `include-tags` does
    /// not name it. Naming such a tag in `include-tags` asks for exactly the exports that carry it, so it is judged
    /// instead of silencing the report (knip keeps exempting it, which reports nothing).
    fn always_exempt(&self, tags: &[String]) -> bool {
        tags.iter()
            .any(|tag| matches!(tag.as_str(), "public" | "beta" | "alpha" | "alias") && !self.includes_tag(tag))
    }

    /// The tags of `tags` that exempt by configuration: `internal` in production mode (unless `include-tags` names
    /// it) and the `ignore-tags` names.
    fn excluding_tags<'t>(&self, tags: &'t [String]) -> impl Iterator<Item = &'t str> {
        tags.iter().map(String::as_str).filter(|tag| {
            (*tag == "internal" && self.setup.production && !self.includes_tag(tag))
                || self.setup.ignore_tags.iter().any(|ignored| ignored == tag)
        })
    }

    /// Whether `include-tags` names `tag`.
    fn includes_tag(&self, tag: &str) -> bool {
        self.setup.include_tags.iter().any(|included| included == tag)
    }

    /// Whether `include-tags` is set and `tags` carries none of its names.
    fn lacks_included_tag(&self, tags: &[String]) -> bool {
        !self.setup.include_tags.is_empty() && !tags.iter().any(|tag| self.includes_tag(tag))
    }

    /// The members of an enum or namespace export.
    pub(crate) fn members_of(export: &Export) -> &[Member] {
        match export.kind {
            ExportKind::Enum => &export.enum_members,
            ExportKind::Namespace => &export.namespace_members,
            _ => &[],
        }
    }

    /// Whether `member` of the used export `export` of `module` is used, or listed in `ignore-members`.
    pub(crate) fn member_in_use(&self, module: &Module, export: &Export, member: &Member) -> bool {
        self.ignores_member(module.owner, &member.name)
            || export.members_read_in_file.contains(&member.name)
            || self.exports.is_member_used(module.id, &export.name, &member.name)
    }

    /// Whether `export` of `module` is used: imported, in the signature of a used export, or referenced in its own
    /// file when `ignore-exports-used-in-file` covers it.
    fn export_in_use<'m>(&self, module: &'m Module, by_name: &LazyExportsByName<'m>, export: &'m Export) -> bool {
        (export.referenced_in_file && self.setup.ignores_in_file_use(module.owner, export))
            || self.exports.is_used(module.id, &export.name)
            || self.signature_use(module, by_name, export, &mut HashSet::default())
    }

    /// Tags of the exports and members of `module` that exempt something used anyway, in source order.
    ///
    /// Only tags that exempt by configuration count (`internal` in production mode and `ignore-tags` names), as in
    /// knip: `@public`, `@beta`, `@alpha` and `@alias` document an API and are never unneeded. A tag on an enum or
    /// namespace export is needed while it hides an unused member.
    #[must_use]
    pub fn unneeded_tags<'m>(&self, module: &'m Module) -> Vec<UnneededTag<'m>> {
        let mut found = Vec::new();
        if !self.reports_exports_of(module) {
            return found;
        }
        let by_name = LazyExportsByName::new();
        for export in &module.info.exports {
            if self.always_exempt(&export.tags) || !self.export_in_use(module, &by_name, export) {
                continue;
            }
            let mut hides_member = false;
            if self.exports.is_used(module.id, &export.name) {
                for member in Self::members_of(export) {
                    if self.always_exempt(&member.tags) {
                        continue;
                    }
                    let mut excluding = self.excluding_tags(&member.tags).peekable();
                    if self.member_in_use(module, export, member) {
                        found.extend(excluding.map(|tag| UnneededTag {
                            identifier: format!("{}.{}", export.name, member.name),
                            span: member.span,
                            tag,
                        }));
                    } else if excluding.peek().is_none() && !self.lacks_included_tag(&member.tags) {
                        hides_member = true;
                    }
                }
            }
            if !hides_member {
                found.extend(self.excluding_tags(&export.tags).map(|tag| UnneededTag {
                    identifier: export.name.clone(),
                    span: export.span,
                    tag,
                }));
            }
        }
        found
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use skif_workspace::PackageId;

    use super::{group_by_owner, relative_to};

    #[test]
    fn files_are_grouped_by_owner_in_their_order() {
        let (a, b) = (PackageId::from_index(1), PackageId::from_index(2));
        let groups = group_by_owner([
            (a, Path::new("/w/a/x.ts")),
            (b, Path::new("/w/b/y.ts")),
            (a, Path::new("/w/a/src/z.ts")),
        ]);
        assert_eq!(groups.len(), 2);
        assert_eq!(
            groups.get(&a).map(Vec::as_slice),
            Some(&[Path::new("/w/a/x.ts"), Path::new("/w/a/src/z.ts")][..])
        );
        assert_eq!(groups.get(&b).map(Vec::as_slice), Some(&[Path::new("/w/b/y.ts")][..]));
        assert_eq!(groups.get(&PackageId::ROOT), None);
    }

    #[test]
    fn only_the_files_below_the_directory_are_made_relative() {
        let group = [
            Path::new("/w/a/x.ts"),
            Path::new("/w/elsewhere/y.ts"),
            Path::new("/w/a/src/z.ts"),
        ];
        assert_eq!(relative_to(Some(&group[..]), Path::new("/w/a")), ["x.ts", "src/z.ts"]);
        assert_eq!(relative_to(None, Path::new("/w/a")), [] as [std::string::String; 0]);
    }
}
