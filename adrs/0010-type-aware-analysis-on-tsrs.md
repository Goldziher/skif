---
status: accepted
date: 2026-11-16
decision-makers: ["Na'aman Hirschfeld"]
---

# Type-Aware Analysis on tsrs

## Context and Problem Statement

skif's JS analysis has no type checker ([ADR 0007](0007-js-analysis-core-on-oxc.md)). Most findings need none: a file
is reached or not, a package is imported or not. Member findings do. Without types `unused-class-members`
([ADR 0009](0009-what-counts-as-a-use.md)) credits a member when any module reads a property of its name, so
`a.dispose()` keeps every `dispose` in the project used, and a member reached only through an interface, an override
or a typed receiver is judged by name alone. Enum and namespace members have the same limit.

tsrs is a Rust port of the TypeScript 7 compiler with its checker, published as pre-release crates. Beyond its API it
makes choices for the whole process: programs live in arenas that only an unsafe call frees, on unix it reserves a
fixed range of address space unless built with `plain-ptrs`, it reads `TSRS_*` and `RAYON_NUM_THREADS` and runs a
global worker pool, and it pins `unicode-id-start` exactly, which Cargo unifies with oxc's requirement. A typed run
also costs time and memory beyond the untyped one.

Should skif be type-aware, how does it take on tsrs, and what may a typed run cost?

## Decision Drivers

* Accuracy of member findings: a finding is worth less than the trust a false one costs
* Speed stays a product property; the cost of types is published and budgeted
* A project without types (no `tsconfig.json`, dependencies not installed, plain JavaScript) still gets every finding
  the untyped analysis gives
* oxc stays the source of the module graph and every syntax fact; types add answers, they do not replace the graph
* Hosts that embed skif can leave the checker out entirely and are never aborted or have their address space claimed

## Considered Options

* tsrs alongside oxc behind one boundary crate with exact pins, opt-in, untyped per package where types are missing
* tsrs behind the same boundary, on by default, which costs every run more than the budget allows
* Type information from `tsc` or `tsserver` as a child process, which needs Node and a process boundary
* No type awareness, improving the name-based heuristics only, which cannot tell receivers apart

## Decision Outcome

Chosen option: "tsrs alongside oxc behind one boundary crate with exact pins, opt-in, untyped per package where types
are missing", because it gives the accurate answer wherever a project with types asks for it, keeps the untyped
analysis and its speed everywhere else, and confines tsrs and its process-level choices to one crate.

### Switches

* Types are off by default: `[js] type-aware = false`. `[js] type-aware = true`, a workspace's
  `[js.workspaces."<dir>"] type-aware = true`, `--type-aware` and the npm option `typeAware` turn them on;
  `--type-aware` also overrides every workspace that sets the key. `--no-type-aware` and the npm option `noTypeAware`
  keep them off over the configuration. The two flags conflict on the command line; in the library and the addon, off
  wins. With types off no tsrs code runs and no memory is reserved for it.
* The facade's Cargo feature `type-aware` is off by default; the CLI and the npm addon enable it, so `--type-aware`
  needs no other build. A build without it reports every package untyped with the reason `not-built`.
* `skif lsp`, `skif mcp` and `--watch` run untyped, and `--type-aware` conflicts with `--watch`. With several checker
  threads, tsrs's worker threads keep arenas across runs that skif cannot release, so a process that lints again and
  again would grow with every run; a one-shot lint frees what it holds when it ends.

### The boundary

* `skif-js-types` is the only crate that depends on tsrs, and `skif-js` depends on it only behind its `type-aware`
  feature. Its API takes the judged files per package and a query (the declarations whose member reads skif judges)
  and returns owned facts: typed member reads, whole uses, reads it could not settle, and the coverage of every
  package. No tsrs type crosses it.
