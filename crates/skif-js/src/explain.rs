//! Why a file, or one export of it, counts as used: the entry rule, the importers and the chain from an entry.
//!
//! [`JsAnalysis::explain_usage`] builds a [`UsageReport`]; the verdict of an export and the evidence listed with it are
//! computed together, and [`UsageReport::render`] is the only place that turns the report into text.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write as _;
use std::path::Path;

use serde::Serialize;
use skif_js_syntax::{ExportKind, Import, ImportKind, ImportedKind, ImportedName};

use crate::analysis::JsAnalysis;
use crate::discover::relative;
use crate::globs::Globs;
use crate::graph::{Module, ModuleId, Target};
use crate::manifest_paths::{crossing_stars, manifest_targets, source_candidates, source_patterns};

/// Re-export hops followed when explaining an export.
const MAX_DEPTH: usize = 8;
/// Importers listed by [`UsageReport::render`] before the rest is summarised.
const MAX_LISTED: usize = 12;

/// Which rule makes a module an entry file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EntryReason {
    /// An entry glob of its package: defaults, configuration, test conventions or a plugin.
    EntryGlob,
    /// A configuration file of a tool.
    ConfigFile,
    /// Run by a script of `package.json`.
    Script,
    /// Published by `package.json` (`main`, `module`, `types`, `bin`, `exports`).
    Published,
    /// Matched by an entry glob of another package that points outside that package.
    ExternalEntryGlob,
    /// The module is an entry, but none of the known rules accounts for it.
    Unknown,
}

impl EntryReason {
    const fn describe(self) -> &'static str {
        match self {
            Self::EntryGlob => "matches an entry glob of its package (defaults, configuration, test conventions or a plugin)",
            Self::ConfigFile => "a configuration file of a tool",
            Self::Script => "run by a script of package.json",
            Self::Published => "published by package.json (main, module, types, bin, exports)",
            Self::ExternalEntryGlob => "matches an entry glob of another package that points outside it",
            Self::Unknown => "unknown: no entry rule of the analysis accounts for it",
        }
    }
}

/// Why a file is not in the module graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Absence {
    /// No entry file reaches it; `unused-files` reports it.
    UnusedFile,
    /// Ignored, outside the project globs, a declaration file, in a package excluded from the run, or in a package whose
    /// entry files are not fully known.
    NotAnalysed,
}

/// One import of a module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Importer {
    /// The importing file, relative to the workspace root.
    pub file: String,
    /// 1-based line of the import.
    pub line: u32,
    /// How the import is written.
    pub kind: ImportKind,
    /// What the import takes; empty for the whole module.
    pub names: Vec<String>,
    /// Whether an entry file reaches the importing file.
    pub reachable: bool,
}

/// A module that imports an export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportUser {
    /// The importing file, relative to the workspace root.
    pub file: String,
    /// 1-based line of the import.
    pub line: u32,
    /// What the import takes.
    pub how: String,
    /// Whether an entry file reaches the importing file.
    pub reachable: bool,
}

/// Whether an export counts as used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    /// The module has no such export.
    NotAnExport,
    /// Public by definition: the file is an entry, or the export is tagged.
    Public,
    /// Another module uses it, or a used export of the same file mentions it in its signature.
    Used,
    /// Nothing uses it.
    Unused,
}

/// A re-export that forwards an export, with the explanation of the forwarded export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReExport {
    /// The re-exporting file.
    pub file: String,
    /// The name it exposes the export under.
    pub name: String,
    /// The explanation of that export in the re-exporting file.
    pub export: ExportReport,
}

