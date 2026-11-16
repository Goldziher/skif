//! The module graph: a worklist from the entry files over resolved imports.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use skif_core::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use globset::{GlobBuilder, GlobSetBuilder};
use skif_core::FileSystem;
use skif_core::timing::{self, Tally};
use skif_js_syntax::{
    ImportKind, ModuleInfo, ParseOptions, RegisteredComponent, SyntaxError, is_source_query, is_stylesheet,
    parse_cached, registered_components, registered_name,
};
use skif_workspace::{PackageId, is_package_manifest, map_ordered};

use crate::compilers::{self, Compiler, reader_path};
use crate::components::{self, Registered};
use crate::discover::{Candidate, relative};
use crate::resolve::{CODE_EXTENSIONS, EXTENSION_ALIASES, ModuleResolver, Resolved};
use crate::setup::Setup;
use crate::shared_modules::{ModuleParser, Served};
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
    /// A file that is not analysed (stylesheet, image, JSON), or installed code outside the workspace.
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
    /// The import is one skif adds for a registered component a template uses (ADR 0020), not one written in the
    /// file: it has no location, and `circular-imports` leaves it out.
    pub component: bool,
}

/// A parsed file of the graph.
#[derive(Debug, Clone)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools, reason = "independent facts about one file")]
pub struct Module {
    /// Position in the graph.
    pub id: ModuleId,
    /// Absolute path.
    pub abs: PathBuf,
    /// Path relative to the workspace root, as shown in diagnostics.
    pub path: PathBuf,
    /// The package whose directory is the closest ancestor, or the package that adopted the file: a file of the root
    /// that is no candidate and is reached through the `rootDirs` of an importer's `tsconfig.json` belongs to the
    /// importer's package, as do such files it reaches by relative imports below the same root directory.
    pub owner: PackageId,
    /// Matches the project globs of its package.
    pub in_project: bool,
    /// Matches an `ignore` glob, or is [`Module::foreign`]: issues in this file are not reported.
    pub ignored: bool,
    /// Belongs to another project: a directory between it and its package holds a `package.json` of its own, such as
    /// a nested root, or it lies outside the workspace root (a file of an enclosing or a sibling root). That project
    /// judges the file against its own manifest, so its imports use no dependency of `owner` either.
    pub foreign: bool,
    /// Is an entry file; its exports are public.
    pub is_entry: bool,
    /// Is a build source that is no entry, a file a plugin names as one a tool builds from (the `srcs` and `data` of a
    /// Bazel rule): the graph starts from it as from an entry, so it and what it imports are used, but its exports are
    /// judged as any module's.
    pub is_build_source: bool,
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
    /// The texts of the modules whose package scans the sources the graph reaches, as read while building it.
    texts: Vec<(ModuleId, Arc<str>)>,
    /// What reading and parsing each module gave, when the graph keeps it for one built after it
    /// ([`Inputs::keep_scans`]).
    scans: Scans,
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

    /// The texts the graph kept of the modules below a package that scans the source files the graph reaches
    /// ([`PackageSetup::scans_reached_sources`](crate::setup::PackageSetup::scans_reached_sources)), by the package
    /// that owns the module or the one whose directory holds it, as read while building the graph, in discovery
    /// order. A file that could not be read has none.
    pub(crate) fn kept_texts(&self) -> &[(ModuleId, Arc<str>)] {
        &self.texts
    }

    /// Lets go of the [`kept_texts`](Self::kept_texts) once they have been searched, and of the scans kept for a graph
    /// built after this one.
    pub(crate) fn release_texts(&mut self) {
        self.texts = Vec::new();
        self.scans = Scans::default();
    }

    /// What reading and parsing each module gave, for a graph built again in the same analysis ([`Inputs::earlier`]);
    /// empty unless the graph kept it ([`Inputs::keep_scans`]).
    pub(crate) fn into_scans(self) -> Scans {
        self.scans
    }
}

/// What reading and parsing a file gives, before its imports are resolved. It depends on the file and on
/// `[js.compilers]` alone, which the graphs of one analysis share, so a graph built again once the reached sources were
/// searched takes it from the graph before instead of reading and parsing the file again. Resolution is redone: the
/// setup of the new graph can change the aliases, paths and extensions the resolver follows.
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools, reason = "independent facts about one file")]
pub(crate) struct Scan {
    info: ModuleInfo,
    error: Option<String>,
    /// The file's text, when it was kept.
    text: Option<Arc<str>>,
    /// The file was read: one that is gone or could not be read has no text to keep.
    read: bool,
    /// The file was parsed, so its imports are resolved.
    parsed: bool,
    /// How many of the imports of `info` the parser found; a text scan added the others.
    parsed_imports: usize,
    /// The file is read as a stylesheet.
    style: bool,
    /// The file is read as a single-file component (Vue, Svelte, Astro).
    component: bool,
    /// The components the file registers, when it is a generated `components.d.ts`.
    registers: Vec<RegisteredComponent>,
}

