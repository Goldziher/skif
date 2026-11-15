//! Skif-owned description of one module. No oxc type appears here.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use skif_core::Span;
use skif_core::{HashMap, HashSet};

/// How a module is pulled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum ImportKind {
    /// `import a from "x"` (not type-only, has bindings).
    Static,
    /// `import type … from "x"`: the whole declaration is type-only. Also `export * from "x"` and `export { a } from
    /// "x"` inside a `declare module` block, which declares types only.
    TypeOnly,
    /// `import "x"`.
    SideEffect,
    /// `import("x")` with a literal specifier.
    Dynamic,
    /// `require("x")` and `import x = require("x")`.
    Require,
    /// `require.resolve("x")`, `createRequire(…).resolve("x")` and `import.meta.resolve("x")`: the call returns the
    /// module's path and loads nothing, so it keeps a local file reachable and uses the package it names, but uses no
    /// export.
    Resolve,
    /// One pattern of `import.meta.glob(...)`, or the glob of a computed relative `require`, `import()` or resolve call
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
    /// `new URL("./x", import.meta.url)` and `new Worker(path.join(__dirname, "x.js"))`, which the parser does not
    /// report: the file is loaded as a program of its own (a worker, an asset), so every export of it counts as used,
    /// as for an entry. Also a stylesheet `url()` outside
    /// an import that names a file of a package (`url("@scope/pkg/a.woff2")`, `url(~pkg/a.svg)`).
    Url,
}

/// A test-runner call that names a module, recorded on an [`ImportKind::Mock`] import. It serializes as the call is
/// written (`"vi.mock"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum MockCall {
    /// `vi.mock("x")`.
    #[serde(rename = "vi.mock")]
    ViMock,
    /// `vi.doMock("x")`.
    #[serde(rename = "vi.doMock")]
    ViDoMock,
    /// `vi.unmock("x")`.
    #[serde(rename = "vi.unmock")]
    ViUnmock,
    /// `vi.doUnmock("x")`.
    #[serde(rename = "vi.doUnmock")]
    ViDoUnmock,
    /// `vi.importActual("x")`.
    #[serde(rename = "vi.importActual")]
    ViImportActual,
    /// `vi.importMock("x")`.
    #[serde(rename = "vi.importMock")]
    ViImportMock,
    /// `jest.mock("x")`.
    #[serde(rename = "jest.mock")]
    JestMock,
    /// `jest.doMock("x")`.
    #[serde(rename = "jest.doMock")]
    JestDoMock,
    /// `jest.unmock("x")`.
    #[serde(rename = "jest.unmock")]
    JestUnmock,
    /// `jest.dontMock("x")`.
    #[serde(rename = "jest.dontMock")]
    JestDontMock,
    /// `jest.requireActual("x")`.
    #[serde(rename = "jest.requireActual")]
    JestRequireActual,
    /// `jest.requireMock("x")`.
    #[serde(rename = "jest.requireMock")]
    JestRequireMock,
    /// `jest.createMockFromModule("x")`.
    #[serde(rename = "jest.createMockFromModule")]
    JestCreateMockFromModule,
    /// `jest.genMockFromModule("x")`.
    #[serde(rename = "jest.genMockFromModule")]
    JestGenMockFromModule,
    /// `rs.mock("x")` (also written `rstest.mock`).
    #[serde(rename = "rs.mock")]
    RsMock,
    /// `rs.mockRequire("x")` (also written `rstest.mockRequire`).
    #[serde(rename = "rs.mockRequire")]
    RsMockRequire,
    /// `rs.doMock("x")` (also written `rstest.doMock`).
    #[serde(rename = "rs.doMock")]
    RsDoMock,
    /// `rs.doMockRequire("x")` (also written `rstest.doMockRequire`).
    #[serde(rename = "rs.doMockRequire")]
    RsDoMockRequire,
    /// `rs.unmock("x")` (also written `rstest.unmock`).
    #[serde(rename = "rs.unmock")]
    RsUnmock,
    /// `rs.doUnmock("x")` (also written `rstest.doUnmock`).
    #[serde(rename = "rs.doUnmock")]
    RsDoUnmock,
    /// `rs.unmockRequire("x")` (also written `rstest.unmockRequire`).
    #[serde(rename = "rs.unmockRequire")]
    RsUnmockRequire,
    /// `rs.doUnmockRequire("x")` (also written `rstest.doUnmockRequire`).
    #[serde(rename = "rs.doUnmockRequire")]
    RsDoUnmockRequire,
    /// `rs.importActual("x")` (also written `rstest.importActual`).
    #[serde(rename = "rs.importActual")]
    RsImportActual,
    /// `rs.importMock("x")` (also written `rstest.importMock`).
    #[serde(rename = "rs.importMock")]
    RsImportMock,
    /// `rs.requireActual("x")` (also written `rstest.requireActual`).
    #[serde(rename = "rs.requireActual")]
    RsRequireActual,
    /// `rs.requireMock("x")` (also written `rstest.requireMock`).
    #[serde(rename = "rs.requireMock")]
    RsRequireMock,
}

