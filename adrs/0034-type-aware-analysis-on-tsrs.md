---
status: accepted
date: 2026-11-15
decision-makers: ["Na'aman Hirschfeld"]
---

# Type-Aware Analysis on tsrs

## Context and Problem Statement

skif's JS analysis is built on oxc ([ADR 0013](0013-js-analysis-core-oxc-module-graph-and-resolution.md)): a parser,
a resolver and a module graph, with no type checker. Most findings need none: a file is reached or not, a package is
imported or not. Member findings do. Without types `unused-class-members`
([ADR 0018](0018-export-namespace-and-member-rules.md)) credits a member when any module reads a property of its name,
so `a.dispose()` keeps every `dispose` in the project used, and a member reached only through an interface, an
override or a typed receiver is judged by name alone. Enum and namespace members have the same limit when an enum is
passed through an alias or a namespace is read through `this`.

tsrs is a Rust port of the TypeScript 7 compiler (Apache-2.0, crates `tsrs_*` on crates.io, published as pre-release
versions) with its checker. Beyond its API it makes choices for the whole process it runs in: programs live in leaked
arenas that only an unsafe call frees, on unix it reserves a fixed range of address space for its pointers unless built
with `plain-ptrs`, it reads `TSRS_*` and `RAYON_NUM_THREADS` from the environment and runs a global worker pool, and it
pins `unicode-id-start` exactly, which Cargo unifies with oxc's requirement.

Should skif be type-aware, how, and how does it take on tsrs so that hosts and users can trust its behaviour, without
losing its speed and without failing on projects that have no types?

## Decision Drivers

* Accuracy of member findings: a finding is worth less than the trust a false one costs
* Speed stays a product property: type awareness may cost time, and the cost is published and budgeted
  ([ADR 0029](0029-performance-caching-benchmarks-and-budgets.md))
* A project without types (no `tsconfig.json`, dependencies not installed, plain JavaScript) still gets every finding
  the untyped analysis gives
* oxc stays the source of the module graph and of every syntax fact; types add answers, they do not replace the graph
* Hosts that embed skif (poly, the npm addon, the language server) can leave the checker out entirely, and are never
  aborted or have their address space claimed by it
* One version of tsrs at a time, upgraded deliberately with evidence

## Considered Options

* tsrs alongside oxc behind one boundary crate with exact pins, opt-in (off by default), untyped per package where
  types are missing
* tsrs alongside oxc behind the same boundary, on by default and switchable off
* Type information from `tsc` or `tsserver` run as a child process
* No type awareness: improve the name-based heuristics only

## Decision Outcome

Chosen option: "tsrs alongside oxc behind one boundary crate with exact pins, opt-in (off by default), untyped per
package where types are missing", because it gives the accurate answer wherever a project with types asks for it,
keeps the untyped analysis everywhere else, lets a user trade speed for accuracy in one switch, and confines tsrs and
its process-level choices to one crate.

It is opt-in because of what a typed run costs today. Every TypeScript program parses the lib and `@types` files
again, and on a 4-vCPU GitHub runner the benchmark of
[ADR 0029](0029-performance-caching-benchmarks-and-budgets.md) measured typed runs at 3 to 25 times the untyped time,
and slower than knip: babel 18.2 s typed against 1.0 s untyped and 5.3 s for knip, tanstack-query 10.0 s against
0.41 s and 3.4 s, n8n 32.2 s against 6.1 s, vscode 14.7 s against 4.3 s and 14.2 s. That breaks the budget of
ADR 0029 and the speed skif is chosen for. Types become the default again once the programs of a run share their
parsed lib and `@types` files, which a new ADR records.

### The engine and its switches

* tsrs runs alongside oxc. oxc stays the only source of the graph and of the syntax facts; types only add answers about
  which declaration a member read reaches.