/// The [`Scan`]s of the modules of a graph, by absolute path.
#[derive(Debug, Clone, Default)]
pub(crate) struct Scans(HashMap<PathBuf, Arc<Scan>>);

/// What the parallel step learns about one file.
#[derive(Default)]
struct Parsed {
    info: ModuleInfo,
    error: Option<String>,
    /// The file's text, when the graph keeps it ([`ModuleGraph::kept_texts`]).
    text: Option<Arc<str>>,
    imports: Vec<RawImport>,
    /// The components a generated `components.d.ts` registers, by name.
    components: Vec<(String, Registered)>,
    /// The positions in `imports` of the `import()` types that register them.
    registrations: Vec<usize>,
    /// What reading and parsing the file gave, when the graph keeps it ([`Inputs::keep_scans`]).
    scan: Option<Arc<Scan>>,
}

struct RawImport {
    import: usize,
    target: RawTarget,
    package: Option<String>,
    type_only: bool,
    reach: Reach,
}

/// How an import reaches its target, which decides the package of a file it reaches first ([`Builder::adopter`]).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Reach {
    /// Through the `rootDirs` of the importer's `tsconfig.json`, below the root directory given.
    RootDirs(PathBuf),
    /// Through a relative specifier.
    Relative,
    /// Any other way.
    Other,
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
    /// How this run parses through the cache of parse results, when there is one.
    pub modules: Option<&'a ModuleParser<'a>>,
    /// What reading and parsing gave for the modules of a graph built before this one in the same analysis
    /// ([`ModuleGraph::into_scans`]): a file among them is not read or parsed again, unless its text is wanted and was
    /// not kept.
    pub earlier: &'a Scans,
    /// The graph keeps what reading and parsing gave for each module, for a graph built after it.
    pub keep_scans: bool,
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
/// (`./reporters/*.js` matches `reporters/a.ts`), and one whose last segment is a plain name without an extension
/// matches that name with each code extension (`./locales/*/messages` matches `locales/en/messages.js`).
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
    let last_segment = joined.rsplit('/').next().unwrap_or_default();
    let extensionless = (!last_segment.is_empty() && !last_segment.contains(['.', '*', '?', '[', '{']))
        .then(|| format!("{joined}.{{{}}}", CODE_EXTENSIONS.join(",")));
    let mut set = GlobSetBuilder::new();
    for pattern in std::iter::once(joined.clone()).chain(aliases).chain(extensionless) {
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
static WAIT_TIME: Tally = Tally::new();
static RESOLVE_TIME: Tally = Tally::new();
static CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static CACHE_MISSES: AtomicU64 = AtomicU64::new(0);
static CACHE_WAITS: AtomicU64 = AtomicU64::new(0);

/// Prints the time `process` spent reading, parsing and resolving since the last report (`SKIF_TIMINGS`), and how
/// the module cache answered when there is one. A lookup that waited for another root's parse of the same file is a
/// wait, and its time goes to `js parse wait`, not `js parse`.
pub(crate) fn report_timings(root: &Path) {
    READ_TIME.report(format_args!("js read {}", root.display()));
    PARSE_TIME.report(format_args!("js parse {}", root.display()));
    RESOLVE_TIME.report(format_args!("js resolve {}", root.display()));
    let hits = CACHE_HITS.swap(0, Ordering::Relaxed);
    let misses = CACHE_MISSES.swap(0, Ordering::Relaxed);
    let waits = CACHE_WAITS.swap(0, Ordering::Relaxed);
    if hits + misses + waits > 0 {
        timing::note(format_args!(
            "js module cache {}: {hits} hits, {misses} misses, {waits} waits",
            root.display()
        ));
    }
    if waits > 0 {
        WAIT_TIME.report(format_args!("js parse wait {}", root.display()));
    }
}

/// Parses `source`, the text of `path`, through the run's module cache when there is one, adding the time to
/// [`PARSE_TIME`], or to [`WAIT_TIME`] for a lookup that waited for another root's parse, and counting how the cache
/// answered.
fn parse_source(modules: Option<&ModuleParser<'_>>, path: &Path, source: &str) -> Result<ModuleInfo, SyntaxError> {
    let options = ParseOptions::default();
    let Some(modules) = modules else {
        return PARSE_TIME.time(|| parse_cached(None, path, source, &options));
    };
    if !timing::enabled() {
        return modules.parse(path, source, options).0;
    }
    let start = Instant::now();
    let (parsed, served) = modules.parse(path, source, options);
    let (tally, counter) = match served {
        Served::Uncached => (&PARSE_TIME, None),
        Served::Hit => (&PARSE_TIME, Some(&CACHE_HITS)),
        Served::Parsed => (&PARSE_TIME, Some(&CACHE_MISSES)),
        Served::Waited => (&WAIT_TIME, Some(&CACHE_WAITS)),
    };
    tally.add(start.elapsed());
    if let Some(counter) = counter {
        counter.fetch_add(1, Ordering::Relaxed);
    }
    parsed
}

/// What the graph learns about the file `path`, with its text when `keep_text`. What reading and parsing it gives is
/// taken from [`Inputs::earlier`] when it is there (with the text, when that is wanted).
fn process(path: &Path, keep_text: bool, inputs: &Inputs<'_>) -> Parsed {
    let earlier = inputs
        .earlier
        .0
        .get(path)
        .filter(|scan| !keep_text || scan.text.is_some() || !scan.read);
    let (scan, kept) = if let Some(earlier) = earlier {
        let mut scan = Scan::clone(earlier);
        if !keep_text {
            scan.text = None;
        }
        (scan, inputs.keep_scans.then(|| Arc::clone(earlier)))
    } else {
        let scan = read_and_parse(path, keep_text, inputs);
        let kept = inputs.keep_scans.then(|| Arc::new(scan.clone()));
        (scan, kept)
    };
    Parsed {
        scan: kept,
        ..resolve_imports(path, scan, inputs)
    }
}

/// Reads and parses the file `path`, keeping its text when `keep_text`.
fn read_and_parse(path: &Path, keep_text: bool, inputs: &Inputs<'_>) -> Scan {
    let source = match READ_TIME.time(|| inputs.fs.read_to_string(path)) {
        Ok(source) => source,
        // A file the git index still lists but that was deleted from disk is no module, not a read error.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Scan::default(),
        Err(error) => {
            return Scan {
                error: Some(error.to_string()),
                ..Scan::default()
            };
        }
    };
    let text = keep_text.then(|| Arc::clone(&source));
    // A file `[js.compilers]` maps onto a reader is read as a file of that reader's extension, and resolves from
    // where it is; one read by patterns has only the imports they capture.
    let compiler = inputs.setup.compilers.of(path);
    let read_as = match compiler {
        Some(Compiler::Reader(reader)) => Cow::Owned(reader_path(path, reader)),
        _ => Cow::Borrowed(path),
    };
    let style = is_stylesheet(&read_as);
    let component = read_as
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "vue" | "svelte" | "astro"));
    let parsed = match compiler {
        Some(Compiler::Patterns(patterns)) => Ok(compilers::extract(patterns, &source)),
        _ => parse_source(inputs.modules, &read_as, &source),
    };
    let mut info = match parsed {
        Ok(info) => info,
        Err(error) => {
            return Scan {
                error: Some(error.to_string()),
                text,
                read: true,
                ..Scan::default()
            };
        }
    };
    let error = info.fatal.then(|| {
        info.parse_diagnostics
            .first()
            .map_or_else(|| "syntax error".to_owned(), |issue| issue.message.clone())
    });
    let parsed_imports = info.imports.len();
    // The prose and code samples of a document are not code: only its ESM, which the parser already read, counts.
    let document = read_as
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mdx"));
    if !style && !document && !matches!(compiler, Some(Compiler::Patterns(_))) {
        add_textual_imports(&mut info, &source);
    }
    let registers = if components::is_generated(path, &source) {
        registered_components(&source)
    } else {
        Vec::new()
    };
    Scan {
        info,
        error,
        text,
        read: true,
        parsed: true,
        parsed_imports,
        style,
        component,
        registers,
    }
}

