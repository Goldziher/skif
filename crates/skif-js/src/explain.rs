//! Why a file, or one export of it, counts as used: the entry rule, the importers and the chain from an entry.
//!
//! [`JsAnalysis::explain_usage`] builds a [`UsageReport`]; the verdict of an export and the evidence listed with it are
//! computed together, and [`UsageReport::render`] is the only place that turns the report into text.

use std::collections::VecDeque;

use skif_core::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

use serde::Serialize;
use skif_js_syntax::{ExportKind, Import, ImportKind, ImportedKind, ImportedName, ModuleInfo, is_declaration_file};
use skif_workspace::PackageId;

use crate::analysis::JsAnalysis;
use crate::discover::relative;
use crate::globs::Globs;
use crate::graph::{Module, ModuleId, ResolvedImport, Target};
use crate::input::IncompleteEntries;
use crate::manifest_paths::{crossing_stars, manifest_targets, source_candidates, source_patterns};
use crate::resolve::join_normalized;
use crate::setup::PackageSetup;
use crate::trace::dir_of;
use crate::usage;

/// Re-export hops followed when explaining an export.
const MAX_DEPTH: usize = 8;
/// Importers listed by [`UsageReport::render`] before the rest is summarised.
const MAX_LISTED: usize = 12;

/// Which rule makes a module an entry file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum EntryReason {
    /// An entry glob of its package: defaults, configuration, test conventions or a plugin.
    EntryGlob,
    /// A configuration file of a tool.
    ConfigFile,
    /// Run by a script of `package.json`.
    Script,
    /// Published by `package.json` (`main`, `module`, `types`, `bin`, `exports`, n8n `nodes` and `credentials`).
    Published,
    /// Loaded by a `bin` file of `package.json` that is no module (an extensionless `#!/usr/bin/env node` stub).
    BinStub,
    /// Matched by an entry glob of another package that points outside that package.
    ExternalEntryGlob,
    /// Matched by an entry glob a plugin gives a package that contains this one, such as a root
    /// `playwright.config.ts` naming the tests of a member.
    EnclosingEntryGlob,
    /// In the narrowed scope of a package whose entries are incomplete: a config the analysis cannot fully read may
    /// make it an entry, so it is treated as one (ADR 0016).
    IncompleteScope,
    /// In the narrowed scope of an incomplete package that contains its package, such as a root test config whose
    /// globs reach the files of the members below it (ADR 0016).
    EnclosingIncompleteScope,
    /// The module is an entry, but none of the known rules accounts for it.
    Unknown,
}

impl EntryReason {
    const fn describe(self) -> &'static str {
        match self {
            Self::EntryGlob => {
                "matches an entry glob of its package (defaults, configuration, test conventions or a plugin)"
            }
            Self::ConfigFile => "a configuration file of a tool",
            Self::Script => "run by a script of package.json",
            Self::Published => {
                "published by package.json (main, module, types, bin, exports, n8n nodes and credentials)"
            }
            Self::BinStub => "loaded by a bin file of package.json that is no module (an extensionless node stub)",
            Self::ExternalEntryGlob => "matches an entry glob of another package that points outside it",
            Self::EnclosingEntryGlob => "matches an entry glob a plugin gives a package that contains its package",
            Self::IncompleteScope => {
                "may be an entry of a configuration that is not fully known, so it is treated as one (narrowed scope)"
            }
            Self::EnclosingIncompleteScope => {
                "may be an entry of a configuration of a package that contains its package and is not fully known, so \
                 it is treated as one (narrowed scope)"
            }
            Self::Unknown => "unknown: no entry rule of the analysis accounts for it",
        }
    }
}

