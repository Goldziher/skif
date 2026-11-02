//! The module graph: a worklist from the entry files over resolved imports.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use globset::GlobBuilder;
use rayon::prelude::*;
use skif_core::FileSystem;
use skif_js_syntax::{ImportKind, ModuleInfo, ParseOptions, is_stylesheet, parse_module, parse_stylesheet};
use skif_workspace::PackageId;

use crate::discover::{Candidate, relative};
use crate::resolve::{ModuleResolver, Resolved};
use crate::setup::Setup;
use crate::source_scan::{add_textual_imports, is_template_file, mark_template_usage};

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
    /// Matches an `ignore` glob: issues in this file are not reported.
    pub ignored: bool,
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
    pub setup: &'a Setup,
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

fn process(path: &Path, inputs: &Inputs<'_>) -> Parsed {
    let failed = |message: String| Parsed {
        error: Some(message),
        ..Parsed::default()
    };
    let source = match inputs.fs.read_to_string(path) {
        Ok(source) => source,
        Err(error) => return failed(error.to_string()),
    };
    let style = is_stylesheet(path);
    let parsed = if style {
        Ok(parse_stylesheet(path, &source))
    } else {
        parse_module(path, &source, &ParseOptions::default())
    };
    let mut info = match parsed {
        Ok(info) => info,
        Err(error) => return failed(error.to_string()),
    };
    let error = info.fatal.then(|| {
        info.parse_diagnostics
            .first()
            .map_or_else(|| "syntax error".to_owned(), |issue| issue.message.clone())
    });
    if is_template_file(path) {
        mark_template_usage(&mut info, &source);
    }
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
            let outcome = if style {
                inputs.resolver.resolve_style(path, &import.specifier)
            } else {
                inputs.resolver.resolve(path, &import.specifier)
            };
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
        }
    }

    fn owner_of(&self, abs: &Path) -> PackageId {
        self.package_dirs
            .iter()
            .find(|(dir, _)| abs.starts_with(dir))
            .map_or(PackageId::ROOT, |(_, id)| *id)
    }

    /// The id of `abs`, creating an unparsed module on first sight. The flag is true for a new module.
    fn intern(&mut self, abs: &Path) -> (ModuleId, bool) {
        if let Some(id) = self.index.get(abs) {
            return (*id, false);
        }
        let inputs = self.inputs;
        let id = ModuleId::from_index(self.modules.len());
        let candidate = self
            .candidate_index
            .get(abs)
            .map(|position| &inputs.candidates[*position]);
        let owner = candidate.map_or_else(|| self.owner_of(abs), |candidate| candidate.owner);
        let rel = relative(abs, inputs.root).unwrap_or_else(|| abs.to_string_lossy().into_owned());
        let setup = inputs.setup.package(owner);
        let package_rel = setup.and_then(|setup| relative(abs, &setup.dir));
        let ignored = inputs.setup.global_ignore.matches_any(&rel)
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
        let parsed: Vec<Parsed> = paths.par_iter().map(|path| process(path, inputs)).collect();
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
