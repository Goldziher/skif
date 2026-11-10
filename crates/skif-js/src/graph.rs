//! The module graph: a worklist from the entry files over resolved imports.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use globset::{GlobBuilder, GlobSetBuilder};
use skif_core::FileSystem;
use skif_core::timing::{self, Tally};
use skif_js_syntax::{
    CachedModule, ImportKind, ModuleCache, ModuleInfo, ModuleKey, ParseOptions, RegisteredComponent, is_source_query,
    is_stylesheet, parse_cached, registered_components, registered_name,
};
use skif_workspace::{PackageId, map_ordered};

use crate::components::{self, Registered};
use crate::discover::{Candidate, is_package_manifest, relative};
use crate::resolve::{EXTENSION_ALIASES, ModuleResolver, Resolved};
use crate::setup::Setup;
use crate::source_scan::add_textual_imports;

/// Identifies a module within one [`ModuleGraph`]. Ids follow discovery order, which is deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleId(u32);

impl ModuleId {
    /// Position of the module in [`ModuleGraph::modules`].
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    fn from_index(index: usize) -> Self {
        Self(u32::try_from(index).unwrap_or(u32::MAX))
    }
}

/// What an import points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A module of the graph.
    Module(ModuleId),
    /// The modules matched by an `import.meta.glob` pattern, or that a generated module (a missing file a
    /// `.gitignore` lists) may import.
    Modules(Vec<ModuleId>),
    /// A package that is not part of the graph, by name.
    External(String),
    /// A Node.js builtin.
    Builtin,
    /// A file that is not analysed (stylesheet, image, JSON) or lies outside the workspace.
    Asset,
    /// A scheme that is not followed (`https:`, `virtual:`).
    Ignored,
    /// Nothing was found.
    Unresolved,
}

/// One import of a module with the result of resolving it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedImport {
    /// Index into [`ModuleInfo::imports`] of the module.
    pub import: usize,
    /// What the specifier resolved to.
    pub target: Target,
    /// The package the import goes through, for package imports (installed or workspace).
    pub package: Option<String>,
    /// The import brings in types only.
    pub type_only: bool,
    /// The import is one skif adds for a registered component a template uses (ADR 0082), not one written in the
    /// file: it has no location, and `circular-imports` leaves it out.
    pub component: bool,
}

/// A parsed file of the graph.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools, reason = "independent facts about one file")]
pub struct Module {
    /// Position in the graph.
    pub id: ModuleId,
    /// Absolute path.
    pub abs: PathBuf,
    /// Path relative to the workspace root, as shown in diagnostics.
    pub path: PathBuf,
    /// The package whose directory is the closest ancestor.
    pub owner: PackageId,
    /// Matches the project globs of its package.
    pub in_project: bool,
    /// Matches an `ignore` glob, or is [`Module::foreign`]: issues in this file are not reported.
    pub ignored: bool,
    /// Belongs to another project: a directory between it and its package holds a `package.json` of its own, such as
    /// a nested root. That project judges the file against its own manifest, so its imports use no dependency of
    /// `owner` either.
    pub foreign: bool,
    /// Is an entry file; its exports are public.
    pub is_entry: bool,
    /// What the file declares and references.
    pub info: ModuleInfo,
    /// Why the file could not be parsed.
    pub parse_error: Option<String>,
    /// Resolution of every import, in source order.
    pub imports: Vec<ResolvedImport>,
}

impl Module {
    /// The module `export … from "specifier"` re-exports from.
    #[must_use]
    pub fn reexport_source(&self, specifier: &str) -> Option<ModuleId> {
        self.imports.iter().find_map(|resolved| {
            let import = self.info.imports.get(resolved.import)?;
            if import.kind != ImportKind::ExportFrom || import.specifier != specifier {
                return None;
            }
            match resolved.target {
                Target::Module(id) => Some(id),
                _ => None,
            }
        })
    }
}

