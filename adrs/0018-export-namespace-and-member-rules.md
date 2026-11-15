---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# Export, Namespace and Member Rules

## Context and Problem Statement

`unused-exports` and `unused-types` report exports nothing imports, and the member rules report enum, namespace and
class members nothing reads. Many modules are not imported by name: they are read through `import * as ns`,
re-exported with `export * as ns`, loaded with `import()` or `require()`, handed to a loader by a resolved path, spied
on in tests, or used as a whole object. An export read only through a namespace is a different finding from one nobody
imports, and knip reports it under its own issue types. A use of a module as a whole can reach every value it exports,
but no type. A template local, a parameter or a local with an import's name is no read of the import. The members of a
class are read through instances that flow through parameters, fields and generics, which syntax alone cannot tie to
one class.

What counts as a use of an export and of a member, and which rule reports an unused one?

## Decision Drivers

* Every unused export and member is reported by exactly one rule
* A use counts when the source names what it uses; a use that can reach anything keeps everything it can reach used
* A use as a whole of a module object reaches its values, not its types
* Name resolution follows scopes, so a shadowing binding reads nothing of an import
* A finding is worth less than the trust a false one costs: where skif cannot tell whether a member is read, it does
  not report it
* What is judged is decided from the declarations, independent of how uses are found, so a typed source of uses can
  replace the name-based one without changing a rule
* knip's findings are the reference where the source does not settle a case

## Considered Options

* Separate namespace rules for namespace-only modules, member reads resolved by scope, whole uses that reach only what
  the runtime object holds, and class members judged from their declarations with uses from a replaceable source
* Report every unused export under `unused-exports` and `unused-types`, and treat any namespace use as a use of every
  export
* Match uses by name only, without scopes, and track class instances from the import binding

## Decision Outcome

Chosen option: "Separate namespace rules for namespace-only modules, member reads resolved by scope, whole uses that
reach only what the runtime object holds, and class members judged from their declarations with uses from a
replaceable source", because each export and member then has one rule, each use counts for what the source shows it
reads, and an uncertain read keeps a member used instead of reporting it.

### Namespace rules

* `unused-namespace-exports` (values) and `unused-namespace-types` (interfaces and type aliases) are `js` rules with
  default severity `warning` and an unsafe fix, like `unused-exports` and `unused-types`.
* A module is namespace-only when it is reached only through `import * as ns` or `export * as ns` whose members are
  read. A named, default or dynamic import of the module, or any use of it as a whole, disqualifies it.
* The unused exports of a namespace-only module are reported by the namespace rules and never also by `unused-exports`
  or `unused-types`; every other module keeps its findings under the latter.

### Member reads

* A member read is `ns.m`, `ns['m']`, destructuring (`const { m } = ns`) and `ns.Type` in a type position.
* `vi.spyOn(ns, 'a')`, `jest.spyOn(ns, 'a')` and `spyOn(ns, 'a')` with a literal second argument read `a`.
* In `const [a, b] = await Promise.all([import('./a'), import('./b')])`, each `import()` takes the bindings of the
  pattern element at its position; an element without one, or after a spread, is a use as a whole.
* One nested level is read: `ns.E.A` reads `A` of `E` when every use of `E` reads its members, so the unused members
  of an enum or namespace reached as `ns.E` are reported. Deeper chains read one level.
* `export * as ns` and `import * as ns; export { ns }` forward only the members their consumers read, and the whole
  module when `ns` is used as a whole.
* References are matched to bindings with `oxc_semantic`: a parameter or local of an inner scope with an import's name
  shadows it and reads nothing of it. The binding of a dynamic import or `require()` also counts the references of its
  name that resolve to no other tracked binding, as knip matches those by name.
* Template expressions are read under the template's scopes
  ([ADR 0020](0020-templates-components-and-custom-elements.md)).

### Uses as a whole

* A namespace binding used as a whole (passed, spread, indexed with a computed key, `typeof ns`), and an `import()` or
  `require()` whose result is not bound to names, use every value export of the module, with `export *` followed, and
  no type export. A named re-export not marked `type` counts as a value. Members read by name keep their types used.
* A namespace binding the importing module exports itself (`export default ns`, `export = ns`) uses every export,
  values and types. So do entry modules, `export * as ns` re-exports, the modules a glob import matches, and a file a
  `new URL('./x', import.meta.url)` names, which loads as a program of its own.
* A module used as a whole is not namespace-only, so its unused types are reported by `unused-types`.