/// What the graph learns about the file `path` from what reading and parsing it gave: its imports, resolved.
fn resolve_imports(path: &Path, scan: Scan, inputs: &Inputs<'_>) -> Parsed {
    let Scan {
        info,
        error,
        text,
        parsed,
        parsed_imports,
        style,
        component,
        registers,
        ..
    } = scan;
    if !parsed {
        return Parsed {
            info,
            error,
            text,
            ..Parsed::default()
        };
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
                    reach: Reach::Other,
                };
            }
            if import.kind == ImportKind::ImportMetaGlob {
                return RawImport {
                    import: index,
                    target: RawTarget::Files(expand_glob(path, &import.specifier, inputs)),
                    package: None,
                    type_only,
                    reach: Reach::Other,
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
            let reach = match outcome.root_dirs {
                Some(root_dir) => Reach::RootDirs(root_dir),
                None if import.specifier.starts_with('.') => Reach::Relative,
                None => Reach::Other,
            };
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
                reach,
            }
        })
        .collect();
    let (components, registrations) = if registers.is_empty() {
        (Vec::new(), Vec::new())
    } else {
        registered(&info, &imports, &registers)
    };
    Parsed {
        info,
        error,
        text,
        imports,
        components,
        registrations,
        scan: None,
    }
}

/// The local components a generated `components.d.ts` registers, and the positions of the imports that register
/// them: where the graph links components by tag, such a component is used where a template uses it, not by the
/// declaration file (ADR 0020).
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
    /// The modules a package adopted ([`Builder::adopter`]), with the root directory they lie below.
    adopted: HashMap<ModuleId, PathBuf>,
}