/// The directories strictly between the file `abs` and the package directory `base` that could hold a project of
/// their own, innermost first. `None` when `abs` is not below `base` or lies below a `node_modules` directory, whose
/// files are installed code, not projects.
fn project_dir_candidates<'p>(abs: &'p Path, base: &Path) -> Option<Vec<&'p Path>> {
    if !abs.starts_with(base) {
        return None;
    }
    let between: Vec<&Path> = abs.ancestors().skip(1).take_while(|dir| *dir != base).collect();
    let installed = between
        .iter()
        .any(|dir| dir.file_name().is_some_and(|name| name == "node_modules"));
    (!installed).then_some(between)
}

/// Every module reachable from the entry files.
#[derive(Debug, Clone, Default)]
pub struct ModuleGraph {
    modules: Vec<Module>,
    index: HashMap<PathBuf, ModuleId>,
    imported_by: Vec<Vec<ModuleId>>,
}

impl ModuleGraph {
    /// All modules, in discovery order.
    #[must_use]
    pub fn modules(&self) -> &[Module] {
        &self.modules
    }

    /// The module with `id`.
    ///
    /// # Panics
    /// Never for an id of this graph.
    #[must_use]
    pub fn module(&self, id: ModuleId) -> &Module {
        &self.modules[id.index()]
    }

    /// The module for an absolute path.
    #[must_use]
    pub fn id_of(&self, abs: &Path) -> Option<ModuleId> {
        self.index.get(abs).copied()
    }

    /// Modules that import `id`, sorted and without duplicates.
    #[must_use]
    pub fn imported_by(&self, id: ModuleId) -> &[ModuleId] {
        self.imported_by.get(id.index()).map_or(&[], Vec::as_slice)
    }
}

/// What the parallel step learns about one file.
#[derive(Default)]
struct Parsed {
    info: ModuleInfo,
    error: Option<String>,
    imports: Vec<RawImport>,
    /// The components a generated `components.d.ts` registers, by name.
    components: Vec<(String, Registered)>,
    /// The positions in `imports` of the `import()` types that register them.
    registrations: Vec<usize>,
}

struct RawImport {
    import: usize,
    target: RawTarget,
    package: Option<String>,
    type_only: bool,
}

enum RawTarget {
    File(PathBuf),
    Files(Vec<PathBuf>),
    External(String),
    Builtin,
    Asset,
    Ignored,
    Unresolved,
}

/// Inputs shared by the parallel parse step.
pub(crate) struct Inputs<'a> {
    pub fs: &'a dyn FileSystem,
    pub resolver: &'a ModuleResolver,
    pub root: &'a Path,
    pub candidates: &'a [Candidate],
    /// Symbolic links to candidates, with their targets: a link is the file it points at.
    pub links: &'a HashMap<PathBuf, PathBuf>,
    pub setup: &'a Setup,
    /// Parse on the calling thread instead of the current rayon pool.
    pub sequential: bool,
    /// The host's cache of parse results, if it gave one.
    pub modules: Option<&'a dyn ModuleCache>,
}

fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// Files matched by an `import.meta.glob` pattern written in `importer`, or by the glob a computed `import()` stands
/// for. A pattern ending in an output extension also matches the sources it is compiled from, as a specifier does
/// (`./reporters/*.js` matches `reporters/a.ts`).
fn expand_glob(importer: &Path, pattern: &str, inputs: &Inputs<'_>) -> Vec<PathBuf> {
    if pattern.starts_with('!') {
        return Vec::new();
    }
    let joined = if let Some(rooted) = pattern.strip_prefix('/') {
        normalize(rooted)
    } else {
        let dir = importer
            .parent()
            .and_then(|parent| relative(parent, inputs.root))
            .unwrap_or_default();
        normalize(&format!("{dir}/{pattern}"))
    };
    let aliases = EXTENSION_ALIASES
        .iter()
        .filter_map(|(from, to)| Some((joined.strip_suffix(from)?, *to)))
        .flat_map(|(stem, to)| to.iter().map(move |alias| format!("{stem}{alias}")));
    let mut set = GlobSetBuilder::new();
    for pattern in std::iter::once(joined.clone()).chain(aliases) {
        let Ok(glob) = GlobBuilder::new(&pattern).literal_separator(true).build() else {
            return Vec::new();
        };
        set.add(glob);
    }
    let Ok(matcher) = set.build() else {
        return Vec::new();
    };
    inputs
        .candidates
        .iter()
        .filter(|candidate| candidate.abs != importer && matcher.is_match(&candidate.rel))
        .map(|candidate| candidate.abs.clone())
        .collect()
}