### Dynamic imports returned to a call

* A function passed as the first argument of a known lazy-component loader that returns `import("x")` (an arrow
  function with that body, or one `return import("x")`, also awaited) uses the default export of `x` only. The known
  loaders are `lazy` of `react` and `preact/compat` (also as `React.lazy` on a default or namespace import), the default
  export of `next/dynamic`, and the default export of `@loadable/component` when its options cannot pick another
  export (no `resolveComponent`, no spread).
* A function returning `import("x")` passed to any other call keeps the module used as a whole, since that call may
  read any export. The parser records the call as written on the import's binding, and `explain-usage` names it.
* `import("x").then((m) => m.Name)` reads `Name`, inside a loader or not.

### Resolve calls

* `require.resolve(x)`, `createRequire(…).resolve(x)` and `import.meta.resolve(x)` use every value export of the module
  they name and no type export, as a `require()` that binds nothing does: what loads a module by its path receives its
  module object.
* A resolve call binds nothing, so it is no source of a re-exported binding and gives a class no known base. It loads
  nothing at the call site, so it closes no import cycle. The package it names is a used dependency, and which package
  `createRequire` resolves from is decided in [ADR 0019](0019-dependency-unlisted-and-unresolved-findings.md).
* A module that only a plugin input resolves (an extensionless `mise` file task) is a build source, so its exports are
  judged.
* `explain-usage` names the use `resolve`.

### Containers and in-file uses

* A `type` alias or `interface` that shares its name with a value export is a container like the value: what it
  mentions is used whenever the name is used. Every declaration of a name contributes, so all overload signatures
  count.
* Members read in their own file count as used. A whole use of an enum in its own file (as a value, `typeof E`,
  `` `${E}` ``) uses every member; a type annotation `x: E` uses none, and `E.A` reads `A` only. Namespaces count only
  the members read by name in their file.
* `ignore-exports-used-in-file` leaves an export unreported when its own file references it.

### `unused-class-members`

* `unused-class-members` (category `js`, default `warning`, unsafe fix) judges the members of a class behind an export
  that another module uses, in a module whose exports are reported: not an entry file unless `include-entry-exports`
  is set, not ignored, not in a fixture directory, not a declaration file, not in an unjudged scope of
  [ADR 0016](0016-config-evaluation-and-incomplete-entries.md). The class is `export class`, `export default class`
  (also anonymous), or `export { C }` or `export default C` of a top-level class declaration. An ambient class and a
  class expression are not judged, and an unused class is `unused-exports`.
* Its members are the methods (also abstract), fields (also `accessor` fields), getters and setters, static or not. A
  getter and a setter of one name are two members, an overloaded method is one, and an ECMAScript private name
  (`#count`) is a member read only in its own module. The constructor and its parameter properties, `private` members,
  index signatures, static blocks and members with a computed key that is not a literal are not members.

#### Exclusions decided from the declarations

* A decorated member, and every member of a decorated class, since a decorator can hand them to a framework that reads
  them by reflection or from a template.
