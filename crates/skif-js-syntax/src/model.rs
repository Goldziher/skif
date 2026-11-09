//! Skif-owned description of one module. No oxc type appears here.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use skif_core::Span;

/// How a module is pulled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportKind {
    /// `import a from "x"` (not type-only, has bindings).
    Static,
    /// `import type … from "x"`: the whole declaration is type-only.
    TypeOnly,
    /// `import "x"`.
    SideEffect,
    /// `import("x")` with a literal specifier.
    Dynamic,
    /// `require("x")`, `require.resolve("x")`, `import.meta.resolve("x")` and `import x = require("x")`.
    Require,
    /// One pattern of `import.meta.glob(...)`, or the glob of a computed relative `require` or `import()`
    /// (`./commands/${name}`): a glob, not a module specifier.
    ImportMetaGlob,
    /// One pattern of an `import.meta.glob(...)` whose options read the matched files as text (`query: '?raw'`,
    /// legacy `as: 'raw'`): the files are not loaded as modules, so the glob keeps none of them in use.
    SourceGlob,
    /// `export … from "x"`; the matching [`Export`] is recorded too.
    ExportFrom,
    /// `vi.mock("x")`, `jest.mock("x")` and the other test-runner calls that name a module: the test loads or
    /// replaces the module, which keeps a local file reachable but is not an import of a dependency.
    Mock,
}

/// Which export of the target module a binding refers to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImportedKind {
    /// The default export.
    Default,
    /// The whole namespace (`import * as ns`, `export * from`, `require("x")`, `import("x")`).
    Namespace,
    /// A named export.
    Named(String),
}

/// One binding brought in by an [`Import`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ImportedName {
    /// What is imported.
    pub kind: ImportedKind,
    /// Local binding name; `None` for re-exports, side effects and unbound `require`/`import()`.
    pub local: Option<String>,
    /// `import { type A }` or a type-only declaration.
    pub is_type: bool,
    /// Members read through the local binding in this file (`ns.a`, `Enum.A`, `const { a } = ns`, `ns.A` in types).
    /// Sorted and deduplicated. Meaningful only when `local` is set.
    pub accessed_members: Vec<String>,
    /// The binding is used in a way that cannot be reduced to members (passed around, re-exported, computed access,
    /// destructured with a rest element), or the module value is not bound at all (`import("x")`, bare `require("x")`).
    pub escapes: bool,
}

/// A module reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Import {
    /// The specifier as written (a glob pattern for [`ImportKind::ImportMetaGlob`]).
    pub specifier: String,
    /// How it is referenced.
    pub kind: ImportKind,
    /// Bindings it brings in.
    pub names: Vec<ImportedName>,
    /// The test-runner call of an [`ImportKind::Mock`] as written (`vi.mock`, `jest.requireActual`); `None` for every
    /// other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<String>,
    /// Location of the whole statement or call.
    pub span: Span,
}

/// What an export is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExportKind {
    /// `const`, `let`, `var`, and `export { x }` of anything not recognised below.
    Value,
    /// `interface` and `type` aliases.
    Type,
    /// `enum` and `const enum`.
    Enum,
    /// TypeScript `namespace`/`module`.
    Namespace,
    /// `class`.
    Class,
    /// `function`.
    Function,
    /// `export default …` (named `default`).
    Default,
    /// `export { a as b } from "x"`.
    ReExport,
    /// `export * from "x"` (named `*`) and `export * as ns from "x"` (named `ns`).
    StarReExport,
}

/// A member of an enum or TypeScript namespace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Member {
    /// Member name.
    pub name: String,
    /// Location of the member name.
    pub span: Span,
    /// `JSDoc` tag names (without `@`) on the member.
    pub tags: Vec<String>,
    /// The byte range (absolute, end exclusive) that deletes the member: its line when it stands alone on it, else
    /// the member with its comma and the blanks after it. `None` when deleting it would break the file: another
    /// member mentions its name, or one statement declares it with siblings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removal: Option<(u32, u32)>,
}

