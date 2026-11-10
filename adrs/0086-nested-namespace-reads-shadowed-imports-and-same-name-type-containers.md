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
are reported, also through a named re-export (`export { E } from`). Through an `export * as E` re-export the members
are exports of its module, which then has only those exports used: knip 6.44.1 reports the other exports of a module
read as `ns.utils.fn()` through `export * as utils` as unused too. Unlike knip, skif keeps such a module used as a whole
when `ns.utils` itself is passed around. Deeper chains (`ns.a.b.c`) and members of a named import that is itself a
namespace (`import { NS }` then `NS.E.A`) still read one level.

**Shadowed imports.** A reference counts for the binding semantic analysis resolves it to, by symbol. The binding of an
`import` declaration (also one in an ambient module block) counts only the references that resolve to it or to
nothing; a parameter or a local of an inner scope with the same name shadows the import and reads nothing of it.
Parameter decorators resolve outside their function, as TypeScript evaluates them, so `@Inject(tok.KEY) tok` reads the
import `tok`. The binding of `const ns = await import("x")`, `const x = require("x")`, a `Promise.all` pattern or an
`import("x").then` callback counts the references that resolve to it, and also, as knip matches the bindings of
dynamic imports by name, the references of its name that resolve to no other tracked binding: a shadowing local can
only make it look more used. An import declaration and an `import()` binding of the same name (`import * as config` and
`const config = await import("./config")` in a test) each get their own reads. `import x = require("x")` is matched by
name. References in a component script block to a binding of another block do not resolve and count for the binding of
their name.

**Same-name type containers.** A `type` alias or `interface` that shares its name with a value export, exported with
it or by the same `export { X }` clause, is a container like the value: what it mentions, including the interfaces it
extends and re-exports named through an import, is used whenever the shared name is used, as a type or as a value. A
type with no value of its name is still not followed, matching knip's default for `ignoreExportsUsedInFile`. knip
merges the two declarations into one export whose kind is that of the first one; skif follows the type whichever comes
first. Every declaration of a name contributes what it mentions, so the signatures of an overloaded function all
count, not only the last one.

### Consequences

* Good, because unused members of enums and namespaces reached through a namespace import are reported.
* Good, because a parameter that shares an import's name no longer hides the module's unused exports.
* Good, because the types a decorator-and-interface service or a value-and-type alias names are judged by the use of
  the name.
* Good, because one semantic analysis per parsed block serves every pass that needs scopes (import bindings, in-file
  references, namespace bodies, custom element registries); only a file with bound imports that no other pass analyses
  gains one.
* Bad, because the parse output changes for files that read a member through a member of an import
  (`nested_members`), that declare a parameter or local with an import's name or bind an `import()` to it (the members
  and the escape of the import), and that pair a type with a value of its name or declare an overloaded function (what
  each export is referenced in, now from every declaration of the name). The probe corpus of the cache digest test
  covers these shapes. The module cache schema stays at 2 only because that schema is not released yet.

### Confirmation

* `skif-js-syntax` tests `members_read_through_a_member_of_a_namespace_are_recorded_one_level_down`,
  `a_member_used_other_than_through_its_members_stays_whole`,
  `members_read_one_level_down_in_two_script_blocks_are_merged`,
  `a_component_template_that_reads_a_member_uses_it_as_a_whole`,
  `a_parameter_or_local_with_the_name_of_an_import_reads_nothing_of_it`,
  `an_import_and_a_dynamic_import_bound_to_the_same_name_each_get_their_own_reads`,
  `a_parameter_decorator_reads_the_import_its_parameter_shadows` and
  `a_type_that_shares_its_name_with_a_value_exported_by_a_clause_contains_what_it_names`.
* `skif-js` tests `an_enum_read_through_a_namespace_uses_only_the_members_read_on_it`,
  `a_member_read_through_an_export_star_as_uses_only_that_export`,
  `a_parameter_that_shadows_a_namespace_import_does_not_use_the_namespace`,
  `a_dynamic_import_bound_to_the_name_of_an_import_gets_its_own_reads` and
  `a_type_that_shares_its_name_with_a_value_export_credits_what_it_names`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Read one level further, match import bindings by scope, and follow a type that shares a value's name

* Good, because the result matches how the code uses each binding.
* Bad, because a member read one level deeper is credited by name: `ns.E.A` reads `A` whatever `E` is. It matters
  where `E` has members skif reports: an enum, a TypeScript namespace, or the module of an `export * as E` re-export.

### Keep the current reads

* Good, because nothing changes.
* Bad, because every enum read through a namespace import keeps its unused members, any parameter named like an import
  hides the module's unused exports, and every type a same-name service interface names stays unreported.

## More Information

* Amends: [ADR 0029](0029-namespace-export-rules-virtual-modules-linked-configs-and-the-node-plugin.md) (what a read
  through a namespace uses)
* Related decisions: [ADR 0079](0079-namespace-reads-through-spies-and-imports-awaited-together.md)
* See the [ADR index](index.md).