/// A package taking a file it reaches first through `rootDirs` from the package of its location ([`Builder::adopter`]).
struct Adoption {
    /// The package of the importing module.
    owner: PackageId,
    /// The root directory the file lies below (`bazel-bin/packages/app`).
    root_dir: PathBuf,
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
            index: HashMap::default(),
            project_dirs: HashMap::default(),
            components: BTreeMap::new(),
            linked: BTreeSet::new(),
            linked_upto: 0,
            relink: false,
            untagged: HashMap::default(),
            adopted: HashMap::default(),
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

    /// The package of `abs`, a path the links already resolve, by location: that of its candidate, else that of the
    /// closest package directory above it.
    fn located_owner(&self, abs: &Path) -> PackageId {
        self.candidate_index.get(abs).map_or_else(
            || self.owner_of(abs),
            |position| self.inputs.candidates[*position].owner,
        )
    }

    /// Whether another package may adopt `abs`: it is no candidate and lies in no package directory but the root's,
    /// as code generated into a build output tree does. A file of a workspace package always stays that package's.
    fn adoptable(&self, abs: &Path) -> bool {
        let abs = self.inputs.links.get(abs).map_or(abs, PathBuf::as_path);
        !self.candidate_index.contains_key(abs) && self.owner_of(abs) == PackageId::ROOT
    }

    /// The package that owns the file `raw` reaches when the module `importer` reaches it first, where that is not the
    /// root, in whose directory alone the file lies ([`Self::adoptable`]). A file found through `rootDirs` (generated
    /// code in a build output tree) belongs to the importer's package, whose dependencies Node.js resolves its imports
    /// from; so does a file that an adopted module reaches by a relative import below the root directory it was found
    /// below (generated code beside it), but not one in the output tree of another package.
    fn adopter(&self, importer: ModuleId, raw: &RawImport) -> Option<Adoption> {
        let RawTarget::File(file) = &raw.target else {
            return None;
        };
        let root_dir = match &raw.reach {
            Reach::RootDirs(root_dir) => root_dir,
            Reach::Relative => self.adopted.get(&importer)?,
            Reach::Other => return None,
        };
        let owner = self.modules[importer.index()].owner;
        (owner != PackageId::ROOT && file.starts_with(root_dir) && self.adoptable(file)).then(|| Adoption {
            owner,
            root_dir: root_dir.clone(),
        })
    }

    /// Whether the graph keeps the text of `module` ([`ModuleGraph::kept_texts`]): the package that owns it, or the one
    /// whose directory holds it, scans the source files the graph reaches.
    fn keeps_text(&self, module: &Module) -> bool {
        let scans = |id: PackageId| {
            self.inputs
                .setup
                .package(id)
                .is_some_and(|package| package.scans_reached_sources)
        };
        scans(module.owner) || scans(self.located_owner(&module.abs))
    }

    /// Whether the graph may keep the text of the file `abs`, a path the links already resolve, before it is known
    /// which package owns it: [`Self::keeps_text`] for the package of its location, or for any package when another
    /// may adopt it ([`Self::adopter`]).
    fn may_keep_text(&self, abs: &Path) -> bool {
        let setup = self.inputs.setup;
        let scans = |id: PackageId| setup.package(id).is_some_and(|package| package.scans_reached_sources);
        scans(self.located_owner(abs)) || (self.adoptable(abs) && setup.scans_reached_sources())
    }

    /// The id of `abs`, creating an unparsed module on first sight. The flag is true for a new module.
    fn intern(&mut self, abs: &Path) -> (ModuleId, bool) {
        self.intern_owned(abs, None)
    }