impl MockCall {
    /// Every call: the `vi` ones, then the `jest` ones, then rstest's (<https://rstest.rs/api/runtime-api/rstest/mock-modules>).
    pub const ALL: [Self; 26] = [
        Self::ViMock,
        Self::ViDoMock,
        Self::ViUnmock,
        Self::ViDoUnmock,
        Self::ViImportActual,
        Self::ViImportMock,
        Self::JestMock,
        Self::JestDoMock,
        Self::JestUnmock,
        Self::JestDontMock,
        Self::JestRequireActual,
        Self::JestRequireMock,
        Self::JestCreateMockFromModule,
        Self::JestGenMockFromModule,
        Self::RsMock,
        Self::RsMockRequire,
        Self::RsDoMock,
        Self::RsDoMockRequire,
        Self::RsUnmock,
        Self::RsDoUnmock,
        Self::RsUnmockRequire,
        Self::RsDoUnmockRequire,
        Self::RsImportActual,
        Self::RsImportMock,
        Self::RsRequireActual,
        Self::RsRequireMock,
    ];

    /// The call as written: `vi.mock`, `jest.requireActual`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ViMock => "vi.mock",
            Self::ViDoMock => "vi.doMock",
            Self::ViUnmock => "vi.unmock",
            Self::ViDoUnmock => "vi.doUnmock",
            Self::ViImportActual => "vi.importActual",
            Self::ViImportMock => "vi.importMock",
            Self::JestMock => "jest.mock",
            Self::JestDoMock => "jest.doMock",
            Self::JestUnmock => "jest.unmock",
            Self::JestDontMock => "jest.dontMock",
            Self::JestRequireActual => "jest.requireActual",
            Self::JestRequireMock => "jest.requireMock",
            Self::JestCreateMockFromModule => "jest.createMockFromModule",
            Self::JestGenMockFromModule => "jest.genMockFromModule",
            Self::RsMock => "rs.mock",
            Self::RsMockRequire => "rs.mockRequire",
            Self::RsDoMock => "rs.doMock",
            Self::RsDoMockRequire => "rs.doMockRequire",
            Self::RsUnmock => "rs.unmock",
            Self::RsDoUnmock => "rs.doUnmock",
            Self::RsUnmockRequire => "rs.unmockRequire",
            Self::RsDoUnmockRequire => "rs.doUnmockRequire",
            Self::RsImportActual => "rs.importActual",
            Self::RsImportMock => "rs.importMock",
            Self::RsRequireActual => "rs.requireActual",
            Self::RsRequireMock => "rs.requireMock",
        }
    }

    /// The call `object.property`, when it is one. `rstest` is an alias of `rs`.
    #[must_use]
    pub fn find(object: &str, property: &str) -> Option<Self> {
        let object = if object == "rstest" { "rs" } else { object };
        Self::ALL
            .into_iter()
            .find(|call| call.as_str().split_once('.') == Some((object, property)))
    }
}

impl std::fmt::Display for MockCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
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
    /// Members read one level further down, by the member of [`ImportedName::accessed_members`] they are read
    /// through: `ns.E.A`, `ns.E['A']` and the type `ns.E.A` read `A` of `ns.E`. A member has an entry only when every
    /// use of it reads its own members that way; a member without one is used as a whole (`ns.E` passed around,
    /// `typeof ns.E`). Each list is sorted and deduplicated. Meaningful only when `local` is set.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub nested_members: BTreeMap<String, Vec<String>>,
    /// The binding is used in a way that cannot be reduced to members (passed around, re-exported, computed access,
    /// destructured with a rest element), or the module value is not bound at all (`import("x")`, bare `require("x")`).
    pub escapes: bool,
    /// The call that receives the namespace of an `import("x")` a function returns, as written (`wrapLazy`,
    /// `routes.define`): `wrapLazy(() => import("x"))` may read any export, so the namespace escapes into it. `None`
    /// for every other binding, and for a callee that is not a name or a chain of names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passed_to: Option<String>,
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
    /// The test-runner call of an [`ImportKind::Mock`] (`vi.mock`, `jest.requireActual`); `None` for every other kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<MockCall>,
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

/// What a member of a class is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClassMemberKind {
    /// A method, abstract or not.
    Method,
    /// A field, including an `accessor` field and a `declare` field.
    Property,
    /// A `get` accessor.
    Getter,
    /// A `set` accessor.
    Setter,
}