/// Why a file is not in the module graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Absence {
    /// No entry file reaches it; `unused-files` reports it.
    UnusedFile,
    /// Its package is excluded from the run (`ignore-packages`, or `[js]` turned off for it).
    PackageExcluded,
    /// A declaration file (`.d.ts`, `.d.mts`, `.d.cts`), which is never reported as unused.
    DeclarationFile,
    /// The entry files of its package are not fully known and may be anywhere in it, so none of its files is judged
    /// (package-wide); [`UsageReport::incomplete`] says why.
    EntriesIncomplete,
    /// Outside the project globs of its package.
    OutsideProject,
    /// Matched by an `ignore` glob.
    Ignored,
    /// Not a source file the analysis looks at: missing, ignored by Git, of a type that is not analysed, or owned by
    /// no package.
    NotDiscovered,
}

/// One import of a module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct Importer {
    /// The importing file, relative to the workspace root.
    pub file: String,
    /// 1-based line of the import; 0 for an import with no location, that of a registered component a template of the
    /// file uses (ADR 0020).
    pub line: u32,
    /// How the import is written.
    pub kind: ImportKind,
    /// What the import takes; empty for the whole module, and for a side-effect import, a test-runner mock or a resolve
    /// call, which take none of it.
    pub names: Vec<String>,
    /// Whether an entry file reaches the importing file.
    pub reachable: bool,
}

/// A module that imports an export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct ExportUser {
    /// The importing file, relative to the workspace root.
    pub file: String,
    /// 1-based line of the import; 0 for an import with no location, that of a registered component a template of the
    /// file uses (ADR 0020).
    pub line: u32,
    /// What the import takes.
    pub how: String,
    /// Whether an entry file reaches the importing file.
    pub reachable: bool,
}

/// Whether an export counts as used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Verdict {
    /// The module has no such export.
    NotAnExport,
    /// Public by definition: the file is an entry, or the export is tagged.
    Public,
    /// Another module uses it, or a used export of the same file mentions it in its signature.
    Used,
    /// Nothing uses it.
    Unused,
    /// Nothing in the analysed graph uses it, but the entry files of its package are not fully known, so a file no
    /// known entry reaches may use it; it is not reported.
    NotJudged,
}

/// A re-export that forwards an export, with the explanation of the forwarded export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
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
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent facts of one serialized report"
)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
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
    /// The class behind the export is registered as a custom element (ADR 0020), so the browser uses it.
    #[serde(skip_serializing_if = "is_false")]
    pub registered: bool,
    /// The entry files of the package are not fully known and the file may be one of the unknown ones (the package
    /// is unjudged as a whole, or the file lies in its narrowed scope), so its exports are not judged.
    #[serde(skip_serializing_if = "is_false")]
    pub entries_incomplete: bool,
    /// The same export was already explained higher up (a circular `export *`); not repeated.
    #[serde(skip_serializing_if = "is_false")]
    pub already_explained: bool,
}

/// The explanation [`JsAnalysis::explain_usage`] returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct UsageReport {
    /// The file, relative to the workspace root.
    pub file: String,
    /// Whether the file is in the module graph.
    pub in_graph: bool,
    /// Why it is not, when it is not.
    pub absence: Option<Absence>,
    /// Why the entry files of its package are not fully known, when the absence is
    /// [`EntriesIncomplete`](Absence::EntriesIncomplete): every unreadable config, with files relative to the workspace
    /// root.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub incomplete: Vec<IncompleteEntries>,
    /// When the entry files of its package are not fully known but can only be files matching these globs (relative
    /// to the package directory): the narrowed scope of ADR 0016. Files in it are treated as entries and every other
    /// file is judged; [`UsageReport::incomplete`] then lists the reasons too. Empty when the package's entries are
    /// known, or unknown package-wide.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub incomplete_scope: Vec<String>,
    /// When the entry is [`EnclosingIncompleteScope`](EntryReason::EnclosingIncompleteScope): the directory of the
    /// enclosing package (relative to the workspace root, `.` for the root) whose narrowed scope
    /// [`UsageReport::incomplete_scope`] is, and whose reasons [`UsageReport::incomplete`] lists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope_package: Option<String>,
    /// When the package's configs gave a narrowed scope but a module of the package imports a computed path, which
    /// may load any of its files: that module and the 1-based line, as `file:line` relative to the workspace root.
    /// The package is then unjudged package-wide.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub widened_by: Option<String>,
    /// Which rule makes it an entry file, when it is one.
    pub entry: Option<EntryReason>,
    /// The file is a build source that is no entry, a file a plugin names as one a tool builds from (the `srcs` and
    /// `data` of a Bazel rule): used, and the graph starts from it, but its exports are judged.
    #[serde(skip_serializing_if = "is_false")]
    pub build_source: bool,
    /// Every import of the module.
    pub imported_by: Vec<Importer>,
    /// The shortest chain of imports from an entry file or a build source to the module, that file first.
    pub chain_from_entry: Option<Vec<String>>,
    /// The chain of [`UsageReport::chain_from_entry`] starts at a build source, not at an entry.
    #[serde(skip_serializing_if = "is_false")]
    pub chain_from_build_source: bool,
    /// The explanation of the requested export.
    pub export: Option<ExportReport>,
}