/// Why one export of a module counts as used or not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExportReport {
    /// The export name.
    pub name: String,
    /// The file that declares or forwards it.
    pub file: String,
    /// The verdict, derived from the facts below.
    pub verdict: Verdict,
    /// The file is an entry whose exports are public.
    pub entry_public: bool,
    /// The export is tagged `@public` (or another ignored tag).
    pub tagged_public: bool,
    /// Imports that take this export or the module as a whole.
    pub users: Vec<ExportUser>,
    /// Re-exports that forward it.
    pub re_exports: Vec<ReExport>,
    /// A used export of the same file mentions it in its signature.
    pub in_used_signature: bool,
    /// The export is itself a re-export.
    pub is_re_export: bool,
    /// The same export was already explained higher up (a circular `export *`); not repeated.
    #[serde(skip_serializing_if = "is_false")]
    pub already_explained: bool,
}

/// The explanation [`JsAnalysis::explain_usage`] returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UsageReport {
    /// The file, relative to the workspace root.
    pub file: String,
    /// Whether the file is in the module graph.
    pub in_graph: bool,
    /// Why it is not, when it is not.
    pub absence: Option<Absence>,
    /// Which rule makes it an entry file, when it is one.
    pub entry: Option<EntryReason>,
    /// Every import of the module.
    pub imported_by: Vec<Importer>,
    /// The shortest chain of imports from an entry file to the module, entry first.
    pub chain_from_entry: Option<Vec<String>>,
    /// The explanation of the requested export.
    pub export: Option<ExportReport>,
}

impl UsageReport {
    /// The report as text for a terminal.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{}", self.file);
        if let Some(absence) = self.absence {
            match absence {
                Absence::UnusedFile => {
                    let _ = writeln!(out, "  no entry file reaches it: reported by `unused-files`");
                }
                Absence::NotAnalysed => {
                    let _ = writeln!(
                        out,
                        "  not in the module graph, and not reported as unused: it is ignored, outside the project \
                         globs, a declaration file, in a package excluded from the run, or in a package whose entry \
                         files are not fully known"
                    );
                }
            }
            return out;
        }
        if let Some(entry) = self.entry {
            let _ = writeln!(out, "  entry file: {}", entry.describe());
        }
        if self.imported_by.is_empty() {
            let _ = writeln!(out, "  imported by no module");
        } else {
            let _ = writeln!(out, "  imported by {} place(s):", self.imported_by.len());
            for importer in self.imported_by.iter().take(MAX_LISTED) {
                let _ = writeln!(
                    out,
                    "    {}:{} ({}: {})",
                    importer.file,
                    importer.line,
                    kind_label(importer.kind),
                    names_label(&importer.names)
                );
            }
            if self.imported_by.len() > MAX_LISTED {
                let _ = writeln!(out, "    ... and {} more", self.imported_by.len() - MAX_LISTED);
            }
        }
        match &self.chain_from_entry {
            Some(chain) => {
                let _ = writeln!(out, "  reached from an entry through: {}", chain.join(" -> "));
            }
            None => {
                let _ = writeln!(out, "  no chain of imports from an entry file reaches it");
            }
        }
        if let Some(export) = &self.export {
            render_export(export, 0, &mut out);
        }
        out
    }
}

/// The stable name of an import kind, as in the JSON output.
const fn kind_label(kind: ImportKind) -> &'static str {
    match kind {
        ImportKind::Static => "static",
        ImportKind::TypeOnly => "type-only",
        ImportKind::SideEffect => "side-effect",
        ImportKind::Dynamic => "dynamic",
        ImportKind::Require => "require",
        ImportKind::ImportMetaGlob => "import-meta-glob",
        ImportKind::ExportFrom => "export-from",
    }
}

fn names_label(names: &[String]) -> String {
    if names.is_empty() {
        "the whole module".to_owned()
    } else {
        names.join(", ")
    }
}