/// A member of a class declaration: a method, field or accessor. The constructor, its parameter properties, index
/// signatures, static blocks, members marked `private` and members with a computed key that is not a literal
/// (`[Symbol.iterator]`) are not recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ClassMember {
    /// Member name; an ECMAScript private name keeps its `#` (`#count`).
    pub name: String,
    /// What it is.
    pub kind: ClassMemberKind,
    /// Declared `static`.
    pub is_static: bool,
    /// Has a decorator (`@Input()`, `@HostListener('click')`).
    pub decorated: bool,
    /// A field that exists only for the type checker, which no code reads at runtime: a `declare` field, or one
    /// typed `void`, `never`, `undefined` or `unique symbol` that has no initialiser or is initialised to `undefined`
    /// (`readonly _brand: void = undefined`, `readonly __brand!: unique symbol`), as nominal brands and phantom
    /// type parameters are declared.
    #[serde(default)]
    pub type_only: bool,
    /// Location of the member name.
    pub span: Span,
    /// `JSDoc` tag names (without `@`) on the member.
    pub tags: Vec<String>,
    /// The byte range (absolute, end exclusive) that deletes the member, as for [`Member::removal`]. `None` when the
    /// rest of the class mentions its name, the name is declared more than once (overloads), or the member is
    /// decorated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removal: Option<(u32, u32)>,
}

/// A string an export of a module holds, usable as a property key: the value of an exported string constant, or of
/// a string member of an exported enum or `as const` object.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct StringConstant {
    /// The export name.
    pub export: String,
    /// The enum member or object property that holds the string; `None` when the export is the string itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    /// The string.
    pub value: String,
}

/// An element access whose key is an imported constant: `x[KEY]` and `x[ns.KEY]` (`member` is `None`), `x[E.M]` and
/// `x[ns.E.M]` (`member` is `M`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ImportedKeyRead {
    /// The specifier the constant is imported from, as written.
    pub specifier: String,
    /// The export of that module (`default` for a default import).
    pub export: String,
    /// The enum member or object property of the export that holds the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
}

/// Where the superclass chain of a class leaves its module.
///
/// A superclass declared at the top level of the same module is followed to its own superclass, so a chain that ends
/// in a class of the module with no superclass has no heritage at all.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Heritage {
    /// A binding imported from `specifier`: the export `name` (`default` for a default import), also when written as
    /// a member of a namespace or default import (`React.Component`).
    Imported {
        /// The specifier as written.
        specifier: String,
        /// The export of that module.
        name: String,
    },
    /// An identifier that no top-level declaration or import of the module binds: a global such as `HTMLElement` or
    /// `Error`.
    Global(String),
    /// Anything else: a call (`Mixin(Base)`), a class expression, a local binding that is no top-level class, or a
    /// chain that loops.
    Opaque,
}

/// The members and heritage of a class behind an export, which `unused-class-members` judges (ADR 0042).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct ClassShape {
    /// The members, in source order.
    pub members: Vec<ClassMember>,
    /// The class itself has a decorator (`@Component(...)`, `@Injectable()`).
    #[serde(default)]
    pub decorated: bool,
    /// Where its superclass chain leaves the module; `None` when it has no superclass outside the module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extends: Option<Heritage>,
    /// The member names of the interfaces and object type aliases of the same module that its `implements` clause
    /// names, with the interfaces those extend; sorted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implemented: Vec<String>,
    /// The `implements` clause names a type whose members are not known from the module (an import, a qualified
    /// name, an interface that extends one).
    #[serde(default)]
    pub implements_unknown: bool,
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
    /// An enum export used as a whole in this file, which may read any member: as a value (`Object.values(E)`,
    /// `E[key]`, a spread, `for (k in E)`, passed, stored or returned) or in a type through `typeof E` or a template
    /// literal type (`${E}`). A type annotation `x: E` names the enum type and reads no member.
    #[serde(default)]
    pub used_whole_in_file: bool,
    /// `export const b = a`: the identifier the exported constant is initialised with.
    #[serde(default)]
    pub alias_of: Option<String>,
    /// How the class behind the export is registered as a custom element, in source order. A registered element is
    /// used by the browser through its tag name, not through an import.
    #[serde(default)]
    pub registrations: Vec<Registration>,
    /// The members and heritage of the class behind the export: `export class`, `export default class`, and
    /// `export { C }` or `export default C` of a top-level class declaration. Not set for an ambient (`declare`)
    /// class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<ClassShape>,
    /// The byte offset of the name of the class, enum or namespace declaration of this module the export stands for
    /// (`Foo` in `class Foo`, also behind `export { Foo }` and `export default Foo`), or for an anonymous
    /// `export default class {}`, which has no name, the offset just past its closing brace. The type-aware analysis
    /// keys declarations by it (ADR 0044), and the exports of one class share it; `None` for other exports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declaration_start: Option<u32>,
}

