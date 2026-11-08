//! The analysis the rules read: module graph, export usage and dependency usage.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_config::{Config, JsDiscovery};
use skif_core::{FileSystem, timing};
use skif_js_syntax::{Export, ExportKind, ParseOptions, parse_module};
use skif_workspace::{PackageId, Workspace, map_ordered};

use crate::deps::DependencyUsage;
use crate::discover::{Candidate, discover, relative, take_file_links};
use crate::entries::entry_files;
use crate::fixture::in_fixture_directory;
use crate::git_index::{IndexCache, Listing, indexed_fs};
use crate::graph::{self, Inputs, Module, ModuleGraph};
use crate::input::InputSource;
use crate::resolve::ModuleResolver;
use crate::setup::Setup;
use crate::specifier::package_name;
use crate::usage::ExportUsage;

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
    pub(crate) setup: Setup,
    /// The ids of the rules that run in this pass; `None` when every rule is assumed to run.
    pub(crate) enabled_rules: Option<HashSet<String>>,
    /// The configuration file, relative to the workspace root, that configuration findings point at.
    pub(crate) config_path: Option<PathBuf>,
}

/// Names declared with `declare module 'name'` by graph modules and by declaration files nothing imports.
fn collect_ambient_modules(
    fs: &dyn FileSystem,
    graph: &ModuleGraph,
    candidates: &[Candidate],
    sequential: bool,
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
            .and_then(|source| parse_module(path, &source, &options).ok())
            .map(|info| info.ambient_modules)
            .unwrap_or_default()
    });
    names.extend(found.into_iter().flatten());
    names
}