/// What a removal edit deletes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RemovalKind {
    /// Only the `export` (or `export default`) keyword and following whitespace: the declaration stays.
    ExportKeyword,
    /// Like [`RemovalKind::ExportKeyword`], but the keyword is shared by several exports of one statement
    /// (`export const a = 1, b = 2`); apply it only when every sibling is unused.
    SharedExportKeyword,
    /// One specifier with its separating comma. Ranges of adjacent specifiers of one statement overlap; use
    /// [`ModuleInfo::removal_edits`] (or [`coalesce_ranges`]) to get disjoint edits.
    Specifier,
    /// The whole statement, including one trailing line break.
    Statement,
}

/// A byte range to delete to fix an unused export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Removal {
    /// Start byte offset.
    pub start: u32,
    /// End byte offset (exclusive).
    pub end: u32,
    /// What the range deletes.
    pub kind: RemovalKind,
    /// Byte range of the whole export statement including one trailing line break.
    pub statement: (u32, u32),
    /// Number of exports declared by that statement (including this one).
    pub siblings: u32,
    /// The statement is `export default <expression>` (or an anonymous default declaration) and
    /// [`RemovalKind::Statement`] deletes the expression too, not only the export. Only an expression whose evaluation
    /// has no effect gets a removal (a literal, an identifier, a function, a class that runs no code, or an array or
    /// object of those); callers that want a conservative fix can still skip or flag these.
    pub default_expression: bool,
}

/// One export of a module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Export {
    /// Exported name: `default`, `*` for `export * from`, otherwise the name importers use.
    pub name: String,
    /// What it is.
    pub kind: ExportKind,
    /// Exists only at type level (`interface`, `type`, `export type { … }`).
    pub is_type: bool,
    /// Local binding behind the export, when it has one.
    pub local_name: Option<String>,
    /// Source module of a re-export.
    pub from: Option<String>,
    /// Name taken from the source module of a re-export (`*` for a star re-export).
    pub imported_name: Option<String>,
    /// `JSDoc` tag names (without `@`) in the comment directly above, such as `public`, `internal`, `alias`.
    pub tags: Vec<String>,
    /// Location of the exported name (the statement for star re-exports).
    pub span: Span,
    /// Members of an enum export.
    pub enum_members: Vec<Member>,
    /// Exported members of a namespace export.
    pub namespace_members: Vec<Member>,
    /// How to remove the export, when a safe edit exists.
    pub removal: Option<Removal>,
    /// The local binding is referenced elsewhere in this file (excluding export clauses).
    pub referenced_in_file: bool,
    /// Names of the other value exports of this file whose signature mentions this export's local binding.
    #[serde(default)]
    pub referenced_in: Vec<String>,
    /// Members of an enum or namespace export read in this file by name (`E.A`, `E['A']`, `const { A } = E`, or a
    /// namespace member's bare name inside the namespace body); sorted.
    #[serde(default)]
    pub members_read_in_file: Vec<String>,
    /// `export const b = a`: the identifier the exported constant is initialised with.
    #[serde(default)]
    pub alias_of: Option<String>,
    /// How the class behind the export is registered as a custom element, in source order. A registered element is
    /// used by the browser through its tag name, not through an import.
    #[serde(default)]
    pub registrations: Vec<Registration>,
}

/// How a class is registered as a custom element (ADR 0062).
///
/// [`Registration::Define`] is unambiguous on its own. The other two only name the binding the registration goes
/// through; whether that binding registers anything depends on the library it comes from, which the caller decides
/// (`lit/decorators.js` registers with `customElement`, `@stencil/core` with `Component`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Registration {
    /// `customElements.define('x-y', Class)`, also through `window.customElements`, a variable holding the registry
    /// (resolved by scope) or a `new CustomElementRegistry()`, and `this` in a static block of the class.
    Define,
    /// A decorator of a top-level class, `@name(…)`, `@name` or `@ns.name(…)`, whose binding is imported from
    /// `module`.
    Decorator {
        /// The specifier the decorator is imported from.
        module: String,
        /// The imported name (the member name for a namespace import).
        name: String,
    },
    /// A top-level class extends `name` imported from `module` (directly or as an argument of a mixin call,
    /// `extends Mixin(Base)`), and `Class.define(…)` or `Class.defineAsync(…)` is called; or `name.define(Class, …)`, the
    /// static form on the imported class, is called with the class first.
    BaseClassDefine {
        /// The specifier the base class is imported from.
        module: String,
        /// The imported name of the base class.
        name: String,
    },
}