impl UsageReport {
    /// Why the file is not in the module graph, as text.
    fn render_absence(&self, absence: Absence, out: &mut String) {
        match absence {
            Absence::UnusedFile => {
                let _ = writeln!(out, "  no entry file reaches it: reported by `unused-files`");
                if !self.incomplete_scope.is_empty() {
                    let _ = writeln!(
                        out,
                        "  the entry files of its package are not fully known, but the unknown ones can only match {} \
                         (narrowed scope); this file does not, so it is judged",
                        self.incomplete_scope.join(", ")
                    );
                    self.render_incomplete(out);
                }
            }
            Absence::PackageExcluded => {
                let _ = writeln!(out, "  not analysed: its package is excluded from the run");
            }
            Absence::DeclarationFile => {
                let _ = writeln!(
                    out,
                    "  not in the module graph: a declaration file is never reported as unused"
                );
            }
            Absence::EntriesIncomplete => {
                let _ = writeln!(
                    out,
                    "  not in the module graph, and not reported as unused: the entry files of its package are \
                     not fully known and may be anywhere in it (package-wide)"
                );
                if let Some(at) = &self.widened_by {
                    let _ = writeln!(
                        out,
                        "  {at} imports a computed path, which may load any file of the package, so the narrowed \
                         scope of its configs is not used"
                    );
                }
                self.render_incomplete(out);
            }
            Absence::OutsideProject => {
                let _ = writeln!(out, "  not analysed: outside the project globs of its package");
            }
            Absence::Ignored => {
                let _ = writeln!(out, "  not analysed: matched by an `ignore` glob");
            }
            Absence::NotDiscovered => {
                let _ = writeln!(
                    out,
                    "  not analysed: missing, ignored by Git, or not a source file the analysis looks at"
                );
            }
        }
    }

    /// One line per reason the entry files of the package are incomplete.
    fn render_incomplete(&self, out: &mut String) {
        for incomplete in &self.incomplete {
            let source = match (&incomplete.plugin, &incomplete.file) {
                (Some(plugin), Some(file)) => format!("{plugin} reads {}: ", slashed(file)),
                (None, Some(file)) => format!("{}: ", slashed(file)),
                (Some(plugin), None) => format!("{plugin}: "),
                (None, None) => String::new(),
            };
            let _ = writeln!(out, "  {source}{}", incomplete.cause.describe());
        }
    }