/// `fs` with the files tracked by git listed from the git index, as `[js] discovery` says: `auto` checks the
/// modification time of each directory against the index, `git` trusts the index alone and `walk` does not use it.
/// `None` for `walk` and when the workspace is not in a git work tree with a usable index.
fn overlay(
    fs: &Arc<dyn FileSystem>,
    workspace: &Workspace,
    config: &Config,
    cache: &IndexCache,
) -> Option<Arc<dyn FileSystem>> {
    let listing = match config.js.discovery {
        JsDiscovery::Walk => return None,
        JsDiscovery::Git => Listing::IndexOnly,
        _ => Listing::VerifyDirectories,
    };
    indexed_fs(fs, &workspace.root, cache, listing)
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
        Self::build_cached(workspace, config, fs, sources, sequential, &IndexCache::default())
    }

    /// Like [`build`](Self::build), reading the git index through `cache` so that the roots of one run share it.
    pub(crate) fn build_cached(
        workspace: &'a Workspace,
        config: &Config,
        fs: &Arc<dyn FileSystem>,
        sources: &[Arc<dyn InputSource>],
        sequential: bool,
        cache: &IndexCache,
    ) -> Self {
        let phase = |name: &str| format!("js {name} {}", workspace.root.display());
        let indexed = timing::phase(phase("git index"), || overlay(fs, workspace, config, cache));
        let listed_from_index = indexed.is_some();
        let fs = indexed.as_ref().unwrap_or(fs);
        let mut setup = timing::phase(phase("setup (plugins, configs)"), || {
            Setup::new(workspace, config, fs, sources, sequential)
        });
        let mut candidates = timing::phase(phase("discover"), || discover(fs.as_ref(), workspace, sequential));
        if !setup.js_ignored.is_empty() {
            candidates.retain(|candidate| !setup.js_ignored.contains(&candidate.owner));
        }
        let links = take_file_links(fs.as_ref(), &workspace.root, &mut candidates);
        let found = timing::phase(phase("entries"), || {
            entry_files(fs.as_ref(), workspace, &setup, &candidates)
        });
        setup.mark_entries_incomplete(&found.unmapped);
        let entries = found.files;
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
        };
        let graph = timing::phase(phase("graph (read, parse, resolve)"), || {
            graph::build(&inputs, &entries)
        });
        graph::report_timings(&workspace.root);
        let exports = timing::phase(phase("exports"), || {
            ExportUsage::compute_by(&graph, |owner| setup.includes_entry_exports(owner))
        });
        let dependencies = timing::phase(phase("dependencies"), || {
            DependencyUsage::compute(workspace, &setup, &graph, fs.as_ref(), sequential)
        });

        let mut ambient_modules = collect_ambient_modules(fs.as_ref(), &graph, &candidates, sequential);
        // `declare module 'pkg/sub'` also declares `pkg`: both spellings are looked up by name.
        let packages: Vec<String> = ambient_modules
            .iter()
            .filter_map(|declared| package_name(declared))
            .collect();
        ambient_modules.extend(packages);
        let unused_files = candidates
            .iter()
            .filter(|candidate| graph.id_of(&candidate.abs).is_none())
            .filter(|candidate| !candidate.rel.ends_with(".d.ts"))
            .filter(|candidate| {
                let Some(package) = setup.package(candidate.owner) else {
                    return false;
                };
                if package.entries_incomplete {
                    return false;
                }
                let Some(in_package) = relative(&candidate.abs, &package.dir) else {
                    return false;
                };
                package.project.is_match(&in_package)
                    && !package.ignore.matches_any(&in_package)
                    && !setup.global_ignore.matches_any(&candidate.rel)
                    && (setup.include_fixtures || !in_fixture_directory(Path::new(&candidate.rel)))
            })
            // The index may still list a file deleted since it was written; one `stat` per unused file settles it.
            .filter(|candidate| !listed_from_index || fs.kind(&candidate.abs).is_file)
            .map(|candidate| UnusedFile {
                path: PathBuf::from(&candidate.rel),
                owner: candidate.owner,
            })
            .collect();
        Self {
            workspace,
            graph,
            exports,
            dependencies,
            unused_files,
            ambient_modules,
            setup,
            enabled_rules: None,
            config_path: None,
        }
    }

    /// Whether the project declares package `name` itself with `declare module 'name'`, so importing it needs no
    /// dependency.
    #[must_use]
    pub fn declares_module(&self, name: &str) -> bool {
        self.ambient_modules.contains(name)
    }

    /// Whether `[js] production` is on.
    #[must_use]
    pub fn production(&self) -> bool {
        self.setup.production
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

    /// Whether the entry files of package `id` are not fully known, so nothing can be reported unused in it.
    #[must_use]
    pub fn entries_incomplete(&self, id: PackageId) -> bool {
        self.setup.package(id).is_some_and(|package| package.entries_incomplete)
    }

    /// Whether the package is excluded from analysis (`ignore-packages`).
    #[must_use]
    pub fn is_skipped(&self, id: PackageId) -> bool {
        self.setup.package(id).is_none()
    }

    /// Whether a module's own issues are reported: it is a project file, not ignored, not a public entry, not in a
    /// fixture directory unless `[js] include-fixtures` is set, and its package's entry files are fully known. In a
    /// package whose entries are incomplete, a file no known entry reaches may still import the module, so none of its
    /// exports can be judged unused.
    #[must_use]
    pub fn reports_exports_of(&self, module: &Module) -> bool {
        module.in_project
            && !module.ignored
            && (!module.is_entry || self.setup.includes_entry_exports(module.owner))
            && !self.entries_incomplete(module.owner)
            && (self.setup.include_fixtures || !in_fixture_directory(&module.path))
    }

    /// Exports of `module` that no other module uses, values and types alike, in source order.
    ///
    /// Exports tagged `@public`, `@beta` or `@alpha` (and `@internal` in production mode, and the `ignore-tags` of `unused-exports`) are not listed.
    #[must_use]
    pub fn unused_exports<'m>(&self, module: &'m Module) -> Vec<&'m Export> {
        if !self.reports_exports_of(module) {
            return Vec::new();
        }
        module
            .info
            .exports
            .iter()
            .filter(|export| !(export.kind == ExportKind::StarReExport && export.name == "*"))
            .filter(|export| export.name != "export=")
            .filter(|export| !self.is_tagged_public(export))
            .filter(|export| !(export.referenced_in_file && self.setup.ignores_in_file_use(module.owner, export)))
            .filter(|export| !self.exports.is_used(module.id, &export.name))
            .filter(|export| !self.in_used_signature(module, export, &mut HashSet::new()))
            .collect()
    }

    /// Whether `export` appears in the signature of another export of its module that is itself used.
    pub(crate) fn in_used_signature(&self, module: &Module, export: &Export, visited: &mut HashSet<String>) -> bool {
        export.referenced_in.iter().any(|name| {
            if self.exports.is_used(module.id, name) {
                return true;
            }
            visited.insert(name.clone())
                && module
                    .info
                    .exports
                    .iter()
                    .filter(|container| container.name == *name)
                    .any(|container| self.in_used_signature(module, container, visited))
        })
    }

    pub(crate) fn is_tagged_public(&self, export: &Export) -> bool {
        export.tags.iter().any(|tag| match tag.as_str() {
            "public" | "beta" | "alpha" => true,
            "internal" => self.setup.production,
            other => self.setup.ignore_tags.iter().any(|ignored| ignored == other),
        })
    }
}