fn render_export(export: &ExportReport, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth + 1);
    if export.verdict == Verdict::NotAnExport {
        let _ = writeln!(out, "{pad}`{}` is not an export of {}", export.name, export.file);
        return;
    }
    if export.already_explained {
        let _ = writeln!(out, "{pad}`{}` of {} is already explained above", export.name, export.file);
        return;
    }
    let _ = writeln!(out, "{pad}export `{}` of {}:", export.name, export.file);
    if export.entry_public {
        let _ = writeln!(out, "{pad}  the file is an entry, so its exports are public");
    }
    if export.tagged_public {
        let _ = writeln!(out, "{pad}  tagged `@public` (or another ignored tag)");
    }
    for user in &export.users {
        let unreached = if user.reachable { "" } else { "; no entry file reaches the importer" };
        let _ = writeln!(out, "{pad}  imported by {}:{} ({}{unreached})", user.file, user.line, user.how);
    }
    for re_export in &export.re_exports {
        let _ = writeln!(out, "{pad}  re-exported by {} as `{}`:", re_export.file, re_export.name);
        render_export(&re_export.export, depth + 1, out);
    }
    if export.users.is_empty() && export.re_exports.is_empty() {
        let _ = writeln!(out, "{pad}  no module imports it by name or as a whole");
    }
    if export.in_used_signature {
        let _ = writeln!(out, "{pad}  mentioned in the signature of a used export of the same file");
    }
    if export.is_re_export {
        let _ = writeln!(out, "{pad}  a re-export; the lines above and below follow where it leads");
    }
    let verdict = match export.verdict {
        Verdict::Public => "public",
        Verdict::Used => "used by another module",
        _ => "not used by another module",
    };
    let _ = writeln!(out, "{pad}=> {verdict}");
}

fn describe_name(name: &ImportedName) -> String {
    match &name.kind {
        ImportedKind::Default => "default".to_owned(),
        ImportedKind::Named(export) => export.clone(),
        ImportedKind::Namespace if name.escapes => "the namespace, used as a whole".to_owned(),
        ImportedKind::Namespace => format!("the namespace, members {}", name.accessed_members.join(", ")),
    }
}

/// The modules an import reaches.
fn reached(target: &Target) -> &[ModuleId] {
    match target {
        Target::Module(id) => std::slice::from_ref(id),
        Target::Modules(ids) => ids.as_slice(),
        _ => &[],
    }
}

/// Whether `import` uses export `export` of the module it points at, or the module as a whole.
fn uses_export(import: &Import, export: &str) -> Option<String> {
    if matches!(import.kind, ImportKind::ExportFrom | ImportKind::SideEffect) {
        return None;
    }
    if import.names.is_empty() {
        return Some("the whole module".to_owned());
    }
    import.names.iter().find_map(|name| {
        let hit = match &name.kind {
            ImportedKind::Default => export == "default",
            ImportedKind::Named(imported) => imported == export,
            ImportedKind::Namespace => name.escapes || name.accessed_members.iter().any(|member| member == export),
        };
        hit.then(|| describe_name(name))
    })
}

/// What one explanation shares: the modules an entry reaches and the exports already explained.
struct Walk {
    reachable: HashSet<ModuleId>,
    seen: HashSet<(ModuleId, String)>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

impl JsAnalysis<'_> {
    /// A report that says why `file` (relative to the workspace root) is reachable and, with `export`, why that
    /// export of it counts as used or what would have to use it.
    #[must_use]
    pub fn explain_usage(&self, file: &Path, export: Option<&str>) -> UsageReport {
        let Some(module) = self
            .graph
            .modules()
            .iter()
            .find(|module| module.path == file || module.abs == file)
        else {
            let unused = self.unused_files.iter().any(|unused| unused.path == file);
            return UsageReport {
                file: slashed(file),
                in_graph: false,
                absence: Some(if unused { Absence::UnusedFile } else { Absence::NotAnalysed }),
                entry: None,
                imported_by: Vec::new(),
                chain_from_entry: None,
                export: None,
            };
        };
        let mut walk = Walk {
            reachable: self.reachable_from_entries(),
            seen: HashSet::new(),
        };
        let chain = self.chain_to_entry(module.id).map(|chain| {
            chain
                .iter()
                .map(|id| slashed(&self.graph.module(*id).path))
                .collect()
        });
        UsageReport {
            file: slashed(&module.path),
            in_graph: true,
            absence: None,
            entry: module.is_entry.then(|| self.entry_reason(module)),
            imported_by: self.importers(module, &walk),
            chain_from_entry: chain,
            export: export.map(|name| self.explain_export(module, name, 0, &mut walk)),
        }
    }