* Type awareness is off by default: `[js] type-aware = false`
  ([ADR 0005](0005-the-js-table-ignore-lists-and-suppression-reasons.md)). `[js] type-aware = true`,
  `[js.workspaces."<dir>"] type-aware = true`, the CLI flag `--type-aware` ([ADR 0024](0024-cli-contract.md)) and the
  npm option `typeAware` turn it on; `--type-aware` also overrides every workspace that sets the key. `--no-type-aware`
  and the npm option `noTypeAware` keep it off over the configuration and win over the option that turns it on; the
  two flags conflict. With it off no tsrs code runs and no memory is reserved for it.
* The facade's Cargo feature `type-aware` is off by default, as `js-analysis` is
  ([ADR 0003](0003-library-api-and-embedding.md)); the CLI and the npm addon enable it, so `--type-aware` needs no
  other build. A build without it reports every package untyped with the reason `not-built`.
* `skif lsp`, `skif mcp` and `--watch` run untyped ([ADR 0028](0028-editors-agents-and-git-hooks.md));
  `--type-aware` conflicts with `--watch`.

### The boundary

* `skif-js-types` is the only crate that depends on tsrs. Its API takes the judged files per package and a query (the
  declarations whose member reads skif judges) and returns owned facts: typed member reads, uses of a declaration as a
  whole, reads it could not settle, the files it covered and the coverage of every package. No tsrs type crosses it.