* A member of a class whose superclass chain leaves the project, as far as skif follows it through the module's
  top-level classes and the class exports of the modules it imports, re-exports included. A chain that reaches a known
  library base (React's `Component` and `PureComponent`) leaves alone the members that base declares and judges the
  others. A chain that reaches any other class outside the project (an installed package, a global such as
  `HTMLElement`, a mixin call, a binding that is no class) leaves every member alone.
* An instance member that an interface the class implements declares, when the interface or object type alias is
  declared in the same module, with the local interfaces it extends; every instance member when the `implements` clause
  names a type the module does not declare.
* A member of a class an entry file exposes, also through `export { C } from` and `export *` chains, unless the entry's
  package sets `include-entry-exports`.
* A member exempted by a tag or by an `ignore-members` list, as enum members are.
* A type-only field: a `declare` field, or a field typed `void`, `never`, `undefined` or `unique symbol` that has no
  value, the value `undefined` or the definite-assignment form (`!`). The exemption is `type-only-field`.

#### Uses

* The rule asks a member-usage source behind one internal interface which modules use a member, never the module facts
  directly. Where the type-aware analysis covers a file the source is typed
  ([ADR 0034](0034-type-aware-analysis-on-tsrs.md)); in every other file, and in template markup, it is name-based.
* The name-based source credits a member when any module of the graph reads a property of its name, on any object:
  `x.name`, `x?.name`, `x['name']`, `` x[`name`] ``, a destructured key, `super.name`, `#name` (also `#name in x`), the
  right side of a qualified type name, a string literal type (`C['name']`) and a string literal argument of a call
  (`vi.spyOn(x, 'name')`, `Reflect.get(x, 'name')`). In the markup of `.vue`, `.svelte`, `.astro` and `.mdx` files and
  the text of `.tsrx` and `.marko` files, every `.name` counts, read lexically.
* An element access `x[K]` reads the string `K` holds when `K` is provably constant: a top-level `const` initialised
  with a string, a string enum member or a string property of an object declared `as const`. `K` is resolved by scope
  in its module and across the module graph through imports, re-exports and `export *`; a binding that is not such a
  constant reads nothing.
* A plain assignment (`x.name = 1`) is no read; a setter is used when one assigns it, and a field that is only assigned
  is unused. Compound assignments and updates count as reads.

#### Fix and explanation

* The fix deletes the member, with the comment lines above it when it stands alone on its lines, as for enum members.
  It is withheld for a member whose name the rest of the class mentions, for an overloaded method and for a decorated
  member. The fix is unsafe because a dynamic access may read the member.
* `skif explain-usage FILE#Class.member` explains the class export and then the member: the files that read it, the
  exclusion that applies, and the verdict (`used`, `unused`, `exempt`, `not-judged`, `not-a-member`).

### Exemptions

* Exports tagged `@public`, `@beta`, `@alpha` or a tag in `ignore-tags` (`--tags`).
* Exports of entry files, unless `include-entry-exports` (`--include-entry-exports`).
* Exports used in the signature of a used export.
* Registered custom elements ([ADR 0020](0020-templates-components-and-custom-elements.md)).
* The fixes of these rules, and their exclusions, are those of
  [ADR 0009](0009-fixes-edits-and-package-json-formatting.md).
* Every new shape of use changes the parse output, so it raises the parse cache schema of
  [ADR 0029](0029-performance-caching-benchmarks-and-budgets.md).

### Consequences

* Good, because both knip namespace issue types have a rule, and every unused export is reported once.
* Good, because the severity of namespace findings can be set apart from the others.
* Good, because spying on one member, loading modules together, or a shadowing parameter hides none of a module's
  other unused exports.
* Good, because the named exports of a lazily loaded page are reported, while a loader that reads another export never
  makes skif report a used export.
* Good, because the exports of workers, bundler loaders and plugins named by a resolved path are used, and a resolve
  call and a `require()` without a binding of the same module agree.
* Good, because unused methods and fields of plain classes are reported without a type checker, a member read anywhere
  is never reported, and `explain-usage` names the exclusion of a member that is not judged.
* Bad, because a user who raises `unused-exports` to an error must raise the namespace rules too to keep reporting the
  exports of namespace-only modules.
* Bad, because skif reports types under a whole use that knip does not report by default, and for an `export enum`
  with a whole use in its own file skif reports fewer members than knip.
* Bad, because a project's own wrapper around `lazy` keeps the whole module used where knip credits `default` only.
* Bad, because a resolve call whose path is only printed or compared keeps every value export of the module used.
* Bad, because in untyped files common member names (`name`, `id`, `render`) are read somewhere in most projects, so
  unused members with such names are missed, and classes of decorator frameworks and of unknown base classes are not
  judged at all.

### Confirmation

* `crates/skif-js/tests/namespaces_and_components.rs`:
  `a_module_only_read_through_a_namespace_reports_its_unread_exports_under_the_namespace_rules`,
  `a_namespace_re_exported_with_export_star_as_forwards_only_the_members_read_through_it`,
  `a_namespace_used_as_a_whole_uses_every_value_export_and_no_type` and
  `a_whole_namespace_use_keeps_the_types_read_by_name_and_an_exported_namespace_keeps_all`.
* `crates/skif-js-syntax/tests/module_info.rs`: `a_spy_on_a_namespace_member_reads_that_member`,
  `imports_awaited_together_bind_the_element_of_the_array_pattern_at_their_position`,
  `imports_awaited_together_after_a_spread_are_used_as_a_whole`,
  `members_read_through_a_member_of_a_namespace_are_recorded_one_level_down`,
  `a_member_used_other_than_through_its_members_stays_whole`,
  `a_parameter_or_local_with_the_name_of_an_import_reads_nothing_of_it`, `type_only_fields_are_marked` and
  `constant_keys_are_read_as_their_strings`.
* `crates/skif-js/tests/knip_parity.rs`: `a_spy_on_a_namespace_uses_only_the_member_it_spies_on`,
  `imports_awaited_together_use_only_what_their_bindings_read`,
  `an_enum_read_through_a_namespace_uses_only_the_members_read_on_it`,
  `a_member_read_through_an_export_star_as_uses_only_that_export`,
  `a_parameter_that_shadows_a_namespace_import_does_not_use_the_namespace`,
  `a_type_that_shares_its_name_with_a_value_export_credits_what_it_names`, `a_new_url_target_is_used_as_a_whole`,
  `a_member_read_in_its_own_file_counts_and_a_whole_use_there_uses_every_member`,
  `a_whole_use_of_an_imported_enum_uses_every_member` and `a_lazy_component_loader_uses_only_the_export_it_renders`.
* `crates/skif-js-syntax/tests/fp_graph.rs`:
  `a_type_that_shares_its_name_with_a_value_exported_by_a_clause_contains_what_it_names`,
  `an_enum_used_as_a_whole_in_its_own_file_is_flagged_and_one_read_by_name_or_annotated_is_not`,
  `a_namespace_used_as_a_whole_in_its_own_file_is_not_flagged` and
  `a_lazy_component_loader_reads_the_default_export_of_the_import_it_returns`.
* `crates/skif-js/tests/explain_absence.rs`: `a_whole_namespace_use_is_listed_for_values_and_not_for_types`.
* `crates/skif-js/tests/resolve_calls.rs`: `esm_resolve_calls_keep_the_files_used_and_use_their_values`,
  `a_commonjs_resolve_call_keeps_the_file_used_and_uses_its_values`,
  `the_exports_of_a_module_loaded_by_its_resolved_path_are_used`, `a_type_export_of_a_resolved_module_is_reported`,
  `explain_usage_names_a_resolve_call_as_using_the_values_of_the_module` and
  `a_module_a_mise_file_task_resolves_is_used_and_its_exports_are_judged`.
* `crates/skif-js/tests/class_members.rs`: `members_no_module_reads_are_reported`,
  `a_class_no_module_uses_is_left_to_unused_exports`, `a_decorated_member_or_class_is_not_judged`,
  `members_a_known_library_base_declares_are_not_judged`,
  `a_base_class_whose_members_are_unknown_leaves_every_member_alone`, `implemented_interface_members_are_not_judged`,
  `tags_and_ignore_members_exempt_a_member`, `entry_exports_are_public_api_unless_include_entry_exports`,
  `a_spied_name_and_a_template_read_count`, `type_only_fields_are_not_judged`,
  `an_element_access_through_a_constant_key_reads_its_string`,
  `an_element_access_through_a_key_that_is_no_constant_reads_nothing`,
  `an_element_access_through_an_imported_constant_reads_its_string`,
  `explain_usage_gives_the_evidence_for_a_member` and `explain_usage_names_the_type_only_exemption`.
* `crates/skif-js/tests/fixtures.rs`: `class_members_follow_knips_fixture`,
  `class_members_of_entry_files_with_include_entry_exports` and `removing_an_unused_class_member_takes_its_line`.
* `docs/rules.md` and `rule-ids.txt` list the namespace rules and `unused-class-members`; `scripts/check-rule-ids.sh`
  passes.

## Pros and Cons of the Options

### Separate namespace rules for namespace-only modules, member reads resolved by scope, whole uses that reach only what the runtime object holds, and class members judged from their declarations with uses from a replaceable source

* Good, because each finding names the access pattern that leaves the export or member unused.
* Good, because one semantic analysis per parsed block serves every pass that needs scopes.
* Good, because every uncertainty makes a member used or not judged, never reported, and a typed source sharpens the
  rule where types exist.
* Bad, because more shapes of use change the parse output, which moves the module cache schema.
* Bad, because a shared member name hides an unused member in an untyped file.

### Report every unused export under `unused-exports` and `unused-types`, and treat any namespace use as a use of every export

* Good, because there are fewer rules and a simpler model.
* Bad, because a namespace import hides every unused export of its module, and knip's namespace findings have no
  counterpart.

### Match uses by name only, without scopes, and track class instances from the import binding

* Good, because no semantic analysis is needed for exports, and a read through an unrelated object would not credit a
  class member.
* Bad, because a parameter or template local with an import's name makes an unused import look used.
* Bad, because instances flow through parameters, returns, fields, containers and generics, so most reads cannot be
  tied to a class without types, and untracked reads would turn used members into findings.