    fn reachable_from_entries(&self) -> HashSet<ModuleId> {
        let modules = self.graph.modules();
        let mut queue: VecDeque<ModuleId> = modules
            .iter()
            .filter(|module| module.is_entry)
            .map(|module| module.id)
            .collect();
        let mut seen: HashSet<ModuleId> = queue.iter().copied().collect();
        while let Some(current) = queue.pop_front() {
            for resolved in &modules[current.index()].imports {
                for next in reached(&resolved.target) {
                    if seen.insert(*next) {
                        queue.push_back(*next);
                    }
                }
            }
        }
        seen
    }

    /// Which entry rule makes `module` an entry file.
    fn entry_reason(&self, module: &Module) -> EntryReason {
        if let Some(package) = self.setup.package(module.owner)
            && let Some(rel) = relative(&module.abs, &package.dir)
        {
            if package.entry.is_match(&rel) {
                return EntryReason::EntryGlob;
            }
            if package.config_files.contains(&module.abs) {
                return EntryReason::ConfigFile;
            }
            if package.scripts.files.contains(&rel) {
                return EntryReason::Script;
            }
            if self.is_published(module, &package.dir, &rel) {
                return EntryReason::Published;
            }
        }
        let path = slashed(&module.path);
        let external = self.workspace.iter().any(|(id, _)| {
            id != module.owner && self.setup.package(id).is_some_and(|other| other.external_entry.is_match(&path))
        });
        if external {
            EntryReason::ExternalEntryGlob
        } else {
            EntryReason::Unknown
        }
    }

    /// Whether the manifest of the owning package publishes `module` (`rel` is relative to the package directory).
    fn is_published(&self, module: &Module, package_dir: &Path, rel: &str) -> bool {
        let Some(package) = self.workspace.package(module.owner) else {
            return false;
        };
        manifest_targets(package.manifest.value()).into_iter().any(|target| {
            if target.path.contains('*') {
                let globs = Globs::new(
                    source_patterns(&target.path)
                        .into_iter()
                        .chain(std::iter::once(target.path.trim_start_matches("./").to_owned()))
                        .map(|pattern| if target.from_exports { crossing_stars(&pattern) } else { pattern }),
                );
                globs.is_match(rel)
            } else {
                source_candidates(&target.path)
                    .into_iter()
                    .any(|candidate| package_dir.join(candidate) == module.abs)
            }
        })
    }

    fn importers(&self, module: &Module, walk: &Walk) -> Vec<Importer> {
        let mut found = Vec::new();
        for importer_id in self.graph.imported_by(module.id) {
            let importer = self.graph.module(*importer_id);
            for resolved in &importer.imports {
                if !reached(&resolved.target).contains(&module.id) {
                    continue;
                }
                if let Some(import) = importer.info.imports.get(resolved.import) {
                    found.push(Importer {
                        file: slashed(&importer.path),
                        line: import.span.start_line,
                        kind: import.kind,
                        names: import.names.iter().map(describe_name).collect(),
                        reachable: walk.reachable.contains(importer_id),
                    });
                }
            }
        }
        found
    }