/// The source files a generated module in `dir` may import: those of the importer's package in `dir` (or below it,
/// when `recursive`), except the importer. They are read in full, as for a glob, since what the generated code takes
/// from them is not known.
fn generated_sources(importer: &Path, dir: &Path, recursive: bool, inputs: &Inputs<'_>) -> Vec<PathBuf> {
    let owner = inputs
        .candidates
        .iter()
        .find(|candidate| candidate.abs == importer)
        .map(|candidate| candidate.owner);
    inputs
        .candidates
        .iter()
        .filter(|candidate| {
            candidate.abs != importer
                && owner.is_none_or(|owner| candidate.owner == owner)
                && if recursive {
                    candidate.abs.starts_with(dir)
                } else {
                    candidate.abs.parent() == Some(dir)
                }
        })
        .map(|candidate| candidate.abs.clone())
        .collect()
}

static READ_TIME: Tally = Tally::new();
static PARSE_TIME: Tally = Tally::new();
static RESOLVE_TIME: Tally = Tally::new();
static CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static CACHE_MISSES: AtomicU64 = AtomicU64::new(0);

/// Prints the time `process` spent reading, parsing and resolving since the last report (`SKIF_TIMINGS`), and how
/// the module cache answered when there is one.
pub(crate) fn report_timings(root: &Path) {
    READ_TIME.report(format_args!("js read {}", root.display()));
    PARSE_TIME.report(format_args!("js parse {}", root.display()));
    RESOLVE_TIME.report(format_args!("js resolve {}", root.display()));
    let hits = CACHE_HITS.swap(0, Ordering::Relaxed);
    let misses = CACHE_MISSES.swap(0, Ordering::Relaxed);
    if hits + misses > 0 {
        timing::note(format_args!(
            "js module cache {}: {hits} hits, {misses} misses",
            root.display()
        ));
    }
}

/// The host's [`ModuleCache`], counting its hits and misses for [`report_timings`].
struct Counted<'a>(&'a dyn ModuleCache);

impl ModuleCache for Counted<'_> {
    fn get(&self, key: ModuleKey) -> Option<CachedModule> {
        let found = self.0.get(key);
        if timing::enabled() {
            let counter = if found.is_some() { &CACHE_HITS } else { &CACHE_MISSES };
            counter.fetch_add(1, Ordering::Relaxed);
        }
        found
    }

    fn put(&self, key: ModuleKey, module: CachedModule) {
        self.0.put(key, module);
    }
}