/// How a class is registered as a custom element (ADR 0020).
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
    /// The file is a module by TypeScript's test: a top-level `import`, `export` or `import x = require(…)`. Without
    /// one it is a script, whose declarations are global.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub module_file: bool,
    /// A module file that adds to the global scope: a top-level `declare global { … }` or `export as namespace X`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub global_augmentation: bool,
    /// The command lines the file runs through `node:child_process` (`execSync('tsc -b')`, `spawn('eslint', ['.'])`),
    /// in source order without duplicates; only literal commands. They are parsed like the scripts of `package.json`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
    /// The tag names of the template of a `.vue` file as written (`AppButton`, `app-card`, `div`), with the static
    /// values of its `is` attributes (`is="AppButton"`, `:is="'AppButton'"`), and the names of the
    /// `resolveComponent('AppButton')` calls of any file, sorted and without duplicates: `unplugin-vue-components`
    /// imports the components it registers into the files that use them so (ADR 0020). Empty for other files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub template_tags: Vec<String>,
    /// The property names the file reads, on any object: `x.name`, `x?.name`, `x['name']`, an element access whose key
    /// is a string constant of the file (`x[KEY]`, `x[E.M]`, see [`ModuleInfo::string_constants`]), a destructured key
    /// (`const { name } = x`, `({ name } = x)`, a parameter pattern), `super.name`, `#name` (with its `#`, also in
    /// `#name in x`), the right side of a qualified type name (`typeof x.name`), a string literal type (`'name'`,
    /// `C['name']`) and a string literal argument of a call (`vi.spyOn(x, 'name')`). The markup of a component file and
    /// the text of a file whose templates are not parsed add every `.name` they hold. Sorted, without duplicates; empty
    /// when member accesses are not tracked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub property_reads: Vec<String>,
    /// The strings behind the exports of the file that an element access elsewhere can use as a key: an exported
    /// `const KEY = 'k'`, the string members of an exported enum (`enum E { M = 'k' }`) and the string properties of
    /// an exported `as const` object (`const X = { M: 'k' } as const`). Sorted, without duplicates; empty when member
    /// accesses are not tracked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub string_constants: Vec<StringConstant>,
    /// The element accesses of the file whose key is an imported constant (`x[KEY]`, `x[E.M]`, `x[ns.E.M]`): the
    /// property they read is the string behind that export ([`ModuleInfo::string_constants`] of the module it is
    /// imported from), which only the module graph resolves. An element access whose key is a constant of the file
    /// itself is in [`ModuleInfo::property_reads`]. Sorted, without duplicates; empty when member accesses are not
    /// tracked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imported_key_reads: Vec<ImportedKeyRead>,
    /// The property names the file assigns with a plain `=` (`x.name = 1`, `x['name'] = 1`, `this.#name = 1`), which
    /// are no reads. Sorted, without duplicates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub property_writes: Vec<String>,
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

    /// Whether the file declares something for every file of a program without being imported: a script (no
    /// [`ModuleInfo::module_file`]), or a module file that augments the global scope
    /// ([`ModuleInfo::global_augmentation`]) or another module ([`ModuleInfo::augmented_modules`]). TypeScript applies
    /// such a declaration file wherever it compiles it; the exports of any other module are used only through imports.
    #[must_use]
    pub fn declares_without_import(&self) -> bool {
        !self.module_file || self.global_augmentation || !self.augmented_modules.is_empty()
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
        let mut selected: HashMap<(u32, u32), u32> = HashMap::default();
        for export in &self.exports {
            if let Some(removal) = &export.removal
                && is_unused(export)
            {
                *selected.entry(removal.statement).or_default() += 1;
            }
        }
        let mut ranges = Vec::new();
        let mut whole_statements = HashSet::default();
        let mut clauses: HashMap<(u32, u32), Vec<(Span, bool)>> = HashMap::default();
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

    /// Adds property names to [`ModuleInfo::property_reads`], keeping it sorted and without duplicates.
    pub(crate) fn add_property_reads(&mut self, names: impl IntoIterator<Item = String>) {
        self.property_reads.extend(names);
        self.property_reads.sort();
        self.property_reads.dedup();
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
        self.template_tags.extend(other.template_tags);
        self.template_tags.sort();
        self.template_tags.dedup();
        self.add_property_reads(other.property_reads);
        self.property_writes.extend(other.property_writes);
        self.property_writes.sort();
        self.property_writes.dedup();
        self.string_constants.extend(other.string_constants);
        self.string_constants.sort();
        self.string_constants.dedup();
        self.imported_key_reads.extend(other.imported_key_reads);
        self.imported_key_reads.sort();
        self.imported_key_reads.dedup();
        self.unresolved_dynamic.extend(other.unresolved_dynamic);
        self.ambient_modules.extend(other.ambient_modules);
        self.augmented_modules.extend(other.augmented_modules);
        self.module_file |= other.module_file;
        self.global_augmentation |= other.global_augmentation;
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