    /// The shortest chain of imports from an entry file to `id`, entry first.
    fn chain_to_entry(&self, id: ModuleId) -> Option<Vec<ModuleId>> {
        let modules = self.graph.modules();
        let mut previous: HashMap<ModuleId, ModuleId> = HashMap::new();
        let mut queue: VecDeque<ModuleId> = modules
            .iter()
            .filter(|module| module.is_entry)
            .map(|module| module.id)
            .collect();
        let mut seen: HashSet<ModuleId> = queue.iter().copied().collect();
        while let Some(current) = queue.pop_front() {
            if current == id {
                let mut chain = vec![id];
                let mut at = id;
                while let Some(before) = previous.get(&at) {
                    chain.push(*before);
                    at = *before;
                }
                chain.reverse();
                return Some(chain);
            }
            for resolved in &modules[current.index()].imports {
                for next in reached(&resolved.target) {
                    if seen.insert(*next) {
                        previous.insert(*next, current);
                        queue.push_back(*next);
                    }
                }
            }
        }
        None
    }

    fn explain_export(&self, module: &Module, name: &str, depth: usize, walk: &mut Walk) -> ExportReport {
        let mut report = ExportReport {
            name: name.to_owned(),
            file: slashed(&module.path),
            verdict: Verdict::NotAnExport,
            entry_public: false,
            tagged_public: false,
            users: Vec::new(),
            re_exports: Vec::new(),
            in_used_signature: false,
            is_re_export: false,
            already_explained: false,
        };
        let Some(export) = module.info.exports.iter().find(|export| export.name == name) else {
            return report;
        };
        if !walk.seen.insert((module.id, name.to_owned())) {
            report.already_explained = true;
            report.verdict = Verdict::Unused;
            return report;
        }
        report.entry_public = module.is_entry && !self.setup.include_entry_exports;
        report.tagged_public = self.is_tagged_public(export);
        report.is_re_export = matches!(export.kind, ExportKind::ReExport | ExportKind::StarReExport);
        for importer_id in self.graph.imported_by(module.id) {
            let importer = self.graph.module(*importer_id);
            for resolved in &importer.imports {
                if !reached(&resolved.target).contains(&module.id) {
                    continue;
                }
                let Some(import) = importer.info.imports.get(resolved.import) else {
                    continue;
                };
                if let Some(how) = uses_export(import, name) {
                    report.users.push(ExportUser {
                        file: slashed(&importer.path),
                        line: import.span.start_line,
                        how,
                        reachable: walk.reachable.contains(importer_id),
                    });
                }
            }
            if depth < MAX_DEPTH {
                self.explain_reexports(importer, module, name, depth, walk, &mut report.re_exports);
            }
        }
        let used = self.exports.is_used(module.id, name);
        report.in_used_signature = !used && self.in_used_signature(module, export, &mut HashSet::new());
        report.verdict = if report.entry_public || report.tagged_public {
            Verdict::Public
        } else if used || report.in_used_signature {
            Verdict::Used
        } else {
            Verdict::Unused
        };
        report
    }

    /// Follows the re-exports of `importer` that forward export `name` of `module`.
    fn explain_reexports(
        &self,
        importer: &Module,
        module: &Module,
        name: &str,
        depth: usize,
        walk: &mut Walk,
        out: &mut Vec<ReExport>,
    ) {
        for export in &importer.info.exports {
            let Some(from) = export.from.as_deref() else {
                continue;
            };
            if importer.reexport_source(from) != Some(module.id) {
                continue;
            }
            let forwarded = match export.kind {
                ExportKind::ReExport => export.imported_name.as_deref().unwrap_or(&export.name) == name,
                ExportKind::StarReExport if export.name == "*" => {
                    name != "default" && !importer.info.exports.iter().any(|own| own.name == name && own.from.is_none())
                }
                ExportKind::StarReExport => true,
                _ => false,
            };
            if !forwarded {
                continue;
            }
            let exposed = if export.kind == ExportKind::StarReExport && export.name == "*" {
                name
            } else {
                export.name.as_str()
            };
            out.push(ReExport {
                file: slashed(&importer.path),
                name: exposed.to_owned(),
                export: self.explain_export(importer, exposed, depth + 1, walk),
            });
        }
    }
}