fn process(path: &Path, inputs: &Inputs<'_>) -> Parsed {
    let failed = |message: String| Parsed {
        error: Some(message),
        ..Parsed::default()
    };
    let source = match READ_TIME.time(|| inputs.fs.read_to_string(path)) {
        Ok(source) => source,
        // A file the git index still lists but that was deleted from disk is no module, not a read error.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Parsed::default(),
        Err(error) => return failed(error.to_string()),
    };
    let style = is_stylesheet(path);
    let component = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "vue" | "svelte" | "astro"));
    let counted = inputs.modules.map(Counted);
    let parsed = PARSE_TIME.time(|| {
        parse_cached(
            counted.as_ref().map(|cache| cache as &dyn ModuleCache),
            path,
            &source,
            &ParseOptions::default(),
        )
    });
    let mut info = match parsed {
        Ok(info) => info,
        Err(error) => return failed(error.to_string()),
    };
    let error = info.fatal.then(|| {
        info.parse_diagnostics
            .first()
            .map_or_else(|| "syntax error".to_owned(), |issue| issue.message.clone())
    });
    let parsed_imports = info.imports.len();
    // The prose and code samples of a document are not code: only its ESM, which the parser already read, counts.
    let document = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mdx"));
    if !style && !document {
        add_textual_imports(&mut info, &source);
    }
    let imports: Vec<RawImport> = info
        .imports
        .iter()
        .enumerate()
        .map(|(index, import)| {
            let type_only = import.kind == ImportKind::TypeOnly
                || (!import.names.is_empty() && import.names.iter().all(|name| name.is_type));
            // A glob that reads the text of its files (`query: '?raw'`) does not load them as modules.
            if import.kind == ImportKind::SourceGlob {
                return RawImport {
                    import: index,
                    target: RawTarget::Asset,
                    package: None,
                    type_only,
                };
            }
            if import.kind == ImportKind::ImportMetaGlob {
                return RawImport {
                    import: index,
                    target: RawTarget::Files(expand_glob(path, &import.specifier, inputs)),
                    package: None,
                    type_only,
                };
            }
            let outcome = RESOLVE_TIME.time(|| {
                if style {
                    inputs.resolver.resolve_style(path, &import.specifier)
                } else {
                    let found = inputs.resolver.resolve(path, &import.specifier);
                    // A component's style blocks import from the same list as its script, and Sass partials are
                    // not files the script resolver can name.
                    if component && matches!(found.target, Resolved::Unresolved) {
                        let styled = inputs.resolver.resolve_style(path, &import.specifier);
                        if matches!(styled.target, Resolved::File(_)) {
                            return styled;
                        }
                    }
                    found
                }
            });
            // A `?raw` specifier reads the file's text too: its package still counts as used, the file does not.
            let reads_source = import
                .specifier
                .split_once('?')
                .is_some_and(|(_, query)| is_source_query(query));
            let target = match outcome.target {
                Resolved::File(_) if reads_source => RawTarget::Asset,
                Resolved::File(file) => RawTarget::File(file),
                Resolved::Asset => RawTarget::Asset,
                Resolved::External(name) => RawTarget::External(name),
                Resolved::Builtin => RawTarget::Builtin,
                Resolved::Ignored => RawTarget::Ignored,
                Resolved::Generated { dir, recursive } => {
                    RawTarget::Files(generated_sources(path, &dir, recursive, inputs))
                }
                // Specifiers found by text scan may name directories or assets; they are followed when they resolve.
                Resolved::Unresolved if index >= parsed_imports => RawTarget::Asset,
                Resolved::Unresolved => RawTarget::Unresolved,
            };
            RawImport {
                import: index,
                target,
                package: outcome.package,
                type_only,
            }
        })
        .collect();
    let (components, registrations) = if components::is_generated(path, &source) {
        registered(&info, &imports, &registered_components(&source))
    } else {
        (Vec::new(), Vec::new())
    };
    Parsed {
        info,
        error,
        imports,
        components,
        registrations,
    }
}

/// The local components a generated `components.d.ts` registers, and the positions of the imports that register
/// them: where the graph links components by tag, such a component is used where a template uses it, not by the
/// declaration file (ADR 0082).
fn registered(
    info: &ModuleInfo,
    imports: &[RawImport],
    registered: &[RegisteredComponent],
) -> (Vec<(String, Registered)>, Vec<usize>) {
    let mut found = Vec::new();
    let mut positions = Vec::new();
    for (position, raw) in imports.iter().enumerate() {
        let (Some(import), RawTarget::File(file)) = (info.imports.get(raw.import), &raw.target) else {
            continue;
        };
        let before = found.len();
        found.extend(
            registered
                .iter()
                .filter(|component| component.specifier == import.specifier)
                .map(|component| {
                    let registration = Registered {
                        specifier: component.specifier.clone(),
                        file: file.clone(),
                    };
                    (component.name.clone(), registration)
                }),
        );
        if found.len() > before {
            positions.push(position);
        }
    }
    (found, positions)
}

