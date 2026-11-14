---
status: accepted
date: 2026-11-14
decision-makers: ["Na'aman Hirschfeld"]
---

# Unused Class Members

## Context and Problem Statement

knip 5 reported the methods, fields and accessors of exported classes that nothing references under its
`classMembers` issue type, off by default. It asked TypeScript's language service for the references of each member,
so a read counted only when the type checker tied it to that member: `instance.render()` where `instance` is a
`MyComponent`, a member that overrides one of an installed base class, a member that implements an interface. knip 6
dropped the issue type because it depended on that service. skif's analysis is built on oxc and has no type
checker when the project gives it none (no `tsconfig.json`, dependencies not installed), and an unused method is a
finding of the same kind as an unused enum member ([ADR 0018](0018-export-namespace-and-member-rules.md)).

Which members of an exported class does skif report as unused, and what counts as a use without type information?

## Decision Drivers

* A finding is worth less than the trust a false one costs: where skif cannot tell, it does not report
* knip 5's semantics are the reference for what is judged, what is excluded and what counts as a use
* The declaration side (what is never judged) does not depend on how uses are found, so a type-aware source of uses
  can replace the name-based one without changing the rule
* The rule follows the export rules: a class nobody imports is `unused-exports`, entry files are public API, tags and
  `ignore-members` exempt as they do for enum members

## Considered Options

* A rule that credits a member by the property names any module reads, behind a member-usage source, with the
  exclusions decided from the declarations
* Track instances: follow `new C()` and parameters typed `C` from the import binding, and credit only reads through
  them
* No rule until skif has type information

## Decision Outcome

Chosen option: "A rule that credits a member by the property names any module reads, behind a member-usage source,
with the exclusions decided from the declarations", because a name read anywhere can only keep a member used, never
report a used one, and the same rule takes a type-aware source when the analysis has type information. This record
describes the name-based source, which is the fallback when no type information is available.

### What is judged

* `unused-class-members` (category `js`, a warning, unsafe fix) judges the members of a class behind an export of a
  module whose exports are reported (not an entry file unless `include-entry-exports` is set, not ignored, not in a
  fixture directory, not a declaration file, not in an unjudged scope of
  [ADR 0016](0016-config-evaluation-and-incomplete-entries.md)): `export class`, `export default class` (also
  anonymous), and `export { C }` or `export default C` of a top-level class declaration. An ambient (`declare`) class
  and a class expression are not judged.
* The class export must be used by another module ([`ExportUsage::is_used`](0018-export-namespace-and-member-rules.md)),
  and not tagged `@public` (or another tag that exempts an export); an unused class is `unused-exports`.
* Its members are the methods (also abstract), fields (also `accessor` and `declare` fields), getters and setters,
  static or not. As in knip, the constructor and its parameter properties and members marked `private` are not
  members; nor are index signatures, static blocks and members with a computed key that is not a literal
  (`[Symbol.iterator]`). A getter and a setter of one name are two members. An overloaded method is one member.
  An ECMAScript private name (`#count`) is a member, read only in its own module.

### Member use (the name-based source)

* Each module records the property names it reads, on any object: `x.name`, `x?.name`, `x['name']` and
  `` x[`name`] ``, a destructured key (`const { name } = x`, `({ name } = x)`, a parameter pattern), `super.name`,
  `#name` (also `#name in x`), the right side of a qualified type name (`typeof x.name`), a string literal type
  (`'name'`, `C['name']`) and a string literal argument of a call (`vi.spyOn(x, 'name')`,
  `Reflect.get(x, 'name')`). In the markup of a `.vue`, `.svelte` or `.astro` file, the markup of an `.mdx` file and
  the text of a `.tsrx` or `.marko` file, every `.name` counts, read lexically.
* It also records the names it assigns with a plain `=` (`x.name = 1`), which are no reads.
* A member is used when a module of the graph reads its name; a setter also when one assigns it. A field that is
  only assigned is unused, as in knip, which reports members whose only references are writes.
* The source sits behind an internal interface that answers which modules use a member. The rule asks it, never the
  module facts, so a source that resolves each read to the member it reads replaces it without touching the
  exclusions below.

### Exclusions decided from the declarations

* A decorated member, and every member of a decorated class: a decorator can hand them to a framework that reads
  them by reflection or from a template (`@Input()`, `@HostListener()`, `@Component()`, `@Injectable()`).