    /// The report as text for a terminal.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "{}", self.file);
        if let Some(absence) = self.absence {
            self.render_absence(absence, &mut out);
            return out;
        }
        if let Some(entry) = self.entry {
            let _ = writeln!(out, "  entry file: {}", entry.describe());
            if entry == EntryReason::IncompleteScope {
                let _ = writeln!(
                    out,
                    "  the narrowed scope of its package: {}",
                    self.incomplete_scope.join(", ")
                );
                self.render_incomplete(&mut out);
            }
            if entry == EntryReason::EnclosingIncompleteScope {
                let _ = writeln!(
                    out,
                    "  the narrowed scope of the enclosing package `{}`: {}",
                    self.scope_package.as_deref().unwrap_or("."),
                    self.incomplete_scope.join(", ")
                );
                self.render_incomplete(&mut out);
            }
        }
        if self.build_source {
            let _ = writeln!(
                out,
                "  build source: a plugin names it as a file a tool builds from (such as a Bazel rule's srcs or data), \
                 so it is used, but it is no entry and its exports are judged"
            );
        }
        if self.imported_by.is_empty() {
            let _ = writeln!(out, "  imported by no module");
        } else {
            let _ = writeln!(out, "  imported by {} place(s):", self.imported_by.len());
            for importer in self.imported_by.iter().take(MAX_LISTED) {
                if importer.line == 0 {
                    let _ = writeln!(out, "    {} ({COMPONENT_USE})", importer.file);
                    continue;
                }
                let _ = writeln!(
                    out,
                    "    {}:{} ({}: {})",
                    importer.file,
                    importer.line,
                    kind_label(importer.kind),
                    names_label(importer.kind, &importer.names)
                );
            }
            if self.imported_by.len() > MAX_LISTED {
                let _ = writeln!(out, "    ... and {} more", self.imported_by.len() - MAX_LISTED);
            }
        }
        match &self.chain_from_entry {
            Some(chain) if self.chain_from_build_source => {
                let _ = writeln!(out, "  reached from a build source through: {}", chain.join(" -> "));
            }
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

/// How a template's use of a registered component is described: the import skif adds for it has no location.
const COMPONENT_USE: &str = "a template uses it as a registered component";

/// The line of `import` for a report: 0 for the import of a registered component a template uses, which has no
/// location of its own.
fn import_line(resolved: &ResolvedImport, import: &Import) -> u32 {
    if resolved.component { 0 } else { import.span.start_line }
}

/// The stable name of an import kind, as in the JSON output.
const fn kind_label(kind: ImportKind) -> &'static str {
    match kind {
        ImportKind::Static => "static",
        ImportKind::TypeOnly => "type-only",
        ImportKind::SideEffect => "side-effect",
        ImportKind::Dynamic => "dynamic",
        ImportKind::Require => "require",
        ImportKind::Resolve => "resolve",
        ImportKind::ImportMetaGlob => "import-meta-glob",
        ImportKind::SourceGlob => "source-glob",
        ImportKind::ExportFrom => "export-from",
        ImportKind::Mock => "mock",
        ImportKind::Url => "url",
        _ => "import",
    }
}

/// What an import takes, for the human report. An import that uses no export by itself
/// ([`usage::credits_no_export`]) says what it does instead; a re-export lists the names it forwards.
fn names_label(kind: ImportKind, names: &[String]) -> String {
    if !usage::credits_no_export(kind) {
        return if names.is_empty() {
            "the whole module".to_owned()
        } else {
            names.join(", ")
        };
    }
    match kind {
        ImportKind::Resolve => "its path, no export".to_owned(),
        ImportKind::SideEffect => "runs it, no export".to_owned(),
        ImportKind::Mock => "a test-runner call, no export".to_owned(),
        ImportKind::ExportFrom if !names.is_empty() => names.join(", "),
        _ => "no export".to_owned(),
    }
}

fn render_export(export: &ExportReport, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth + 1);
    if export.verdict == Verdict::NotAnExport {
        let _ = writeln!(out, "{pad}`{}` is not an export of {}", export.name, export.file);
        return;
    }
    if export.already_explained {
        let _ = writeln!(
            out,
            "{pad}`{}` of {} is already explained above",
            export.name, export.file
        );
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
        let unreached = if user.reachable {
            ""
        } else {
            "; no entry file reaches the importer"
        };
        if user.line == 0 {
            let _ = writeln!(out, "{pad}  imported by {} ({COMPONENT_USE}{unreached})", user.file);
            continue;
        }
        let _ = writeln!(
            out,
            "{pad}  imported by {}:{} ({}{unreached})",
            user.file, user.line, user.how
        );
    }
    for re_export in &export.re_exports {
        let _ = writeln!(out, "{pad}  re-exported by {} as `{}`:", re_export.file, re_export.name);
        render_export(&re_export.export, depth + 1, out);
    }
    if export.users.is_empty() && export.re_exports.is_empty() {
        let _ = writeln!(out, "{pad}  no module imports it by name or as a whole");
    }
    if export.in_used_signature {
        let _ = writeln!(
            out,
            "{pad}  mentioned in the signature of a used export of the same file"
        );
    }
    if export.is_re_export {
        let _ = writeln!(out, "{pad}  itself a re-export of another module");
    }
    if export.registered {
        let _ = writeln!(out, "{pad}  registered as a custom element");
    }
    if export.entries_incomplete {
        let _ = writeln!(
            out,
            "{pad}  the entry files of its package are not fully known and this file may be one, so its exports are not judged"
        );
    }
    let verdict = match export.verdict {
        Verdict::Public => "public",
        Verdict::Used => "used by another module",
        Verdict::NotJudged => "not judged: no known module uses it, but its package's entries are incomplete",
        _ => "not used by another module",
    };
    let _ = writeln!(out, "{pad}=> {verdict}");
}

