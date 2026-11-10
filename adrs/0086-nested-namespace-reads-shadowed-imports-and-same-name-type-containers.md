---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0029"]
---

# Nested Namespace Reads, Shadowed Imports and Same-Name Type Containers

## Context and Problem Statement

Three shapes made skif and knip disagree on the knip comparison corpus, each time with skif wrong.

* `import * as Constants from './constants'` followed by `Constants.SearchCommandIds.OpenMatch` (vscode) read member
  `SearchCommandIds` of the namespace, and a member read through a namespace was used as a whole: every member of the
  enum counted as read, so the members nothing reads were not reported.
* `import * as chars from './chars'` in a file whose methods take a parameter `chars` (angular's lexers) counted
  `this.attempt(chars)` as a use of the namespace as a whole, because uses were matched by name. The unused exports
  of `chars.ts` were not reported.
* `export const Service = ClientService; export type Service = ClientService` (opencode), and the
  `export const IStore = createDecorator<IStore>(…); export interface IStore { … }` pattern (vscode): the type
  declaration names what the shared name stands for, but skif followed only value declarations into the types and
  classes they mention, so `ClientService`, and the option and result types of the interface's methods, were reported
  unused although the name is imported.

How does the analysis read these three shapes?

## Decision Drivers

* A read of one member must not make every member used
* A name bound in an inner scope is a different binding, as TypeScript and knip see it
* knip keeps one export per name: a value and a type of the same name are one export
* The analysis stays syntactic: scopes from `oxc_semantic`, no type information

## Considered Options

* Read one level further, match import bindings by scope, and follow a type that shares a value's name
* Keep the current reads

## Decision Outcome

Chosen option: "Read one level further, match import bindings by scope, and follow a type that shares a value's name",
because each shape names in the source what it uses.

**Nested reads.** For each member `E` read through a bound import `ns`, the module facts record the members read on
it: `ns.E.A`, `ns.E['A']` and the type `ns.E.A` read `A` of `E` (`ImportedName::nested_members`). A member gets an
entry only when every use of it reads its own members that way; `ns.E` passed around, `typeof ns.E`, `const { E } =
ns`, a spy on it, or a read in a component template leave it used as a whole, as before. A namespace import credits
such a member with only those members, so the unused members of an enum or a TypeScript namespace reached as `ns.E`
are reported, also through a named re-export (`export { E } from`) and an `export * as E` re-export, whose module then
has only those exports used. Deeper chains (`ns.a.b.c`) and members of a named import that is itself a namespace
(`import { NS }` then `NS.E.A`) still read one level.

**Shadowed imports.** A binding of an `import` declaration counts a reference only when semantic analysis resolves it
to a top-level binding, to an import binding (also one of an ambient module block), or not at all. A parameter or a
local of an inner scope with the same name shadows the import and reads nothing of it. Bindings of `import()` and
`require()` (`const ns = await import("x")` inside a function) are still matched by name, as knip does for dynamic
imports, so a shadowing local can only make them look more used. References in a component script block to a binding
of another block do not resolve and still count.

**Same-name type containers.** An exported `type` alias or `interface` that shares its name with a value export is a
container like the value: what it mentions, including the interfaces it extends and re-exports named through an
import, is used whenever the shared name is used, as a type or as a value. A type with no value of its name is still
not followed, matching knip's default for `ignoreExportsUsedInFile`. knip merges the two declarations into one export
whose kind is that of the first one; skif follows the type whichever comes first.

### Consequences

* Good, because unused members of enums and namespaces reached through a namespace import are reported.
* Good, because a parameter that shares an import's name no longer hides the module's unused exports.
* Good, because the types a decorator-and-interface service or a value-and-type alias names are judged by the use of
  the name.
* Bad, because the parse output changes for files that read `ns.E.A`; the module cache schema stays at 2 only because
  that schema is not released yet.
* Bad, because each file with a bound import is analysed for scopes once more.

### Confirmation

* `skif-js-syntax` tests `members_read_through_a_member_of_a_namespace_are_recorded_one_level_down`,
  `a_component_template_that_reads_a_member_uses_it_as_a_whole` and
  `a_parameter_or_local_with_the_name_of_an_import_reads_nothing_of_it`.
* `skif-js` tests `an_enum_read_through_a_namespace_uses_only_the_members_read_on_it`,
  `a_parameter_that_shadows_a_namespace_import_does_not_use_the_namespace` and
  `a_type_that_shares_its_name_with_a_value_export_credits_what_it_names`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Read one level further, match import bindings by scope, and follow a type that shares a value's name

* Good, because the result matches how the code uses each binding.
* Bad, because a member read one level deeper is credited by name: `ns.E.A` reads `A` whatever `E` is, which only
  matters for enums and namespaces.

### Keep the current reads

* Good, because nothing changes.
* Bad, because every enum read through a namespace import keeps its unused members, any parameter named like an import
  hides the module's unused exports, and every type a same-name service interface names stays unreported.

## More Information

* Amends: [ADR 0029](0029-namespace-export-rules-virtual-modules-linked-configs-and-the-node-plugin.md) (what a read
  through a namespace uses)
* Related decisions: [ADR 0079](0079-namespace-reads-through-spies-and-imports-awaited-together.md)
* See the [ADR index](index.md).