* Declarations are joined on (file, UTF-8 byte offset of the declaration's name), the key oxc's spans give.

### Programs

* Programs follow TypeScript's own discovery: each judged file belongs to its nearest `tsconfig.json` or
  `jsconfig.json`; a solution config is followed through its references to the config whose `include` or `files` holds
  the file; `[js] tsconfig` replaces discovery for the root package, and a given config that cannot be read fails the
  analysis. Each config is built once.
* A file in no program is analysed without types; skif builds no inferred program.
* The facts are copied out and each program is freed before the rules run.

### Coverage

* Each package is `typed`, `partial` or `untyped`, with a reason when not typed: `disabled`, `not-built`,
  `no-tsconfig`, `tsconfig-error`, `not-typescript`, `unresolved-modules`, `files-outside-programs`, `checker-failed` or
  `cancelled`. The report carries the coverage ([ADR 0022](0022-reporting-and-output-formats.md)); missing types are no
  diagnostic.
* A package whose imports do not resolve (dependencies not installed) is `partial`.
* A panic inside tsrs is caught at the boundary, and the affected packages fall back to the untyped analysis with
  `checker-failed`. The release profiles unwind for this reason
  ([ADR 0027](0027-distribution-and-release.md)).

### Credit semantics

* The reads sent to the checker are property access, element access with a literal key, qualified names, binding
  patterns and destructuring assignments, shorthand properties, indexed access types (`C['m']`) and `'m' in x` with a
  literal, and only for names that some judged declaration has as a member.
* A typed read credits what TypeScript would rename together with it: the members its symbol lands on through root
  symbols (unions, intersections, mapped and synthetic properties), and, for a read of a member of a base class or an
  interface, the members that override or implement it in the judged classes.
* A declaration is used as a whole when it escapes so that any member may be read (`keyof typeof E`, `Object.keys(E)`,
  a spread, a value alias passed on). Whole uses from types are only added, never subtracted, and types never remove a
  syntactic use, so typed enum and namespace findings are a subset of the untyped ones.
* A read whose receiver is `any`, an error type or a value of an unresolved module, and every read in a file the checker
  did not cover or in a Vue, Svelte or Astro template, is credited by name as without types. Missing types never create
  a finding.
* What is judged, and every exclusion, stays decided by the declarations
  ([ADR 0018](0018-export-namespace-and-member-rules.md)); only the source of uses changes. `explain-usage` names the
  typed evidence: the read and the declaration it reaches.
* The comparison with knip counts the typed run and sets apart what only one mode reports
  ([ADR 0021](0021-knip-and-syncpack-comparison-harness-and-its-known-deviations.md)).

### Dependency policy

* Every tsrs crate is pinned with `=` to one version. An upgrade is its own change and carries the parity run, the
  typed-versus-untyped diff, the benchmark and an MSRV check (tsrs's `rust-version` must not exceed skif's).
* tsrs's `unicode-id-start` pin applies to the whole workspace lock, oxc included; an upgrade of either side checks that
  the pin still resolves.
* `skif-js` depends on `skif-js-types` only behind its `type-aware` feature, so the facade without default features has
  no tsrs crate in its tree ([ADR 0002](0002-crates-publishing-and-repository-consistency-checks.md)).
* Each program is owned by a guard whose `Drop` calls tsrs's `free_unshared_program`. That call is the one hand-written
  unsafe call in skif, allowed there with its reason. A test checks that arena memory does not grow over repeated
  single-threaded runs in one process.

### Address space, environment and threads

* The npm addon enables `plain-ptrs`. The CLI keeps tsrs's address-space reservation. The facade exposes `plain-ptrs`
  for hosts that hold address-space reservations of their own ([ADR 0003](0003-library-api-and-embedding.md)).
* skif passes the bundled lib path and the checker count explicitly, and never sets or clears `TSRS_*` or
  `RAYON_NUM_THREADS`. Variables a user sets reach tsrs, the recorded exception to "no environment input in the
  library" of [ADR 0003](0003-library-api-and-embedding.md).
* tsrs's worker pool is global to the process and cannot be handed in. The checker count comes from skif's `threads`
  setting, and a single-threaded run uses tsrs's single-threaded mode.
* tsrs's NOTICE, with the TypeScript and DefinitelyTyped notices, ships in every artefact
  ([ADR 0033](0033-attribution-and-vendoring.md)).

### Consequences

* Good, because member findings in typed projects are as precise as the checker, the precision of
  TypeScript's language service.
* Good, because a project without types loses nothing, the default run keeps the untyped speed, and one switch
  turns on the precise member findings.
* Good, because hosts that do not want tsrs never build it, the addon cannot abort Node.js through a reservation, and
  upgrades are visible and measured.
* Bad, because typed runs read and bind every file of each program a second time, beside oxc's parse, and parse the
  lib and `@types` files once per program; the cost is published and budgeted, not avoided, and keeps types off by
  default until those files are shared.
* Bad, because the default run keeps the less accurate member findings, and users who never turn types on never get
  the precise ones.
* Bad, because skif takes on a large pre-release dependency with process-level constraints, the CLI binary grows by the
  checker, the emitter crates tsrs's compiler pulls in and the embedded `lib.*.d.ts` files, and the boundary copies
  every fact into owned types.
* Bad, because a project with many `any` receivers gains little from types.
* Neutral, because oxc stays the only parser of the module graph and the syntax facts, so
  [ADR 0013](0013-js-analysis-core-oxc-module-graph-and-resolution.md) holds for everything but member usage in typed
  files.

### Confirmation

* `crates/skif-js/tests/type_aware.rs`: `typed_reads_leave_a_same_named_member_of_another_class_unused`,
  `a_read_through_an_interface_keeps_the_member_an_implementing_class_inherits_used`,
  `typed_mode_only_removes_enum_and_namespace_findings`,
  `a_package_without_tsconfig_keeps_name_based_class_member_findings`, `uninstalled_dependencies_credit_reads_by_name`,
  `a_vue_template_reading_a_class_member_by_name_keeps_it_used`, `the_report_summarizes_a_mixed_workspace`,
  `a_workspace_that_turns_types_off_is_reported_disabled`, `types_are_off_by_default`,
  `a_workspace_that_turns_types_on_is_typed_under_the_off_default`, `explain_usage_names_the_typed_evidence` and
  `explain_usage_names_reads_credited_by_name_through_unresolved_receivers`.
* `crates/skif-js/tests/type_aware_off.rs`: `types_off_by_default_by_the_file_or_by_no_type_aware_run_no_checker`.
* `crates/skif-config/tests/config.rs`: `defaults_are_sensible`, `type_aware_wins_over_the_file_and_every_workspace`,
  `no_type_aware_wins_over_the_file_and_every_workspace`, `no_type_aware_wins_over_type_aware` and
  `the_file_turns_types_on_over_the_off_default`; `crates/skif-cli/src/args.rs`:
  `type_aware_and_no_type_aware_conflict` and `type_aware_conflicts_with_watch`.
* `crates/skif-js-types/tests/members.rs`:
  `a_typed_receiver_credits_its_class_and_not_a_same_named_member_of_another_class`,
  `a_union_receiver_credits_the_member_on_every_class_of_the_union`,
  `a_read_through_the_base_class_credits_the_overriding_members`,
  `a_read_through_an_interface_credits_the_classes_that_implement_it`,
  `a_read_the_checker_cannot_settle_is_credited_by_name` and
  `declarations_after_a_bom_and_non_ascii_text_join_on_utf8_offsets`.
* `crates/skif-js-types/tests/enums_and_namespaces.rs`:
  `an_enum_member_read_through_an_alias_or_a_literal_key_credits_that_member`,
  `an_enum_that_escapes_is_used_as_a_whole` and
  `a_namespace_member_read_through_this_or_a_qualified_name_credits_that_member`.
* `crates/skif-js-types/tests/programs.rs`: `project_references_resolve_to_the_source_of_the_referenced_project`,
  `a_solution_config_is_followed_to_the_referenced_config_that_includes_the_file`,
  `a_package_without_a_tsconfig_is_untyped`, `uninstalled_dependencies_credit_reads_by_name_and_report_partial`,
  `a_package_whose_tsconfig_does_not_parse_is_untyped`, `an_explicit_tsconfig_that_cannot_be_read_fails_the_analysis`,
  `an_explicit_tsconfig_replaces_discovery_for_the_root_package`,
  `a_file_outside_every_program_keeps_the_untyped_analysis_and_makes_the_package_partial` and
  `cancelled_before_start_returns_cancelled`.
* `crates/skif-js-types/src/coverage.rs`: `a_failed_checker_is_named_first` and
  `a_package_of_plain_javascript_outside_every_program_is_not_typescript`.
* `crates/skif-js-types/tests/determinism.rs`: `type_facts_are_identical_at_one_and_eight_threads`.
* `crates/skif-js-types/tests/arenas.rs`: `repeated_analyses_free_their_arenas`.
* `scripts/check-pins.sh` fails unless every tsrs crate is pinned with `=` to one version;
  `scripts/check-dependency-tree.sh` fails when a crate other than `skif-js-types` reaches a tsrs crate or `skif`
  without default features does; `scripts/npm-smoke.sh` runs the addon with `typeAware` and with `noTypeAware`.

## Pros and Cons of the Options

### tsrs alongside oxc behind one boundary crate with exact pins, opt-in (off by default), untyped per package where types are missing

* Good, because the default run keeps the untyped speed, and untyped packages keep the untyped analysis when types are
  on.
* Good, because the constraints of tsrs are handled in one crate and stay out of the rest of skif.
* Bad, because most users never turn it on and keep the less accurate member findings.

### tsrs alongside oxc behind the same boundary, on by default and switchable off

* Good, because the accurate answer is the default where it is available.
* Bad, because the default run takes 3 to 25 times the untyped time while every program parses the lib and `@types`
  files again, slower than knip on babel and tanstack-query.

### Type information from `tsc` or `tsserver` run as a child process

* Good, because it is the reference implementation.
* Bad, because it needs Node.js and an installed `typescript`, starts slowly and answers over a protocol built for
  editors; skif's binary would not be self-contained.

### No type awareness: improve the name-based heuristics only

* Good, because it adds nothing.
* Bad, because the false positives of name-based member usage stay, and some are not fixable without types.