struct Builder<'a> {
    inputs: &'a Inputs<'a>,
    candidate_index: HashMap<&'a Path, usize>,
    package_dirs: Vec<(PathBuf, PackageId)>,
    modules: Vec<Module>,
    index: HashMap<PathBuf, ModuleId>,
    /// Whether a directory holds a real `package.json`, by directory: the files of one directory share their
    /// ancestors, so each is checked once.
    project_dirs: HashMap<PathBuf, bool>,
    /// The files of the components generated declaration files register, by name, in the order they were found.
    /// Registrations are global: a component one workspace's declaration file registers is linked into the templates
    /// of every workspace that uses its tag.
    components: BTreeMap<String, Vec<Registered>>,
    /// The modules already importing a file of a registered component, with the file.
    linked: BTreeSet<(ModuleId, PathBuf)>,
    /// The modules below this index were linked against the components registered then.
    linked_upto: usize,
    /// A component or a file of one was registered since the last linking, so every module is linked again.
    relink: bool,
    /// Whether the components a package registers may be used where skif records no tags, by package.
    untagged: HashMap<PackageId, bool>,
}

impl<'a> Builder<'a> {
    fn new(inputs: &'a Inputs<'a>) -> Self {
        let candidate_index = inputs
            .candidates
            .iter()
            .enumerate()
            .map(|(position, candidate)| (candidate.abs.as_path(), position))
            .collect();
        let mut package_dirs: Vec<(PathBuf, PackageId)> = inputs
            .setup
            .packages
            .iter()
            .flatten()
            .map(|package| (package.dir.clone(), package.id))
            .collect();
        package_dirs.sort_by_key(|(dir, _)| std::cmp::Reverse(dir.components().count()));
        Self {
            inputs,
            candidate_index,
            package_dirs,
            modules: Vec::new(),
            index: HashMap::new(),
            project_dirs: HashMap::new(),
            components: BTreeMap::new(),
            linked: BTreeSet::new(),
            linked_upto: 0,
            relink: false,
            untagged: HashMap::new(),
        }
    }

    fn owner_of(&self, abs: &Path) -> PackageId {
        self.package_dirs
            .iter()
            .find(|(dir, _)| abs.starts_with(dir))
            .map_or(PackageId::ROOT, |(_, id)| *id)
    }

    /// Whether `abs`, a file that is no candidate, lies in a project of its own below the package `owner`: a
    /// directory between the two holds a real `package.json`. Such a file is reached from this workspace (a script
    /// runs it, a file imports it) but belongs to that project, often a nested root of the same run, which judges it
    /// against its own manifest. Files below `node_modules` are installed code, not projects.
    fn in_other_project(&mut self, abs: &Path, owner: PackageId) -> bool {
        let inputs = self.inputs;
        let base = self
            .package_dirs
            .iter()
            .find(|(_, id)| *id == owner)
            .map_or(inputs.root, |(dir, _)| dir.as_path());
        let Some(between) = project_dir_candidates(abs, base) else {
            return false;
        };
        between.into_iter().any(|dir| {
            *self
                .project_dirs
                .entry(dir.to_path_buf())
                .or_insert_with(|| inputs.fs.is_file(&dir.join("package.json")) && is_package_manifest(inputs.fs, dir))
        })
    }