* A member of a class whose superclass chain leaves the project, as far as skif can follow it: through the
  top-level classes of the module and the class exports of the modules it imports, also through re-exports. A chain
  that reaches a known library base class leaves alone the members that base declares: React's `Component` and
  `PureComponent` (`render`, the lifecycle methods, `state`, `props`, `defaultProps`, ...). A chain that reaches any
  other class outside the project (an installed package, a global such as `HTMLElement` or `Error`, a mixin call, a
  binding that is no class) leaves every member alone, since any of them may override a member of the base.
* An instance member that an interface the class implements declares, when the interface (or object type alias) is
  declared in the same module, with the local interfaces it extends; every instance member when the `implements`
  clause names a type the module does not declare.
* A member of a class an entry file exports, also through `export { C } from` and `export *` chains: it is public API
  unless the entry's package sets `include-entry-exports`.
* A member tagged `@public`, `@beta`, `@alpha` or `@alias` (`@internal` in production mode, an `ignore-tags` name,
  and the `include-tags` filter, as for enum members), and a member name on an `ignore-members` list.

### Deviations from knip 5

* A read of the name on any object counts, so a member is used when an unrelated object's property of the same name
  is read; knip's fixture reports `AbstractClass.implemented`, which skif keeps because a subclass declares the same
  name and a module reads it.
* String literal call arguments and string literal types count as reads; knip counted them only where TypeScript
  tied them to the member (a parameter typed `keyof C`).
* Compound assignments and updates (`x.n += 1`, `x.n++`) are reads; TypeScript calls them writes.
* knip judged decorated members and the members of decorated classes, and the members of classes extending an
  installed base that the base does not declare. skif leaves them alone, except for the known bases above.
* knip left alone only the members of an implemented interface; skif leaves alone every instance member when the
  interface is declared in another module.
* knip judged the members of a class that an entry re-exports when another module also imports it; skif treats
  every class an entry exposes as public API.
* knip's hints about tags that exempt a used member do not cover class members in skif yet.

### Fix, report and cache

* The fix deletes the member: its lines with the comment lines above it when it stands alone on them, as for enum
  members. A member whose name the rest of the class mentions, an overloaded method and a decorated member get no
  fix. The fix is unsafe: a dynamic access (`x[key]`) may read the member.
* `skif explain-usage FILE#Class.member` explains the class export and then the member: the files that read its name,
  the exclusion that applies, and the verdict (`used`, `unused`, `exempt`, `not-judged`, `not-a-member`).
* The module facts gain the class shapes and the read and written property names, so the parse cache schema of
  [ADR 0029](0029-performance-caching-and-benchmarks.md) moves to a new version.

### Consequences

* Good, because unused methods and fields of plain classes are reported without a type checker, and a member read
  anywhere is never reported.
* Good, because the exclusions are named and shown by `explain-usage`, so a member that is not judged says why.
* Good, because a type-aware source can replace the name-based one behind the same rule.
* Bad, because common names (`name`, `id`, `value`, `render`) are read somewhere in most projects, so unused members
  with such names are missed.
* Bad, because a member that installed code calls on an object handed to it (a plugin object with `install()`) is
  reported unless the class implements the library's interface or extends its base class.
* Bad, because classes of frameworks built on decorators (Angular, NestJS, Stencil) and on unknown base classes (Lit,
  web components) are not judged at all.

### Confirmation

* `crates/skif-js/tests/class_members.rs`: the forms of a read, each exclusion with a counterpart that is reported,
  hierarchies across modules, re-exports through an entry and a barrel, spies and template reads, and the
  `explain-usage` report.
* `crates/skif-js/tests/fixtures.rs`: `class_members_follow_knips_fixture`,
  `class_members_of_entry_files_with_include_entry_exports` and `removing_an_unused_class_member_takes_its_line`, on
  knip's `class-members` fixture.
* `crates/skif-js-syntax/tests/module_info.rs`: the members, heritage and implemented names of class shapes, and the
  property names a file reads and writes.

## Pros and Cons of the Options

### Credit by name behind a member-usage source, with exclusions from the declarations

* Good, because it needs only what one file's syntax says, so it costs one pass per file and fits the parse cache.
* Good, because every uncertainty makes a member used or not judged, never reported.
* Bad, because a shared name hides an unused member.

### Track instances from the import binding

* Good, because a read through an unrelated object would not count.
* Bad, because instances flow through parameters, returns, fields, containers and generics, and without types most
  reads cannot be tied to a class; untracked reads would have to count as reads of every member, or be missed and
  turn used members into findings.

### No rule until skif has type information

* Good, because no approximation needs explaining.
* Bad, because projects without type information get no report at all, and the declaration-side exclusions are needed
  either way.
