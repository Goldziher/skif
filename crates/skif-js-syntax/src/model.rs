//! Skif-owned description of one module. No oxc type appears here.

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
    /// `require("x")`, `require.resolve("x")` and `import x = require("x")`.
    Require,
    /// One pattern of `import.meta.glob(...)` (a glob, not a module specifier).
    ImportMetaGlob,
    /// `export … from "x"`; the matching [`Export`] is recorded too.
    ExportFrom,
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
    /// JSDoc tag names (without `@`) on the member.
    pub tags: Vec<String>,
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
    /// One specifier with its separating comma. Ranges of adjacent specifiers of one statement overlap; the union of
    /// the ranges is the correct deletion.
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
    /// JSDoc tag names (without `@`) in the comment directly above, such as `public`, `internal`, `alias`.
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
    /// `module.exports = …`, `module.exports.x = …` or `exports.x = …`.
    pub has_cjs_exports: bool,
    /// The file contains JSX.
    pub has_jsx: bool,
    /// Value of a `@jsxImportSource` pragma.
    pub jsx_import_source: Option<String>,
    /// Directive prologue strings such as `use client`.
    pub directives: Vec<String>,
    /// `import()`, `require()` and `import.meta.glob()` calls whose argument is not static.
    pub unresolved_dynamic: Vec<Span>,
}

impl ModuleInfo {
    /// Removal edits of every export that has one, in source order.
    pub fn removal_ranges(&self) -> impl Iterator<Item = &Removal> {
        self.exports.iter().filter_map(|e| e.removal.as_ref())
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
        }
        self.directives.extend(other.directives);
        self.unresolved_dynamic.extend(other.unresolved_dynamic);
    }
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