    /// The id of `abs`, creating an unparsed module on first sight. The flag is true for a new module.
    fn intern(&mut self, abs: &Path) -> (ModuleId, bool) {
        let inputs = self.inputs;
        let abs = inputs.links.get(abs).map_or(abs, PathBuf::as_path);
        if let Some(id) = self.index.get(abs) {
            return (*id, false);
        }
        let id = ModuleId::from_index(self.modules.len());
        let candidate = self
            .candidate_index
            .get(abs)
            .map(|position| &inputs.candidates[*position]);
        let owner = candidate.map_or_else(|| self.owner_of(abs), |candidate| candidate.owner);
        let rel = relative(abs, inputs.root).unwrap_or_else(|| abs.to_string_lossy().into_owned());
        let setup = inputs.setup.package(owner);
        let package_rel = setup.and_then(|setup| relative(abs, &setup.dir));
        let foreign = candidate.is_none() && self.in_other_project(abs, owner);
        let ignored = foreign
            || inputs.setup.global_ignore.matches_any(&rel)
            || setup
                .zip(package_rel.as_deref())
                .is_some_and(|(setup, rel)| setup.ignore.matches_any(rel));
        let in_project = candidate.is_some()
            && setup
                .zip(package_rel.as_deref())
                .is_some_and(|(setup, rel)| setup.project.is_match(rel));
        self.modules.push(Module {
            id,
            abs: abs.to_path_buf(),
            path: PathBuf::from(rel),
            owner,
            in_project,
            ignored,
            foreign,
            is_entry: false,
            info: ModuleInfo::default(),
            parse_error: None,
            imports: Vec::new(),
        });
        self.index.insert(abs.to_path_buf(), id);
        (id, true)
    }

    fn lower(&mut self, target: RawTarget, next: &mut Vec<ModuleId>) -> Target {
        match target {
            RawTarget::File(path) => {
                let (id, fresh) = self.intern(&path);
                if fresh {
                    next.push(id);
                }
                Target::Module(id)
            }
            RawTarget::Files(paths) => Target::Modules(
                paths
                    .iter()
                    .map(|path| {
                        let (id, fresh) = self.intern(path);
                        if fresh {
                            next.push(id);
                        }
                        id
                    })
                    .collect(),
            ),
            RawTarget::External(name) => Target::External(name),
            RawTarget::Builtin => Target::Builtin,
            RawTarget::Asset => Target::Asset,
            RawTarget::Ignored => Target::Ignored,
            RawTarget::Unresolved => Target::Unresolved,
        }
    }

    /// Whether the components the declaration file `id` registers are linked by tag: not when its package may use them
    /// in files skif records no tags for, where the file keeps its edges and credits them all.
    fn links_by_tag(&mut self, id: ModuleId) -> bool {
        let inputs = self.inputs;
        let owner = self.modules[id.index()].owner;
        let untagged = *self.untagged.entry(owner).or_insert_with(|| {
            inputs.setup.package(owner).is_none_or(|package| {
                components::may_be_used_untagged(inputs.fs, package, inputs.candidates)
            })
        });
        !untagged
    }

    fn register(&mut self, components: Vec<(String, Registered)>) {
        for (name, registration) in components {
            let files = self.components.entry(name).or_default();
            if !files.iter().any(|known| known.file == registration.file) {
                files.push(registration);
                self.relink = true;
            }
        }
    }

    /// Imports each registered component into the modules whose `template_tags` name it, as knip's compiler adds the
    /// import (ADR 0082), and returns the modules this reaches for the first time. Only the modules read since the
    /// last linking are visited, unless components were registered since.
    fn link_components(&mut self) -> Vec<ModuleId> {
        let mut fresh = Vec::new();
        let start = if std::mem::take(&mut self.relink) {
            0
        } else {
            self.linked_upto
        };
        let end = self.modules.len();
        self.linked_upto = end;
        if self.components.is_empty() {
            return fresh;
        }
        for index in start..end {
            let module = &self.modules[index];
            let mut links: Vec<(String, Registered, bool)> = Vec::new();
            for tag in &module.info.template_tags {
                let Some(name) = registered_name(tag, |name| self.components.contains_key(name)) else {
                    continue;
                };
                let Some(files) = self.components.get(&name) else {
                    continue;
                };
                let mut first = true;
                for registration in files.iter().filter(|registration| registration.file != module.abs) {
                    links.push((name.clone(), registration.clone(), first));
                    first = false;
                }
            }
            let id = ModuleId::from_index(index);
            for (name, registration, first) in links {
                if !self.linked.insert((id, registration.file.clone())) {
                    continue;
                }
                let target = self.lower(RawTarget::File(registration.file), &mut fresh);
                let module = &mut self.modules[index];
                let import = module.info.imports.len();
                module
                    .info
                    .imports
                    .push(components::import(&name, &registration.specifier, first));
                module.imports.push(ResolvedImport {
                    import,
                    target,
                    package: None,
                    type_only: false,
                    component: true,
                });
            }
        }
        fresh
    }
}