/// A syntax problem reported by the parser. The analysis of the file continues on the parser's recovered program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ParseIssue {
    /// The parser's message.
    pub message: String,
    /// Location of the problem in the original file.
    pub span: Span,
}

/// Everything skif needs to know about one source file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ModuleInfo {
    /// Module references, in source order.
    pub imports: Vec<Import>,
    /// Exports, in source order.
    pub exports: Vec<Export>,
    /// Export names referenced inside the same file; sorted, deduplicated.
    pub in_file_references: Vec<String>,
    /// `module.exports = …`, `module.exports.x = …` or `exports.x = …` anywhere in the file. The file is then a
    /// `CommonJS` module whose export names are not tracked: treat all of its exports as used.
    pub has_cjs_exports: bool,
    /// The file contains JSX.
    pub has_jsx: bool,
    /// Value of a `@jsxImportSource` pragma.
    pub jsx_import_source: Option<String>,
    /// Location of the value of the `@jsxImportSource` pragma. Only a pragma in the comments before the first
    /// statement counts.
    pub jsx_import_source_span: Option<Span>,
    /// Directive prologue strings of the top level, such as `use client` and `use server`. See
    /// [`ModuleInfo::is_client_boundary`] and [`ModuleInfo::is_server_actions`].
    pub directives: Vec<String>,
    /// `import()`, `require()` and `import.meta.glob()` calls whose argument is not static.
    pub unresolved_dynamic: Vec<Span>,
    /// Names declared with `declare module 'name' { … }` in a script (a file without top-level `import` or
    /// `export`): the declaration is the module, for every file of the program.
    #[serde(default)]
    pub ambient_modules: Vec<String>,
    /// Names augmented with `declare module 'name' { … }` in a module file: the block adds to a module that has to
    /// exist already and declares nothing on its own.
    #[serde(default)]
    pub augmented_modules: Vec<String>,
    /// Syntax errors the parser recovered from; the other fields describe the recovered program. For component
    /// files, the issues of every script block.
    pub parse_diagnostics: Vec<ParseIssue>,
    /// The parser could not recover (or, for a component file, could not recover in at least one script block): the
    /// facts of the affected code are missing. Report the file as unparseable instead of treating it as empty.
    pub fatal: bool,
}

impl ModuleInfo {
    /// Removal edits of every export that has one, in source order.
    pub fn removal_ranges(&self) -> impl Iterator<Item = &Removal> {
        self.exports.iter().filter_map(|e| e.removal.as_ref())
    }

    /// The file starts with the `"use client"` directive: a React Server Components client boundary whose exports
    /// are consumed by the framework.
    #[must_use]
    pub fn is_client_boundary(&self) -> bool {
        self.directives.iter().any(|d| d == "use client")
    }

    /// The file starts with the `"use server"` directive: every export is a server action the framework calls.
    #[must_use]
    pub fn is_server_actions(&self) -> bool {
        self.directives.iter().any(|d| d == "use server")
    }

    /// Disjoint, sorted deletions that remove every export for which `is_unused` holds.
    ///
    /// Specifier ranges of one statement are merged; a statement whose specifiers are all unused is deleted whole
    /// instead of leaving `export {}`; a [`RemovalKind::SharedExportKeyword`] applies only when every sibling is
    /// unused.
    pub fn removal_edits(&self, is_unused: impl Fn(&Export) -> bool) -> Vec<(u32, u32)> {
        let mut selected: HashMap<(u32, u32), u32> = HashMap::new();
        for export in &self.exports {
            if let Some(removal) = &export.removal
                && is_unused(export)
            {
                *selected.entry(removal.statement).or_default() += 1;
            }
        }
        let mut ranges = Vec::new();
        let mut whole_statements = HashSet::new();
        let mut clauses: HashMap<(u32, u32), Vec<(Span, bool)>> = HashMap::new();
        // Every specifier of a clause, used or not: a run's separator depends on its neighbours.
        for export in &self.exports {
            if let Some(removal) = &export.removal
                && removal.kind == RemovalKind::Specifier
            {
                clauses
                    .entry(removal.statement)
                    .or_default()
                    .push((export.span, is_unused(export)));
            }
        }
        for export in &self.exports {
            let Some(removal) = &export.removal else { continue };
            if !is_unused(export) {
                continue;
            }
            let all_selected = selected.get(&removal.statement).copied().unwrap_or(0) >= removal.siblings;
            match removal.kind {
                RemovalKind::ExportKeyword | RemovalKind::Statement => ranges.push((removal.start, removal.end)),
                RemovalKind::SharedExportKeyword if all_selected => ranges.push((removal.start, removal.end)),
                RemovalKind::Specifier if all_selected => {
                    whole_statements.insert(removal.statement);
                }
                // Partly unused shared declarations stay; partly unused clauses are handled per run below.
                RemovalKind::SharedExportKeyword | RemovalKind::Specifier => {}
            }
        }
        for (statement, specifiers) in &mut clauses {
            if whole_statements.contains(statement) {
                continue;
            }
            specifiers.sort_by_key(|(span, _)| span.start);
            ranges.extend(specifier_runs(specifiers));
        }
        ranges.extend(whole_statements);
        coalesce_ranges(ranges)
    }

