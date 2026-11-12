---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Export, Namespace and Member Rules

## Context and Problem Statement

`unused-exports` and `unused-types` report exports nothing imports, and the member rules report enum and namespace
members nothing reads. Many modules are not imported by name: they are read through `import * as ns`, re-exported with
`export * as ns`, loaded with `import()` or `require()`, spied on in tests, or used as a whole object. An export read
only through a namespace is a different finding from one nobody imports, and knip reports it under its own issue
types. A use of a module as a whole can reach every value it exports, but no type. A template local, a parameter or a
local with an import's name is no read of the import.

What counts as a use of an export and of a member, and which rule reports an unused one?

## Decision Drivers

* Every unused export is reported by exactly one rule
* A use counts when the source names what it uses; a use that can reach anything keeps everything it can reach used
* A use as a whole of a module object reaches its values, not its types
* Name resolution follows scopes, so a shadowing binding reads nothing of an import
* knip's findings are the reference where the source does not settle a case

## Considered Options

* Separate namespace rules for namespace-only modules, member reads resolved by scope, and whole uses that reach only
  what the runtime object holds
* Report every unused export under `unused-exports` and `unused-types`, and treat any namespace use as a use of every
  export
* Match uses by name only, without scopes

## Decision Outcome

Chosen option: "Separate namespace rules for namespace-only modules, member reads resolved by scope, and whole uses
that reach only what the runtime object holds", because each export then has one rule, and each use counts for what
the source shows it reads.

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

### Containers and in-file uses

* A `type` alias or `interface` that shares its name with a value export is a container like the value: what it
  mentions is used whenever the name is used. Every declaration of a name contributes, so all overload signatures
  count.
* Members read in their own file count as used. A whole use of an enum in its own file (as a value, `typeof E`,
  `` `${E}` ``) uses every member; a type annotation `x: E` uses none, and `E.A` reads `A` only. Namespaces count only
  the members read by name in their file.
* `ignore-exports-used-in-file` leaves an export unreported when its own file references it.

### Exemptions

* Exports tagged `@public`, `@beta`, `@alpha` or a tag in `ignore-tags` (`--tags`).
* Exports of entry files, unless `include-entry-exports` (`--include-entry-exports`).
* Exports used in the signature of a used export.
* Registered custom elements ([ADR 0020](0020-templates-components-and-custom-elements.md)).
* The fixes of these rules, and their exclusions, are those of
  [ADR 0009](0009-fixes-edits-and-package-json-formatting.md).

### Consequences

* Good, because both knip namespace issue types have a rule, and every unused export is reported once.
* Good, because the severity of namespace findings can be set apart from the others.
* Good, because spying on one member, loading modules together, or a shadowing parameter hides none of a module's
  other unused exports.
* Good, because the types of a module only loaded with `require()` or `import()` are reported when nothing reads them,
  and `explain-usage` lists a whole use for values only, so the evidence agrees with the verdict.
* Bad, because a user who raises `unused-exports` to an error must raise the namespace rules too to keep reporting the
  exports of namespace-only modules.
* Bad, because skif reports types under a whole use that knip does not report by default, and for an `export enum`
  with a whole use in its own file skif reports fewer members than knip.

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
  `a_parameter_or_local_with_the_name_of_an_import_reads_nothing_of_it`.
* `crates/skif-js/tests/knip_parity.rs`: `a_spy_on_a_namespace_uses_only_the_member_it_spies_on`,
  `imports_awaited_together_use_only_what_their_bindings_read`,
  `an_enum_read_through_a_namespace_uses_only_the_members_read_on_it`,
  `a_member_read_through_an_export_star_as_uses_only_that_export`,
  `a_parameter_that_shadows_a_namespace_import_does_not_use_the_namespace`,
  `a_type_that_shares_its_name_with_a_value_export_credits_what_it_names`, `a_new_url_target_is_used_as_a_whole`,
  `a_member_read_in_its_own_file_counts_and_a_whole_use_there_uses_every_member` and
  `a_whole_use_of_an_imported_enum_uses_every_member`.
* `crates/skif-js-syntax/tests/fp_graph.rs`:
  `a_type_that_shares_its_name_with_a_value_exported_by_a_clause_contains_what_it_names`,
  `an_enum_used_as_a_whole_in_its_own_file_is_flagged_and_one_read_by_name_or_annotated_is_not` and
  `a_namespace_used_as_a_whole_in_its_own_file_is_not_flagged`.
* `crates/skif-js/tests/explain_absence.rs`: `a_whole_namespace_use_is_listed_for_values_and_not_for_types`.
* `docs/rules.md` and `rule-ids.txt` list both namespace rules; `scripts/check-rule-ids.sh` passes.

## Pros and Cons of the Options

### Separate namespace rules for namespace-only modules, member reads resolved by scope, and whole uses that reach only what the runtime object holds

* Good, because each finding names the access pattern that leaves the export unused.
* Good, because one semantic analysis per parsed block serves every pass that needs scopes.
* Bad, because more shapes of use change the parse output, which moves the module cache schema.

### Report every unused export under `unused-exports` and `unused-types`, and treat any namespace use as a use of every export

* Good, because there are fewer rules and a simpler model.
* Bad, because a namespace import hides every unused export of its module, and knip's namespace findings have no
  counterpart.

### Match uses by name only, without scopes

* Good, because no semantic analysis is needed.
* Bad, because a parameter or template local with an import's name makes an unused import look used.