/// Builds the graph by following imports from `entries` in rounds. Each round parses and resolves its files in
/// parallel and merges the results in order, so ids and edges do not depend on scheduling.
pub(crate) fn build(inputs: &Inputs<'_>, entries: &[PathBuf]) -> ModuleGraph {
    let mut builder = Builder::new(inputs);
    let mut frontier = Vec::new();
    for entry in entries {
        let (id, fresh) = builder.intern(entry);
        builder.modules[id.index()].is_entry = true;
        if fresh {
            frontier.push(id);
        }
    }
    while !frontier.is_empty() {
        let paths: Vec<PathBuf> = frontier
            .iter()
            .map(|id| builder.modules[id.index()].abs.clone())
            .collect();
        let parsed: Vec<Parsed> = map_ordered(&paths, inputs.sequential, |path| process(path, inputs));
        let mut next = Vec::new();
        for (id, mut parsed) in frontier.iter().zip(parsed) {
            if !parsed.components.is_empty() && builder.links_by_tag(*id) {
                for position in &parsed.registrations {
                    parsed.imports[*position].target = RawTarget::Ignored;
                }
                builder.register(std::mem::take(&mut parsed.components));
            }
            let imports = parsed
                .imports
                .into_iter()
                .map(|raw| ResolvedImport {
                    import: raw.import,
                    target: builder.lower(raw.target, &mut next),
                    package: raw.package,
                    type_only: raw.type_only,
                    component: false,
                })
                .collect();
            let module = &mut builder.modules[id.index()];
            module.info = parsed.info;
            module.parse_error = parsed.error;
            module.imports = imports;
        }
        frontier = next;
        if frontier.is_empty() {
            // Components are linked once every declaration file reached so far is read; the components this reaches
            // are parsed in the next round and linked in turn.
            frontier = builder.link_components();
        }
    }

    let mut imported_by: Vec<BTreeSet<ModuleId>> = vec![BTreeSet::new(); builder.modules.len()];
    for module in &builder.modules {
        for import in &module.imports {
            let targets: &[ModuleId] = match &import.target {
                Target::Module(id) => std::slice::from_ref(id),
                Target::Modules(ids) => ids,
                _ => &[],
            };
            for target in targets {
                imported_by[target.index()].insert(module.id);
            }
        }
    }
    ModuleGraph {
        modules: builder.modules,
        index: builder.index,
        imported_by: imported_by.into_iter().map(|set| set.into_iter().collect()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_directories_between_a_file_and_its_package_may_be_projects_unless_installed() {
        let base = Path::new("/ws");
        assert_eq!(
            project_dir_candidates(Path::new("/ws/build/npm/postinstall.ts"), base),
            Some(vec![Path::new("/ws/build/npm"), Path::new("/ws/build")])
        );
        assert_eq!(project_dir_candidates(Path::new("/ws/main.ts"), base), Some(Vec::new()));
        assert_eq!(
            project_dir_candidates(Path::new("/ws/node_modules/tool/cli.js"), base),
            None
        );
        assert_eq!(
            project_dir_candidates(Path::new("/ws/a/node_modules/@s/t/x.js"), base),
            None
        );
        assert_eq!(project_dir_candidates(Path::new("/other/x.ts"), base), None);
    }
}
