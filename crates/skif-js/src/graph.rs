//! The module graph: a worklist from the entry files over resolved imports.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use globset::GlobBuilder;
use skif_core::FileSystem;
use skif_core::timing::Tally;
use skif_js_syntax::{ImportKind, ModuleInfo, ParseOptions, is_stylesheet, parse_module, parse_stylesheet};
use skif_workspace::{PackageId, map_ordered};

use crate::discover::{Candidate, is_package_manifest, relative};
use crate::resolve::{ModuleResolver, Resolved};
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
    /// The modules matched by an `import.meta.glob` pattern.
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

/// Files matched by an `import.meta.glob` pattern written in `importer`.
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
    let Ok(glob) = GlobBuilder::new(&joined).literal_separator(true).build() else {
        return Vec::new();
    };
    let matcher = glob.compile_matcher();
    inputs
        .candidates
        .iter()
        .filter(|candidate| candidate.abs != importer && matcher.is_match(&candidate.rel))
        .map(|candidate| candidate.abs.clone())
        .collect()
}

static READ_TIME: Tally = Tally::new();
static PARSE_TIME: Tally = Tally::new();
static RESOLVE_TIME: Tally = Tally::new();

/// Prints the time `process` spent reading, parsing and resolving since the last report (`SKIF_TIMINGS`).
pub(crate) fn report_timings(root: &Path) {
    READ_TIME.report(format_args!("js read {}", root.display()));
    PARSE_TIME.report(format_args!("js parse {}", root.display()));
    RESOLVE_TIME.report(format_args!("js resolve {}", root.display()));
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
    let parsed = PARSE_TIME.time(|| {
        if style {
            Ok(parse_stylesheet(path, &source))
        } else {
            parse_module(path, &source, &ParseOptions::default())
        }
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
    if !style {
        add_textual_imports(&mut info, &source);
    }
    let imports = info
        .imports
        .iter()
        .enumerate()
        .map(|(index, import)| {
            let type_only = import.kind == ImportKind::TypeOnly
                || (!import.names.is_empty() && import.names.iter().all(|name| name.is_type));
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
            let target = match outcome.target {
                Resolved::File(file) => RawTarget::File(file),
                Resolved::Asset => RawTarget::Asset,
                Resolved::External(name) => RawTarget::External(name),
                Resolved::Builtin => RawTarget::Builtin,
                Resolved::Ignored => RawTarget::Ignored,
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
    Parsed { info, error, imports }
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
        if !abs.starts_with(base) {
            return false;
        }
        let between: Vec<&Path> = abs.ancestors().skip(1).take_while(|dir| *dir != base).collect();
        if between
            .iter()
            .any(|dir| dir.file_name().is_some_and(|name| name == "node_modules"))
        {
            return false;
        }
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
        for (id, parsed) in frontier.iter().zip(parsed) {
            let imports = parsed
                .imports
                .into_iter()
                .map(|raw| ResolvedImport {
                    import: raw.import,
                    target: builder.lower(raw.target, &mut next),
                    package: raw.package,
                    type_only: raw.type_only,
                })
                .collect();
            let module = &mut builder.modules[id.index()];
            module.info = parsed.info;
            module.parse_error = parsed.error;
            module.imports = imports;
        }
        frontier = next;
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
