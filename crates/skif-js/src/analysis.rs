//! The analysis the rules read: module graph, export usage and dependency usage.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_config::{Config, JsDiscovery};
use skif_core::{FileSystem, Span, timing};
use skif_js_syntax::{Export, ExportKind, Member, ParseOptions, parse_module};
use skif_workspace::{PackageId, Workspace, map_ordered};

use crate::deps::DependencyUsage;
use crate::discover::{Candidate, discover, relative, take_file_links};
use crate::entries::entry_files;
use crate::git_index::{IndexCache, Listing, indexed_fs};
use crate::globs::Globs;
use crate::graph::{self, Inputs, Module, ModuleGraph};
use crate::input::InputSource;
use crate::resolve::ModuleResolver;
use crate::setup::Setup;
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

/// A configured `entry`, `project` or `ignore` pattern that matches no file the analysis knows (knip's `entry-empty`,
/// `project-empty` and `ignore` hints).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmptyPattern {
    /// The configuration key, such as `js.entry` or `js.workspaces."packages/a".project`.
    pub key: String,
    pub pattern: String,
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
    pub(crate) setup: Setup,
    /// The ids of the rules that run in this pass; `None` when every rule is assumed to run.
    pub(crate) enabled_rules: Option<HashSet<String>>,
    /// The configuration file, relative to the workspace root, that configuration findings point at.
    pub(crate) config_path: Option<PathBuf>,
    /// Configured `entry`, `project` and `ignore` patterns that match no file.
    pub(crate) empty_patterns: Vec<EmptyPattern>,
    /// `ignore-files` patterns that hide no file `unused-files` would report.
    pub(crate) unused_ignore_files: Vec<EmptyPattern>,
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

/// The configured `entry`, `project` and `ignore` patterns that match none of the files the analysis knows: the
/// discovered source files and the modules of the graph. Negated patterns, patterns that leave the package (`../`) and
/// the defaults are not judged. A file only reaches a package's patterns when the package owns it, as for entries.
fn empty_patterns(
    workspace: &Workspace,
    config: &Config,
    setup: &Setup,
    candidates: &[Candidate],
    graph: &ModuleGraph,
) -> Vec<EmptyPattern> {
    let files: Vec<(PackageId, &Path, String)> = candidates
        .iter()
        .map(|candidate| (candidate.owner, candidate.abs.as_path(), candidate.rel.clone()))
        .chain(graph.modules().iter().map(|module| {
            (
                module.owner,
                module.abs.as_path(),
                module.path.to_string_lossy().replace('\\', "/"),
            )
        }))
        .collect();
    let judged = |pattern: &&String| {
        let body = pattern.trim();
        !body.is_empty() && !body.starts_with('!') && !body.starts_with("../")
    };
    let mut found = Vec::new();
    for pattern in config.js.ignore.iter().filter(judged) {
        let globs = Globs::new([pattern]);
        if !files.iter().any(|(_, _, rel)| globs.matches_any(rel)) {
            found.push(EmptyPattern {
                key: "js.ignore".to_owned(),
                pattern: pattern.clone(),
            });
        }
    }
    for (id, package) in workspace.iter() {
        let Some(package_setup) = setup.package(id) else {
            continue;
        };
        let owned: Vec<String> = files
            .iter()
            .filter(|(owner, _, _)| *owner == id)
            .filter_map(|(_, abs, _)| relative(abs, &package_setup.dir))
            .collect();
        let dir = if package.is_root {
            ".".to_owned()
        } else {
            package.dir.to_string_lossy().replace('\\', "/")
        };
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
                let globs = Globs::new([pattern]);
                if owned.iter().any(|rel| globs.matches_any(rel)) {
                    continue;
                }
                let key = if from_top_level {
                    format!("js.{name}")
                } else {
                    format!("js.workspaces.\"{dir}\".{name}")
                };
                found.push(EmptyPattern {
                    key,
                    pattern: pattern.clone(),
                });
            }
        }
    }
    found.dedup();
    found
}

/// The `ignore-files` patterns that hide none of the files that would be reported unused (knip's `ignoreFiles` hint).
fn unused_ignore_files(
    workspace: &Workspace,
    config: &Config,
    setup: &Setup,
    unreached: &[&Candidate],
) -> Vec<EmptyPattern> {
    let mut found = Vec::new();
    let all_judged = workspace
        .iter()
        .all(|(id, _)| setup.package(id).is_none_or(|package| !package.entries_incomplete));
    for pattern in config.js.ignore_files.iter().filter(|_| all_judged) {
        let globs = Globs::new([pattern]);
        if !globs.is_empty() && !unreached.iter().any(|candidate| globs.matches_any(&candidate.rel)) {
            found.push(EmptyPattern {
                key: "js.ignore-files".to_owned(),
                pattern: pattern.clone(),
            });
        }
    }
    for (id, package) in workspace.iter() {
        let Some(package_setup) = setup.package(id) else {
            continue;
        };
        if package_setup.entries_incomplete {
            continue;
        }
        let dir = if package.is_root {
            ".".to_owned()
        } else {
            package.dir.to_string_lossy().replace('\\', "/")
        };
        let owned: Vec<String> = unreached
            .iter()
            .filter(|candidate| candidate.owner == id)
            .filter_map(|candidate| relative(&candidate.abs, &package_setup.dir))
            .collect();
        for pattern in &package_setup.user_patterns.ignore_files {
            let globs = Globs::new([pattern]);
            if !globs.is_empty() && !owned.iter().any(|rel| globs.matches_any(rel)) {
                found.push(EmptyPattern {
                    key: format!("js.workspaces.\"{dir}\".ignore-files"),
                    pattern: pattern.clone(),
                });
            }
        }
    }
    found
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
        let unreached: Vec<&Candidate> = candidates
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
        let empty_patterns = empty_patterns(workspace, config, &setup, &candidates, &graph);
        let unused_ignore_files = unused_ignore_files(workspace, config, &setup, &unreached);
        let unused_files = unreached
            .iter()
            .filter(|candidate| !ignores_file(candidate))
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
            empty_patterns,
            unused_ignore_files,
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

    /// Whether a module's own issues are reported: it is a project file, not ignored, and not a public entry.
    #[must_use]
    pub fn reports_exports_of(&self, module: &Module) -> bool {
        module.in_project && !module.ignored && (!module.is_entry || self.setup.includes_entry_exports(module.owner))
    }

    /// Exports of `module` that no other module uses, values and types alike, in source order.
    ///
    /// Exports tagged `@public`, `@beta`, `@alpha` or `@alias` (and `@internal` in production mode, and the
    /// `ignore-tags` of `unused-exports`) are not listed, unless `include-tags` names the tag.
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
    fn export_in_use(&self, module: &Module, export: &Export) -> bool {
        (export.referenced_in_file && self.setup.ignores_in_file_use(module.owner, export))
            || self.exports.is_used(module.id, &export.name)
            || self.in_used_signature(module, export, &mut HashSet::new())
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
        for export in &module.info.exports {
            if self.always_exempt(&export.tags) || !self.export_in_use(module, export) {
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