    pub(crate) fn merge(&mut self, other: Self) {
        self.imports.extend(other.imports);
        self.exports.extend(other.exports);
        self.in_file_references.extend(other.in_file_references);
        self.in_file_references.sort();
        self.in_file_references.dedup();
        self.has_cjs_exports |= other.has_cjs_exports;
        self.has_jsx |= other.has_jsx;
        if self.jsx_import_source.is_none() {
            self.jsx_import_source = other.jsx_import_source;
            self.jsx_import_source_span = other.jsx_import_source_span;
        }
        self.directives.extend(other.directives);
        self.unresolved_dynamic.extend(other.unresolved_dynamic);
        self.ambient_modules.extend(other.ambient_modules);
        self.augmented_modules.extend(other.augmented_modules);
        self.parse_diagnostics.extend(other.parse_diagnostics);
        self.fatal |= other.fatal;
    }
}

/// Deletions for the unused specifiers of one export clause (in source order, at least one kept): each run of
/// adjacent unused specifiers takes the comma after it, or, at the end of the clause, the comma before it.
fn specifier_runs(specifiers: &[(Span, bool)]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < specifiers.len() {
        if !specifiers[i].1 {
            i += 1;
            continue;
        }
        let first = i;
        while i < specifiers.len() && specifiers[i].1 {
            i += 1;
        }
        let last = i - 1;
        if let Some(next) = specifiers.get(i) {
            out.push((specifiers[first].0.start, next.0.start));
        } else if let Some(before) = first.checked_sub(1).and_then(|k| specifiers.get(k)) {
            out.push((before.0.end, specifiers[last].0.end));
        }
    }
    out
}

/// Merges overlapping and touching byte ranges into sorted, disjoint ones.
#[must_use]
pub fn coalesce_ranges(ranges: impl IntoIterator<Item = (u32, u32)>) -> Vec<(u32, u32)> {
    let mut sorted: Vec<(u32, u32)> = ranges.into_iter().filter(|(s, e)| s < e).collect();
    sorted.sort_unstable();
    let mut out: Vec<(u32, u32)> = Vec::with_capacity(sorted.len());
    for (start, end) in sorted {
        match out.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => out.push((start, end)),
        }
    }
    out
}

/// Whether a query (`?raw`, `raw&inline`, with or without its `?`) makes Vite load a file's text instead of the
/// module, by Vite's own test: a `raw` parameter with no value. `?url` is not one: it names the file to load it some
/// other way (a worklet, a worker), so the file still counts as used.
#[must_use]
pub fn is_source_query(query: &str) -> bool {
    query
        .strip_prefix('?')
        .unwrap_or(query)
        .split('&')
        .any(|parameter| parameter == "raw")
}

/// Options for [`crate::parse_module`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParseOptions {
    /// Resolve references to exports inside the file with semantic analysis.
    pub in_file_references: bool,
    /// Track members accessed through imported bindings.
    pub member_accesses: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            in_file_references: true,
            member_accesses: true,
        }
    }
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ModuleInfo>();
    assert_send_sync::<ParseOptions>();
};