    /// [`Self::intern`] for a file that the package of `adoption` owns, when given, instead of the package of its
    /// location. A module seen before keeps its owner. An adopted file keeps the `ignore` globs of the package of its
    /// location too.
    fn intern_owned(&mut self, abs: &Path, adoption: Option<Adoption>) -> (ModuleId, bool) {
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
        let located = self.located_owner(abs);
        let owner = adoption.as_ref().map_or(located, |adoption| adoption.owner);
        if let Some(adoption) = adoption {
            self.adopted.insert(id, adoption.root_dir);
        }
        let rel = relative(abs, inputs.root).unwrap_or_else(|| abs.to_string_lossy().into_owned());
        let setup = inputs.setup.package(owner);
        let package_rel = setup.and_then(|setup| relative(abs, &setup.dir));
        let foreign = candidate.is_none() && (!abs.starts_with(inputs.root) || self.in_other_project(abs, owner));
        let ignored_by = |package: PackageId| {
            inputs
                .setup
                .package(package)
                .is_some_and(|setup| relative(abs, &setup.dir).is_some_and(|rel| setup.ignore.matches_any(&rel)))
        };
        let ignored = foreign
            || inputs.setup.global_ignore.matches_any(&rel)
            || ignored_by(owner)
            || (owner != located && ignored_by(located));
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
            is_build_source: false,
            info: ModuleInfo::default(),
            parse_error: None,
            imports: Vec::new(),
        });
        self.index.insert(abs.to_path_buf(), id);
        (id, true)
    }

    /// The target of an import, interning the files it reaches; the package of `adoption` owns a file reached for the
    /// first time ([`Self::adopter`]).
    fn lower(&mut self, target: RawTarget, adoption: Option<Adoption>, next: &mut Vec<ModuleId>) -> Target {
        match target {
            RawTarget::File(path) => {
                let (id, fresh) = self.intern_owned(&path, adoption);
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
            inputs
                .setup
                .package(owner)
                .is_none_or(|package| components::may_be_used_untagged(inputs.fs, package, inputs.candidates))
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
    /// import (ADR 0020), and returns the modules this reaches for the first time. Only the modules read since the
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
                let target = self.lower(RawTarget::File(registration.file), None, &mut fresh);
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

/// Builds the graph by following imports from `entries` and `build_sources` in rounds. Each round merges what parsing
/// and resolving its files gave in order, so ids and edges do not depend on scheduling. Unless `sequential`, the
/// files the entries reach are parsed and resolved ahead of the rounds ([`prefetch`]), each as soon as a file
/// importing it is, so a long chain of imports does not wait for the widest round before it. Entries are interned
/// first, so a build source that is also an entry stays an entry, whose exports are public.
///
/// `declarations` are declaration files that are entries only when they apply without being imported
/// ([`ModuleInfo::declares_without_import`]), or when they cannot be read or parsed. They are parsed first; one that
/// is no entry joins the graph only when a module imports it, as any module does, and its parse is used then.
pub(crate) fn build(
    inputs: &Inputs<'_>,
    entries: &[PathBuf],
    build_sources: &[PathBuf],
    declarations: &[PathBuf],
) -> ModuleGraph {
    let mut builder = Builder::new(inputs);
    let mut ahead = parse_ahead(inputs, declarations);
    let mut seeds: Vec<&PathBuf> = entries
        .iter()
        .chain(
            declarations
                .iter()
                .filter(|path| ahead.get(path.as_path()).is_some_and(is_declaration_entry)),
        )
        .collect();
    seeds.sort();
    seeds.dedup();
    let mut frontier = Vec::new();
    for entry in seeds {
        let (id, fresh) = builder.intern(entry);
        builder.modules[id.index()].is_entry = true;
        if fresh {
            frontier.push(id);
        }
    }
    for source in build_sources {
        let (id, fresh) = builder.intern(source);
        if fresh {
            builder.modules[id.index()].is_build_source = true;
            frontier.push(id);
        }
    }
    let mut prefetched = if inputs.sequential {
        HashMap::default()
    } else {
        let start = frontier
            .iter()
            .map(|id| builder.modules[id.index()].abs.clone())
            .collect();
        prefetch(&builder, start, &ahead)
    };
    let mut texts = Vec::new();
    let mut scans = HashMap::default();
    while !frontier.is_empty() {
        let files: Vec<(&Path, bool)> = frontier
            .iter()
            .map(|id| {
                let module = &builder.modules[id.index()];
                (module.abs.as_path(), builder.keeps_text(module))
            })
            .collect();
        let known: Vec<Option<Parsed>> = files
            .iter()
            .map(|&(path, keep_text)| {
                ahead
                    .remove(path)
                    .or_else(|| {
                        prefetched
                            .remove(path)
                            .filter(|parsed| !keep_text || parsed.text.is_some())
                    })
                    .map(|mut parsed| {
                        if !keep_text {
                            parsed.text = None;
                        }
                        parsed
                    })
            })
            .collect();
        let unknown: Vec<(&Path, bool)> = files
            .iter()
            .zip(&known)
            .filter(|(_, known)| known.is_none())
            .map(|(file, _)| *file)
            .collect();
        let mut fresh = map_ordered(&unknown, inputs.sequential, |&(path, keep_text)| {
            process(path, keep_text, inputs)
        })
        .into_iter();
        let parsed: Vec<Parsed> = known
            .into_iter()
            .map(|known| known.unwrap_or_else(|| fresh.next().unwrap_or_default()))
            .collect();
        let mut next = Vec::new();
        for (id, mut parsed) in frontier.iter().zip(parsed) {
            if let Some(scan) = parsed.scan.take() {
                scans.insert(builder.modules[id.index()].abs.clone(), scan);
            }
            if !parsed.components.is_empty() && builder.links_by_tag(*id) {
                for position in &parsed.registrations {
                    parsed.imports[*position].target = RawTarget::Ignored;
                }
                builder.register(std::mem::take(&mut parsed.components));
            }
            let imports = parsed
                .imports
                .into_iter()
                .map(|raw| {
                    let adoption = builder.adopter(*id, &raw);
                    ResolvedImport {
                        import: raw.import,
                        target: builder.lower(raw.target, adoption, &mut next),
                        package: raw.package,
                        type_only: raw.type_only,
                        component: false,
                    }
                })
                .collect();
            if let Some(text) = parsed.text {
                texts.push((*id, text));
            }
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

    ModuleGraph {
        imported_by: importers(&builder.modules),
        modules: builder.modules,
        index: builder.index,
        texts,
        scans: Scans(scans),
    }
}

/// What the graph learns about each of `declarations`, by path, with its text.
fn parse_ahead(inputs: &Inputs<'_>, declarations: &[PathBuf]) -> HashMap<PathBuf, Parsed> {
    let parsed = map_ordered(declarations, inputs.sequential, |path| process(path, true, inputs));
    declarations.iter().cloned().zip(parsed).collect()
}

/// What the graph learns about each file that `start` reaches, by path, with its text where the graph may keep it
/// ([`Builder::may_keep_text`]): the files are worked on, on the current rayon pool, as soon as a file importing them
/// is, and each once. The files in `ahead` are not worked on again, but what they import is followed. An import that
/// registers a component is not followed, as the graph may link the component by tag instead; the files the graph
/// reaches that way, or by any path not found here, are parsed by [`build`] when it reaches them.
fn prefetch(builder: &Builder<'_>, start: Vec<PathBuf>, ahead: &HashMap<PathBuf, Parsed>) -> HashMap<PathBuf, Parsed> {
    let work = Prefetch {
        builder,
        ahead,
        claimed: Mutex::new(start.iter().cloned().collect()),
        done: Mutex::default(),
    };
    rayon::scope(|scope| {
        for path in start {
            let work = &work;
            scope.spawn(move |scope| work.visit(scope, path));
        }
    });
    work.done.into_inner().unwrap_or_else(PoisonError::into_inner)
}

/// The shared state of a [`prefetch`].
struct Prefetch<'p, 'a> {
    builder: &'p Builder<'a>,
    ahead: &'p HashMap<PathBuf, Parsed>,
    /// The files found so far, each worked on by the task that found it first.
    claimed: Mutex<HashSet<PathBuf>>,
    done: Mutex<HashMap<PathBuf, Parsed>>,
}

impl Prefetch<'_, '_> {
    fn visit<'s>(&'s self, scope: &rayon::Scope<'s>, path: PathBuf) {
        let inputs = self.builder.inputs;
        let fresh = if self.ahead.contains_key(&path) {
            None
        } else {
            Some(process(&path, self.builder.may_keep_text(&path), inputs))
        };
        let Some(parsed) = fresh.as_ref().or_else(|| self.ahead.get(&path)) else {
            return;
        };
        let mut found = Vec::new();
        for (position, raw) in parsed.imports.iter().enumerate() {
            if parsed.registrations.contains(&position) {
                continue;
            }
            let targets: &[PathBuf] = match &raw.target {
                RawTarget::File(file) => std::slice::from_ref(file),
                RawTarget::Files(files) => files,
                _ => &[],
            };
            found.extend(
                targets
                    .iter()
                    .map(|target| inputs.links.get(target).unwrap_or(target).clone()),
            );
        }
        {
            let mut claimed = lock(&self.claimed);
            found.retain(|target| claimed.insert(target.clone()));
        }
        for target in found {
            scope.spawn(move |scope| self.visit(scope, target));
        }
        if let Some(parsed) = fresh {
            lock(&self.done).insert(path, parsed);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether a declaration file parsed ahead is an entry: it applies without being imported, or what it holds is not
/// known because it could not be read or parsed.
fn is_declaration_entry(parsed: &Parsed) -> bool {
    parsed.error.is_some() || parsed.info.declares_without_import()
}

/// The modules that import each module, sorted and without duplicates, by position. The modules are visited in id
/// order, so each list grows in order and a repeat can only be its last entry.
fn importers(modules: &[Module]) -> Vec<Vec<ModuleId>> {
    let mut imported_by: Vec<Vec<ModuleId>> = vec![Vec::new(); modules.len()];
    for module in modules {
        for import in &module.imports {
            let targets: &[ModuleId] = match &import.target {
                Target::Module(id) => std::slice::from_ref(id),
                Target::Modules(ids) => ids,
                _ => &[],
            };
            for target in targets {
                let importers = &mut imported_by[target.index()];
                if importers.last() != Some(&module.id) {
                    importers.push(module.id);
                }
            }
        }
    }
    imported_by
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use skif_config::Config;
    use skif_core::MemoryFs;
    use skif_workspace::Workspace;

    use super::*;
    use crate::discover::discover;
    use crate::input::InputSource;

    /// A [`MemoryFs`] that counts the reads of TypeScript files.
    struct SourceReads {
        inner: MemoryFs,
        reads: AtomicUsize,
    }

    impl FileSystem for SourceReads {
        fn read_to_string(&self, path: &Path) -> std::io::Result<Arc<str>> {
            if path.extension().is_some_and(|extension| extension == "ts") {
                self.reads.fetch_add(1, Ordering::SeqCst);
            }
            self.inner.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> std::io::Result<Vec<skif_core::DirEntry>> {
            self.inner.read_dir(path)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.inner.is_file(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.inner.is_dir(path)
        }
    }

    /// A workspace at `/ws` holding `files`, with what building its graph needs.
    struct Fixture {
        fs: Arc<SourceReads>,
        workspace: Workspace,
        setup: Setup,
        candidates: Vec<Candidate>,
    }

    impl Fixture {
        fn new(files: &[(String, String)]) -> Self {
            let root = Path::new("/ws");
            let memory = files
                .iter()
                .fold(MemoryFs::new(), |fs, (path, text)| fs.with_file(root.join(path), text));
            let config = Config::from_toml_str("").unwrap();
            let workspace = Workspace::discover_with_fs(root, &config, &memory).unwrap();
            let candidates = discover(&memory, &workspace, true);
            let fs = Arc::new(SourceReads {
                inner: memory,
                reads: AtomicUsize::new(0),
            });
            let shared: Arc<dyn FileSystem> = Arc::clone(&fs) as Arc<dyn FileSystem>;
            let sources: Vec<Arc<dyn InputSource>> = Vec::new();
            let setup = Setup::new(&workspace, &config, &shared, &sources, true);
            fs.reads.store(0, Ordering::SeqCst);
            Self {
                fs,
                workspace,
                setup,
                candidates,
            }
        }

        fn reads(&self) -> usize {
            self.fs.reads.load(Ordering::SeqCst)
        }

        /// The graph from `entries` (relative to the root), with a resolver of its own as a rebuild has.
        fn graph(&self, entries: &[&str], sequential: bool, earlier: &Scans, keep_scans: bool) -> ModuleGraph {
            let entries: Vec<PathBuf> = entries.iter().map(|entry| self.workspace.root.join(entry)).collect();
            self.with_inputs(sequential, earlier, keep_scans, |inputs| {
                build(inputs, &entries, &[], &[])
            })
        }

        fn with_inputs<T>(
            &self,
            sequential: bool,
            earlier: &Scans,
            keep_scans: bool,
            then: impl FnOnce(&Inputs<'_>) -> T,
        ) -> T {
            let fs: Arc<dyn FileSystem> = Arc::clone(&self.fs) as Arc<dyn FileSystem>;
            let resolver = ModuleResolver::new(Arc::clone(&fs), &self.workspace, &self.setup);
            let links = HashMap::default();
            let inputs = Inputs {
                fs: fs.as_ref(),
                resolver: &resolver,
                root: &self.workspace.root,
                candidates: &self.candidates,
                links: &links,
                setup: &self.setup,
                sequential,
                modules: None,
                earlier,
                keep_scans,
            };
            then(&inputs)
        }
    }

    fn owned(files: &[(&str, &str)]) -> Vec<(String, String)> {
        files
            .iter()
            .map(|(path, text)| ((*path).to_owned(), (*text).to_owned()))
            .collect()
    }

    /// Everything a graph holds about its modules, for comparing two graphs.
    fn shape(graph: &ModuleGraph) -> String {
        let importers: Vec<&[ModuleId]> = graph
            .modules()
            .iter()
            .map(|module| graph.imported_by(module.id))
            .collect();
        format!("{:#?}\n{importers:?}", graph.modules())
    }

    fn paths(graph: &ModuleGraph) -> Vec<String> {
        graph
            .modules()
            .iter()
            .map(|module| module.path.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn a_graph_built_again_reuses_what_reading_and_parsing_gave_and_resolves_the_same() {
        let fixture = Fixture::new(&owned(&[
            ("package.json", r#"{"name":"root","private":true}"#),
            (
                "src/a.ts",
                "import { b } from './b';\nimport { c } from './c';\nexport const a = b + c;\n",
            ),
            ("src/b.ts", "export const b = 1;\n"),
            ("src/c.ts", "import { d } from './d';\nexport const c = d;\n"),
            ("src/d.ts", "export const d = 2;\n"),
            ("src/unused.ts", "export const u = 3;\n"),
        ]));
        let first = fixture.graph(&["src/a.ts"], true, &Scans::default(), true);
        assert_eq!(paths(&first), ["src/a.ts", "src/b.ts", "src/c.ts", "src/d.ts"]);
        assert_eq!(fixture.reads(), 4);

        let again = fixture.graph(&["src/a.ts"], true, &first.clone().into_scans(), true);
        assert_eq!(fixture.reads(), 4, "the graph built again read a file");
        assert_eq!(shape(&again), shape(&first));

        // A graph that keeps no scans gives none to the next, which reads every file again.
        let unkept = fixture.graph(&["src/a.ts"], true, &Scans::default(), false);
        assert_eq!(fixture.reads(), 8);
        let fresh = fixture.graph(&["src/a.ts"], true, &unkept.into_scans(), false);
        assert_eq!(fixture.reads(), 12);
        assert_eq!(shape(&fresh), shape(&first));
    }

    #[test]
    fn a_graph_built_again_from_more_entries_reads_only_the_new_files() {
        let fixture = Fixture::new(&owned(&[
            ("package.json", r#"{"name":"root","private":true}"#),
            ("src/a.ts", "import { b } from './b';\nexport const a = b;\n"),
            ("src/b.ts", "export const b = 1;\n"),
            (
                "scripts/run.ts",
                "import { b } from '../src/b';\nimport { e } from './e';\nconsole.log(b, e);\n",
            ),
            ("scripts/e.ts", "export const e = 2;\n"),
        ]));
        let first = fixture.graph(&["src/a.ts"], true, &Scans::default(), true);
        assert_eq!(fixture.reads(), 2);
        let wider = fixture.graph(&["scripts/run.ts", "src/a.ts"], true, &first.into_scans(), true);
        assert_eq!(fixture.reads(), 4, "only scripts/run.ts and scripts/e.ts are new");
        let cold = fixture.graph(&["scripts/run.ts", "src/a.ts"], true, &Scans::default(), false);
        assert_eq!(shape(&wider), shape(&cold));
    }

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

    fn module(index: usize, targets: Vec<Target>) -> Module {
        Module {
            id: ModuleId::from_index(index),
            abs: PathBuf::from(format!("/ws/{index}.ts")),
            path: PathBuf::from(format!("{index}.ts")),
            owner: PackageId::ROOT,
            in_project: true,
            ignored: false,
            foreign: false,
            is_entry: false,
            is_build_source: false,
            info: ModuleInfo::default(),
            parse_error: None,
            imports: targets
                .into_iter()
                .enumerate()
                .map(|(import, target)| ResolvedImport {
                    import,
                    target,
                    package: None,
                    type_only: false,
                    component: false,
                })
                .collect(),
        }
    }

    #[test]
    fn importers_are_sorted_and_listed_once_however_often_a_module_imports() {
        let id = ModuleId::from_index;
        let modules = vec![
            module(0, vec![Target::Module(id(3))]),
            module(1, vec![Target::Module(id(0)), Target::External("react".to_owned())]),
            module(
                2,
                vec![
                    Target::Module(id(0)),
                    Target::Modules(vec![id(0), id(1)]),
                    Target::Module(id(2)),
                    Target::Unresolved,
                ],
            ),
            module(3, vec![Target::Module(id(0)), Target::Module(id(0))]),
        ];
        assert_eq!(
            importers(&modules),
            vec![vec![id(1), id(2), id(3)], vec![id(2)], vec![id(2)], vec![id(0)]]
        );
    }

    /// A chain of `depth` files, `src/c0.ts` importing `src/c1.ts` and so on; every tenth also imports a side file,
    /// which imports the file five links further down the chain, reached again later.
    fn chain(depth: usize) -> Vec<(String, String)> {
        let mut files = owned(&[("package.json", r#"{"name":"root","private":true}"#)]);
        for index in 0..depth {
            let mut text = String::new();
            if index + 1 < depth {
                text.push_str(&format!("import {{ v{next} }} from './c{next}';\n", next = index + 1));
            }
            if index % 10 == 0 {
                text.push_str(&format!("import './side{index}';\n"));
                let further = (index + 5).min(depth - 1);
                files.push((format!("src/side{index}.ts"), format!("import './c{further}';\n")));
            }
            text.push_str(&format!("export const v{index} = 1;\n"));
            files.push((format!("src/c{index}.ts"), text));
        }
        files
    }

    #[test]
    fn a_deep_chain_of_imports_parsed_ahead_gets_the_ids_and_edges_of_a_round_by_round_build() {
        const DEPTH: usize = 300;
        let fixture = Fixture::new(&chain(DEPTH));
        let sequential = fixture.graph(&["src/c0.ts"], true, &Scans::default(), false);
        assert_eq!(fixture.reads(), DEPTH + DEPTH / 10);
        assert_eq!(
            &paths(&sequential)[..5],
            ["src/c0.ts", "src/c1.ts", "src/side0.ts", "src/c2.ts", "src/c5.ts"]
        );

        let parallel = fixture.graph(&["src/c0.ts"], false, &Scans::default(), false);
        assert_eq!(shape(&parallel), shape(&sequential));
        // Each file is read once, also the files two importers reach at once.
        assert_eq!(fixture.reads(), 2 * (DEPTH + DEPTH / 10));
    }

    #[test]
    fn prefetching_reaches_the_end_of_a_deep_chain_and_reads_each_file_once() {
        const DEPTH: usize = 300;
        let fixture = Fixture::new(&chain(DEPTH));
        let found = fixture.with_inputs(false, &Scans::default(), false, |inputs| {
            let builder = Builder::new(inputs);
            prefetch(&builder, vec![PathBuf::from("/ws/src/c0.ts")], &HashMap::default())
        });
        assert_eq!(found.len(), DEPTH + DEPTH / 10);
        assert_eq!(fixture.reads(), DEPTH + DEPTH / 10);
        let last = Path::new("/ws/src").join(format!("c{}.ts", DEPTH - 1));
        assert!(found.get(&last).is_some_and(|parsed| parsed.error.is_none()));
    }
}
