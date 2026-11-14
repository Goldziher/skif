---
status: accepted
date: 2026-11-14
decision-makers: ["Na'aman Hirschfeld"]
---

# Type-Aware Analysis on tsrs: On by Default, Switchable, Untyped Where a Project Has No Types

## Context and Problem Statement

skif's JS analysis is built on oxc ([ADR 0013](0013-js-analysis-core-oxc-module-graph-and-resolution.md)): a parser,
a resolver and a module graph, with no type checker. Most findings need none: a file is reached or not, a package is
imported or not. Member findings do. `unused-class-members` ([ADR 0042](0042-unused-class-members.md)) credits a
member when any module reads a property of its name, so `a.dispose()` keeps every `dispose` in the project alive, and
a member reached only through an interface, an override or a typed receiver is judged by name alone. Enum and
namespace members ([ADR 0018](0018-export-namespace-and-member-rules.md)) have the same limit when an enum is passed
through an alias or a namespace is read through `this`.

tsrs is a Rust port of the TypeScript 7 compiler (Apache-2.0, crates `tsrs_*` on crates.io) with its checker. With it
skif can ask which declaration a read reaches.

Should skif be type-aware, and how, without losing what makes it fast and without failing on projects that have no
types?

## Decision Drivers

* Accuracy of member findings: a finding is worth less than the trust a false one costs
* Speed stays a product property: type awareness may cost time, and the cost is published, never hidden
* A project without types (no `tsconfig.json`, dependencies not installed, plain JavaScript) still gets every finding
  it gets today
* oxc stays the source of the module graph and of every syntax fact; types add answers, they do not replace the graph
* Hosts that embed skif (poly, the npm addon, the language server) can leave the checker out entirely

## Considered Options

* tsrs alongside oxc, on by default, switchable off, untyped per package where types are missing
* tsrs, opt-in (off by default)
* Type information from `tsc` or `tsserver` run as a child process
* No type awareness: improve the name-based heuristics only

## Decision Outcome

Chosen option: "tsrs alongside oxc, on by default, switchable off, untyped per package where types are missing",
because it gives the accurate answer wherever a project has types, keeps today's analysis everywhere else, and lets a
user trade accuracy for speed in one switch.

### What runs

* A new crate, `skif-js-types`, is the only crate that depends on tsrs ([ADR 0045](0045-tsrs-dependency-policy.md)).
  Its API takes the judged files per package and a query (the declarations whose member reads skif judges) and returns
  owned facts: typed member reads, uses of a declaration as a whole, reads it could not settle, the files it covered
  and the type coverage of every package. No tsrs type appears in it.
* Declarations are joined on (file, UTF-8 byte offset of the declaration's name), the key oxc's spans give.
* Programs follow TypeScript's own discovery: each judged file belongs to its nearest `tsconfig.json` (or
  `jsconfig.json`); a solution config is followed through its references to the config whose `include` or `files`
  holds the file; `[js] tsconfig` replaces discovery for the root package. Each config is built once. A file in no
  program is analysed without types; skif builds no inferred program.
* The facts are copied out and every program's memory is freed before the rules run.

### What it changes

* `unused-class-members` takes its member uses from the typed source in every covered file and from the name-based
  source elsewhere (Vue, Svelte and Astro templates keep the name-based source and the framework exemptions).
* `unused-enum-members` and `unused-namespace-members` add typed reads and typed whole uses to the syntactic ones.
  Types never remove a syntactic use, so with types on these findings are a subset of the findings with types off.
* `explain-usage` names the typed evidence (the read and the declaration it reaches).
* Reads through `any`, an error type or an unresolved module are credited by name, as without types
  ([ADR 0046](0046-typed-member-semantics.md)).

### On by default, switchable off

* `[js] type-aware = true` is the default; `[js] type-aware = false`, `[js.workspaces."<dir>"] type-aware`, the CLI
  flag `--no-type-aware` and the npm option `noTypeAware` turn it off. With it off nothing of tsrs runs and no memory
  is reserved for it.
* The documentation states the trade-off: type awareness is slower and more accurate, with the measured cost from the
  benchmark workflow ([ADR 0047](0047-performance-budget-with-types.md)).
* The Cargo feature `type-aware` of the facade is off by default, as `js-analysis` is; the CLI enables it by default
  and the npm addon always. A build without it reports every package untyped with the reason `not-built`.
* The language server, `skif mcp` and `--watch` run untyped until repeated analyses are shown to free their memory
  ([ADR 0045](0045-tsrs-dependency-policy.md)).

### Fallback and reporting

* Per package, coverage is `typed`, `partial` or `untyped` with a reason: `disabled`, `not-built`, `no-tsconfig`,
  `tsconfig-error`, `not-typescript`, `unresolved-modules: N`, `files-outside-programs: N`, `checker-failed`,
  `cancelled`.
* The JSON report carries a `type-aware` object (enabled, engine, packages); SARIF carries it in the run properties;
  the human report prints one footer line when a package is partial or untyped. Missing types are no diagnostic.
* A checker failure (a panic inside tsrs) is caught; the affected packages fall back to the untyped analysis with
  `checker-failed`.

### Consequences

* Good, because member findings in typed projects become as precise as the checker, the precision knip 5 had through
  TypeScript's language service.
* Good, because a project without types loses nothing, and one switch restores today's speed.
* Bad, because typed runs read and bind every file of each program a second time, beside oxc's parse; the cost is
  published, not avoided.
* Bad, because skif takes on a large pre-release dependency with process-level constraints
  ([ADR 0045](0045-tsrs-dependency-policy.md)).
* Neutral: oxc stays the only parser of the module graph and the syntax facts, so
  [ADR 0013](0013-js-analysis-core-oxc-module-graph-and-resolution.md) holds for everything but member usage in typed
  files.

### Confirmation

* `typed_mode_only_removes_enum_and_namespace_findings`: on the fixtures, enum and namespace findings with types on
  are a subset of those with types off.
* `no_type_aware_reserves_nothing`: with types off no tsrs code runs (the arena counter stays at zero).
* Fixtures with a real `tsconfig.json` for every case of [ADR 0046](0046-typed-member-semantics.md), plus no
  `tsconfig`, uninstalled dependencies, a `tsconfig` parse error, project references and a solution config, each
  asserting the coverage state and reason.
* CLI snapshots of the JSON `type-aware` key and the footer line; the npm smoke test runs with and without
  `noTypeAware`.
* `scripts/check-dependency-tree.sh` fails when a crate other than `skif-js-types` depends on a tsrs crate or the
  facade without default features pulls one in.

## Pros and Cons of the Options

### tsrs alongside oxc, on by default, switchable off

* Good, because the accurate answer is the default where it is available.
* Good, because untyped packages keep today's analysis.
* Bad, because the default run is slower than today's.

### tsrs, opt-in

* Good, because the default run keeps today's speed.
* Bad, because most users never turn it on and keep the less accurate member findings.

### `tsc` or `tsserver` as a child process

* Good, because it is the reference implementation.
* Bad, because it needs Node.js and an installed `typescript`, starts slowly and answers over a protocol built for
  editors; skif's binary would no longer be self-contained.

### No type awareness

* Good, because it adds nothing.
* Bad, because the false positives of name-based member usage stay, and some are not fixable without types.