fn describe_name(name: &ImportedName) -> String {
    match &name.kind {
        ImportedKind::Default => "default".to_owned(),
        ImportedKind::Named(export) => export.clone(),
        ImportedKind::Namespace if name.escapes => "the namespace, used as a whole".to_owned(),
        ImportedKind::Namespace if name.accessed_members.is_empty() => "the namespace, no member read".to_owned(),
        ImportedKind::Namespace => format!("the namespace, members {}", name.accessed_members.join(", ")),
    }
}

/// What one name of an import of `kind` takes: an `export * from` forwards every export rather than reading members.
fn describe_imported(kind: ImportKind, name: &ImportedName) -> String {
    let star = kind == ImportKind::ExportFrom
        && name.kind == ImportedKind::Namespace
        && !name.escapes
        && name.accessed_members.is_empty();
    if star {
        "every export, re-exported".to_owned()
    } else {
        describe_name(name)
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

/// Whether `import`, of the module `importer`, uses export `export` of the module it points at, or the module as a
/// whole. A use of the module object as a whole uses no type export (`is_type`), as in [`usage::ExportUsage`].
fn uses_export(importer: &ModuleInfo, import: &Import, export: &str, is_type: bool) -> Option<String> {
    if usage::credits_no_export(import.kind) {
        return None;
    }
    if import.names.is_empty() {
        let reads = !is_type || !usage::loads_module_value(import.kind);
        return reads.then(|| "the whole module".to_owned());
    }
    import.names.iter().find_map(|name| {
        let hit = match &name.kind {
            ImportedKind::Default => export == "default",
            ImportedKind::Named(named) => named == export,
            ImportedKind::Namespace => {
                let whole = name.escapes && (!is_type || usage::exports_binding(importer, name));
                whole || name.accessed_members.iter().any(|member| member == export)
            }
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
            let unused = self.unused_files.iter().find(|unused| unused.path == file);
            let (absence, incomplete, incomplete_scope) = if let Some(unused) = unused {
                let (incomplete, scope) = self.narrowed_scope(unused.owner);
                (Absence::UnusedFile, incomplete, scope)
            } else {
                let (absence, incomplete) = self.absence_of(file);
                (absence, incomplete, Vec::new())
            };
            let widened_by = (absence == Absence::EntriesIncomplete)
                .then(|| self.widened_by(file))
                .flatten();
            return UsageReport {
                file: slashed(file),
                in_graph: false,
                absence: Some(absence),
                incomplete,
                incomplete_scope,
                scope_package: None,
                widened_by,
                entry: None,
                build_source: false,
                imported_by: Vec::new(),
                chain_from_entry: None,
                chain_from_build_source: false,
                export: None,
            };
        };
        let mut walk = Walk {
            reachable: self.reachable_from_entries(),
            seen: HashSet::default(),
        };
        let chain = self.chain_to_entry(module.id);
        let chain_from_build_source = chain
            .as_ref()
            .and_then(|chain| chain.first())
            .is_some_and(|first| self.graph.module(*first).is_build_source);
        let chain = chain.map(|chain| chain.iter().map(|id| slashed(&self.graph.module(*id).path)).collect());
        let entry = module.is_entry.then(|| self.entry_reason(module));
        let scope_owner = match entry {
            Some(EntryReason::IncompleteScope) => Some(module.owner),
            Some(EntryReason::EnclosingIncompleteScope) => self.enclosing_incomplete_scope(module),
            _ => None,
        };
        let (incomplete, incomplete_scope) =
            scope_owner.map_or_else(|| (Vec::new(), Vec::new()), |owner| self.narrowed_scope(owner));
        let scope_package = match entry {
            Some(EntryReason::EnclosingIncompleteScope) => {
                scope_owner.and_then(|owner| self.workspace.package(owner)).map(dir_of)
            }
            _ => None,
        };
        UsageReport {
            file: slashed(&module.path),
            in_graph: true,
            absence: None,
            incomplete,
            incomplete_scope,
            scope_package,
            widened_by: None,
            entry,
            build_source: module.is_build_source,
            imported_by: self.importers(module, &walk),
            chain_from_entry: chain,
            chain_from_build_source,
            export: export.map(|name| self.explain_export(module, name, 0, &mut walk, false)),
        }
    }

    pub(crate) fn reachable_from_entries(&self) -> HashSet<ModuleId> {
        let modules = self.graph.modules();
        let mut queue: VecDeque<ModuleId> = modules
            .iter()
            .filter(|module| module.is_entry || module.is_build_source)
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
            if self.is_script_file(&module.abs) {
                return EntryReason::Script;
            }
            if self.is_published(module, &package.dir, &rel) {
                return EntryReason::Published;
            }
            if let Some(&reason) = self.entry_origins.get(&module.abs) {
                return reason;
            }
            if package.in_incomplete_scope(&rel) {
                return EntryReason::IncompleteScope;
            }
        }
        let enclosing = self.workspace.iter().any(|(id, _)| {
            id != module.owner
                && self.setup.package(id).is_some_and(|other| {
                    relative(&module.abs, &other.dir).is_some_and(|rel| other.shared_entry.is_match(&rel))
                })
        });
        if enclosing {
            return EntryReason::EnclosingEntryGlob;
        }
        let path = slashed(&module.path);
        let external = self.workspace.iter().any(|(id, _)| {
            id != module.owner
                && self
                    .setup
                    .package(id)
                    .is_some_and(|other| other.external_entry.is_match(&path))
        });
        if external {
            return EntryReason::ExternalEntryGlob;
        }
        if self.enclosing_incomplete_scope(module).is_some() {
            return EntryReason::EnclosingIncompleteScope;
        }
        EntryReason::Unknown
    }

    /// The innermost package that contains the package of `module` and whose narrowed incomplete scope matches it, as
    /// `scope_entries` makes such files entries.
    fn enclosing_incomplete_scope(&self, module: &Module) -> Option<PackageId> {
        let root = &self.workspace.root;
        self.workspace
            .iter()
            .filter(|(id, _)| *id != module.owner)
            .filter(|(id, _)| {
                self.setup.package(*id).is_some_and(|other| {
                    relative(&module.abs, &other.dir).is_some_and(|rel| other.in_incomplete_scope(&rel))
                })
            })
            .max_by_key(|(_, package)| root.join(&package.dir).components().count())
            .map(|(id, _)| id)
    }

    /// Whether a script of any package runs `abs`. The paths scripts name are collapsed as the entry files are, so
    /// `./scripts/a.ts` and `../../scripts/a.ts` from another package name the file.
    fn is_script_file(&self, abs: &Path) -> bool {
        self.workspace.iter().any(|(id, _)| {
            self.setup.package(id).is_some_and(|package| {
                package
                    .scripts
                    .files
                    .iter()
                    .any(|file| join_normalized(&package.dir, file) == abs)
            })
        })
    }

    /// Why `file` (relative to the workspace root, or absolute), which is neither in the graph nor reported unused,
    /// is not analysed. The checks follow the `unused-files` filter, with what discovery, the project globs and the
    /// ignores leave out ahead of incomplete entries: those only explain a source file the analysis would judge.
    fn absence_of(&self, file: &Path) -> (Absence, Vec<IncompleteEntries>) {
        let root = &self.workspace.root;
        let abs = if file.is_absolute() {
            file.to_path_buf()
        } else {
            join_normalized(root, &slashed(file))
        };
        let owner = self
            .workspace
            .iter()
            .map(|(id, package)| (id, root.join(&package.dir)))
            .filter(|(_, dir)| abs.starts_with(dir))
            .max_by_key(|(_, dir)| dir.components().count())
            .map(|(id, _)| id);
        let Some(owner) = owner else {
            return (Absence::NotDiscovered, Vec::new());
        };
        let Some(package) = self.setup.package(owner) else {
            return (Absence::PackageExcluded, Vec::new());
        };
        let Some(rel) = relative(&abs, root) else {
            return (Absence::NotDiscovered, Vec::new());
        };
        if !self.outside_graph.contains(&rel) {
            return (Absence::NotDiscovered, Vec::new());
        }
        if is_declaration_file(&rel) {
            return (Absence::DeclarationFile, Vec::new());
        }
        let Some(in_package) = relative(&abs, &package.dir) else {
            return (Absence::OutsideProject, Vec::new());
        };
        if !package.project.is_match(&in_package) {
            return (Absence::OutsideProject, Vec::new());
        }
        if package.ignore.matches_any(&in_package) || self.setup.global_ignore.matches_any(&rel) {
            return (Absence::Ignored, Vec::new());
        }
        if package.unjudged_package_wide() {
            return (Absence::EntriesIncomplete, self.incomplete_reasons(package));
        }
        (Absence::NotDiscovered, Vec::new())
    }

    /// Where the package owning `file` imports a computed path that made its narrowed scope unusable, as `file:line`.
    fn widened_by(&self, file: &Path) -> Option<String> {
        let root = &self.workspace.root;
        let abs = if file.is_absolute() {
            file.to_path_buf()
        } else {
            join_normalized(root, &slashed(file))
        };
        self.workspace
            .iter()
            .filter(|(_, package)| abs.starts_with(root.join(&package.dir)))
            .max_by_key(|(_, package)| root.join(&package.dir).components().count())
            .and_then(|(id, _)| self.setup.package(id))
            .and_then(|package| package.widened_by.as_ref())
            .map(|(path, line)| format!("{}:{line}", slashed(path)))
    }

    /// Why the entries of `package` are incomplete, with files relative to the workspace root.
    fn incomplete_reasons(&self, package: &PackageSetup) -> Vec<IncompleteEntries> {
        let root = &self.workspace.root;
        package
            .incomplete_reasons
            .iter()
            .cloned()
            .map(|mut reason| {
                reason.file = reason
                    .file
                    .map(|path| relative(&path, root).map_or(path, std::path::PathBuf::from));
                reason
            })
            .collect()
    }

    /// The reasons and the narrowed scope of package `owner`, when its entries are incomplete within a scope; empty
    /// otherwise.
    fn narrowed_scope(&self, owner: PackageId) -> (Vec<IncompleteEntries>, Vec<String>) {
        match self.setup.package(owner) {
            Some(package) if package.entries_incomplete => package.incomplete_scope.as_ref().map_or_else(
                || (Vec::new(), Vec::new()),
                |scope| (self.incomplete_reasons(package), scope.patterns.clone()),
            ),
            _ => (Vec::new(), Vec::new()),
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
                        .map(|pattern| {
                            if target.from_exports {
                                crossing_stars(&pattern)
                            } else {
                                pattern
                            }
                        }),
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
                        line: import_line(resolved, import),
                        kind: import.kind,
                        names: import
                            .names
                            .iter()
                            .map(|name| describe_imported(import.kind, name))
                            .collect(),
                        reachable: walk.reachable.contains(importer_id),
                    });
                }
            }
        }
        found
    }

    /// The shortest chain of imports from an entry file or a build source to `id`, that file first.
    fn chain_to_entry(&self, id: ModuleId) -> Option<Vec<ModuleId>> {
        let modules = self.graph.modules();
        let mut previous: HashMap<ModuleId, ModuleId> = HashMap::default();
        let mut queue: VecDeque<ModuleId> = modules
            .iter()
            .filter(|module| module.is_entry || module.is_build_source)
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

    /// Why export `name` of `module` counts as used. With `through_star`, `module` forwards the name with an
    /// `export * from`, which declares no export of that name, and the name is followed through its users and
    /// re-exports all the same.
    fn explain_export(
        &self,
        module: &Module,
        name: &str,
        depth: usize,
        walk: &mut Walk,
        through_star: bool,
    ) -> ExportReport {
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
            registered: false,
            entries_incomplete: false,
            already_explained: false,
        };
        let export = module.info.exports.iter().find(|export| export.name == name);
        if export.is_none() && !through_star {
            return report;
        }
        if !walk.seen.insert((module.id, name.to_owned())) {
            report.already_explained = true;
            report.verdict = Verdict::Unused;
            return report;
        }
        report.entry_public = module.is_entry && !self.setup.includes_entry_exports(module.owner);
        report.tagged_public = export.is_some_and(|export| self.is_tagged_public(export));
        report.is_re_export =
            export.is_none_or(|export| matches!(export.kind, ExportKind::ReExport | ExportKind::StarReExport));
        report.entries_incomplete = !self.judges_exports_of(module);
        let is_type = export.is_some_and(|export| export.is_type);
        for importer_id in self.graph.imported_by(module.id) {
            let importer = self.graph.module(*importer_id);
            for resolved in &importer.imports {
                if !reached(&resolved.target).contains(&module.id) {
                    continue;
                }
                let Some(import) = importer.info.imports.get(resolved.import) else {
                    continue;
                };
                if let Some(how) = uses_export(&importer.info, import, name, is_type) {
                    report.users.push(ExportUser {
                        file: slashed(&importer.path),
                        line: import_line(resolved, import),
                        how,
                        reachable: walk.reachable.contains(importer_id),
                    });
                }
            }
            if depth < MAX_DEPTH {
                self.explain_reexports(importer, module, name, depth, walk, &mut report.re_exports);
            }
        }
        report.registered = export.is_some_and(|export| self.setup.is_registered(module.owner, export));
        let forwarded_use = export.is_none()
            && (!report.users.is_empty()
                || report
                    .re_exports
                    .iter()
                    .any(|re_export| matches!(re_export.export.verdict, Verdict::Used | Verdict::Public)));
        let used = report.registered || forwarded_use || self.exports.is_used(module.id, name);
        report.in_used_signature = !used && export.is_some_and(|export| self.in_used_signature(module, export));
        report.verdict = if report.entry_public || report.tagged_public {
            Verdict::Public
        } else if used || report.in_used_signature {
            Verdict::Used
        } else if report.entries_incomplete {
            Verdict::NotJudged
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
                    name != "default"
                        && !importer
                            .info
                            .exports
                            .iter()
                            .any(|own| own.name == name && own.from.is_none())
                }
                ExportKind::StarReExport => true,
                _ => false,
            };
            if !forwarded {
                continue;
            }
            let through_star = export.kind == ExportKind::StarReExport && export.name == "*";
            let exposed = if through_star { name } else { export.name.as_str() };
            out.push(ReExport {
                file: slashed(&importer.path),
                name: exposed.to_owned(),
                export: self.explain_export(importer, exposed, depth + 1, walk, through_star),
            });
        }
    }
}