* Declarations are joined on (file, UTF-8 byte offset of the declaration's name), the key oxc's spans give.

### Programs

* Programs follow TypeScript's own discovery: each judged file belongs to its nearest `tsconfig.json` or
  `jsconfig.json`; a solution config is followed through its references to the config whose `include` or `files` holds
  the file; `[js] tsconfig` replaces discovery for the root package, and a given config that cannot be read fails the
  analysis. Each config is built once per analysis.
* A file in no program is analysed without types; skif builds no inferred program.
* An analysis plans its programs and builds a few at a time, the largest first, each with checkers in proportion to its
  share of the judged files and never more checkers than it has judged files. Programs can start building from the
  files that may be judged before the query is known; one that no judged file needs is freed unwalked, and nothing
  that shapes a fact depends on which programs started early.
* The facts are copied out and each program is freed as soon as its walk ends, before the rules run.
* The bundled `lib.*.d.ts` files and the declaration files below `node_modules` are read, parsed and bound once per run
  (`shared_files.rs`), each in a region of its own that outlives every program reading it. Sharing is sound because a
  parse depends only on the file's text and parse options, which key the cache, and checkers write only into stores
  of their own or into caches set once in the region that owns the file. This is how tsrs shares declaration files
  between the projects of `tsc -b`.
* Each `tsconfig.json` or `jsconfig.json` a planning searches or a program references, and each config those extend,
  is read and parsed once per run in the same way (`shared_configs.rs`). A program takes its own config as a copy of
  the run's parse, whose options it may replace and whose lazily computed tables it fills for itself, so a program
  changes nothing another program reads. Planning searches each config chain once instead of once per file.
* In a multi-root run the analyses share their cores, so their programs together hold no more cores than the machine
  has, and their programs (`TypePrograms`): a program the judged files of several roots go to is built once, kept while
  a root that has not finished holds a file its config includes, and walked by every analysis that plans files into
  it. An analysis that needs a program another is building waits for that build.
* A program's own sources, and the sources of the projects it references, are shared the same way, keyed on the full
  parse options (so a file two projects parse differently is parsed for each), but held only by the programs that read
  them and freed with the last of them: keeping them for the run would hold every source of a large repository at
  once. Programs that are not alive at the same time each parse a file they share.
* Freed project-source regions are retired, returning their pages at once, only in a process that opts in
  (`retire_freed_sources`), which the `skif` binary's one-shot lint does, and only while the arena holds less than
  8 GiB. A retired range, and the rest of every slab it shares, is never reused, so the npm addon, embedders and
  long-lived hosts free regions normally and leave the address space as they found it.
* Workspace sources are also parsed by oxc for the untyped analysis; the two parses are not shared.

### Coverage and credit

* Each package is `typed`, `partial` or `untyped`, with a reason when not typed (`disabled`, `not-built`,
  `no-tsconfig`, `tsconfig-error`, `not-typescript`, `unresolved-modules`, `files-outside-programs`,
  `checker-failed`, `cancelled`). The report carries the coverage as a fact, not a diagnostic
  ([ADR 0012](0012-cli-contract-and-reporting.md)). A package whose imports do not resolve is `partial`.
* A panic inside tsrs is caught at the boundary, and the affected packages fall back to the untyped analysis with
  `checker-failed`; the release profiles unwind for this reason
  ([ADR 0015](0015-distribution-release-and-supported-versions.md)).
* The checker is asked only about reads of names that some judged declaration has as a member, for ancestors only of
  classes with an `extends` or `implements` clause, and for property-access escapes only where the name can resolve to
  an asked declaration. What it is not asked could credit nothing.
* A typed read credits what TypeScript would rename together with it: the members its symbol lands on through unions,
  intersections and mapped properties, and the members that override or implement a base or interface member it
  reads. A declaration that escapes so that any member may be read is used as a whole.
* Types never remove a syntactic use, and whole uses from types are only added, so typed enum and namespace findings
  are a subset of the untyped ones. A read through `any`, an error type or an unresolved module, and every read in a
  file the checker did not cover or in a template, is credited by name. Missing types never create a finding.
* What is judged, and every exclusion, stays decided by the declarations; only the source of uses changes.
  `explain-usage` names the typed evidence.

### Dependency policy

* Every tsrs crate is pinned with `=` to one version. An upgrade is its own change and carries the parity run, the
  typed-versus-untyped diff, the benchmark and an MSRV check.
* tsrs's `unicode-id-start` pin applies to the whole workspace lock, oxc included; an upgrade of either side checks that
  it still resolves.
* Each program is owned by a guard whose `Drop` calls tsrs's `free_unshared_program`, the one hand-written unsafe call
  in skif, allowed there with its reason.

### Address space, environment and threads

* The npm addon enables `plain-ptrs`, because Node.js holds address-space reservations of its own. The CLI keeps tsrs's
  reservation, and the facade exposes `plain-ptrs` for other hosts.
* skif passes the bundled lib path and the checker count explicitly and never sets or clears `TSRS_*` or
  `RAYON_NUM_THREADS`; variables a user sets reach tsrs, the recorded exception to the library's no-environment rule
  ([ADR 0002](0002-crates-library-api-and-upstream-code.md)).
* The checker count comes from skif's `threads` setting, and a single-threaded run uses tsrs's single-threaded mode.
* tsrs's NOTICE, with the TypeScript and DefinitelyTyped notices, ships in every artefact through `ATTRIBUTIONS.md`.

### Cost and budget

* Types off costs nothing against an untyped release: nothing of tsrs runs.
* Types on, measured on the benchmark runner, must stay within `TYPE_AWARE_BUDGET` in `tools/bench/src/run.rs`: never
  slower than knip, knip's mean over the typed run's at least 1.0 on every corpus repository where knip completes; at
  most 4 GiB peak RSS on every target; and at most 5 s on the generated 10,000-file repository. Being well ahead of
  knip with types is the goal; the summary reports the measured multiple, the check enforces only the floor. The time
  of types on against types off is published, not limited: types cost what they cost against the untyped run. The
  memory limit bounds what skif adds, checkable without running another tool.
* Every dispatched `bench` run times both modes, records their peak RSS and publishes the budget checks
  ([ADR 0014](0014-benchmarks-and-the-comparison-harness.md)). While types are opt-in the workflow does not pass
  `--check-budget`, so a broken budget shows in the summary without failing the run.
* Types become the default when typed runs meet the budget, in a change that amends this ADR and passes
  `--check-budget` in the workflow.
* `SKIF_TIMINGS` prints the type-aware stages and the checker memory per root, so a run shows where its peak comes from.

### Consequences

* Good, because member findings in typed projects are as precise as TypeScript's language service.
* Good, because a project without types loses nothing, the default run keeps the untyped speed, and one switch turns on
  the precise findings.
* Good, because hosts that do not want tsrs never build it, the addon cannot abort Node.js through a reservation, and
  upgrades are visible and measured.
* Good, because declaration files, configs and, across roots, whole programs are built once per run.
* Bad, because typed runs parse and bind the workspace sources a second time beside oxc's parse, once more for each
  program that reads a file after the last program sharing it was freed; the cost is published and budgeted, not
  avoided, and keeps types off by default.
* Bad, because the default run keeps the less accurate member findings, and long-lived hosts never get the precise ones.
* Bad, because skif takes on a large pre-release dependency with process-level constraints, the CLI binary grows by
  the checker and the embedded lib files, and the boundary copies every fact into owned types.
* Bad, because a fixed memory limit does not scale with the largest program, and a project with many `any` receivers
  gains little from types.

### Confirmation

* `crates/skif-js/tests/type_aware.rs` and `crates/skif-js/tests/type_aware_off.rs` cover the switches, the off
  default, coverage and the subset rule; `crates/skif-config/tests/config.rs` and `crates/skif-cli/src/args.rs` cover
  precedence and the flag conflicts.
* `crates/skif-js-types/tests/members.rs` and `crates/skif-js-types/tests/enums_and_namespaces.rs` cover typed credit.
* `crates/skif-js-types/tests/programs.rs`, `early_programs.rs`, `shared_files.rs`, `shared_configs.rs` and
  `shared_programs.rs` cover discovery and what the programs of a run share; `determinism.rs` holds identical facts at
  one and eight threads, and `arenas.rs` holds that repeated analyses free their arenas; `retire.rs` holds that only a process that opts
  in retires freed sources.
* `scripts/check-pins.sh` fails unless every tsrs crate is pinned with `=` to one version;
  `scripts/check-dependency-tree.sh` fails when a crate other than `skif-js-types` reaches tsrs or `skif` without
  default features does; `scripts/npm-smoke.sh` runs the addon with `typeAware` and with `noTypeAware`.
* `tools/bench/src/run.rs` holds `TYPE_AWARE_BUDGET` and its checks, and `.github/workflows/bench.yml` publishes them.
