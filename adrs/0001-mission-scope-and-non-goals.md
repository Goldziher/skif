---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Mission, Scope and Non-Goals

## Context and Problem Statement

A JavaScript monorepo needs three kinds of dependency hygiene, and each usually comes from a different tool with its own
runtime, configuration format and speed:

1. Workspace and manifest rules: every package has a `package.json`, workspace patterns match something, the root is
   `private`, dependencies are sorted, `@types/*` sit in `devDependencies`. sherif covers this, in Rust, with a fixed
   rule set.
2. Version policy: one version of `react` across the repository, pinned ranges for some dependencies, `workspace:` for
   internal packages, catalogs where they exist. syncpack covers this with version and semver groups.
3. Usage analysis: dependencies listed but never imported, imports of packages that are not listed, unused files,
   exports and members. knip covers this as a Node CLI that loads tool configurations by executing them.

A repository that runs all three installs two Node toolchains, keeps three configuration formats and pays three
start-ups per run. None of the three can be embedded in another linter as a library. What is skif, what does it cover,
and what does it refuse to do?

## Decision Drivers

* One run, one configuration file and one diagnostic model for manifest rules, version policy and usage analysis
* An embeddable library, not a binary whose rules print, prompt and exit
* Safe on untrusted pull requests: no execution of user JavaScript and no network access while linting
* Measurable performance targets
* A release ships what the tool has, not a staged plan

## Considered Options

* One library-first Rust dependency linter covering all three kinds of check
* Extend sherif and keep its configuration and command line
* Wrap syncpack and knip and merge their output
* Three separate Rust tools, one per kind of check

## Decision Outcome

Chosen option: "One library-first Rust dependency linter covering all three kinds of check", because it replaces three
tools, three runtimes and three configuration formats with one library, one configuration file and one diagnostic model,
and it can promise never to execute user code.

### What skif is

* One Rust tool, built as a library first ([ADR 0003](0003-library-api-and-embedding.md)) with a thin CLI, with three
  kinds of check:
  * sherif-style manifest and workspace rules;
  * syncpack-style version, semver-range and catalog policies
    ([ADR 0010](0010-version-semver-range-and-catalog-policies.md));
  * knip-style JavaScript and TypeScript analysis on oxc
    ([ADR 0013](0013-js-analysis-core-oxc-module-graph-and-resolution.md)).
* All three share one configuration file ([ADR 0004](0004-configuration-model-and-layering.md)), one workspace model
  ([ADR 0006](0006-workspace-discovery-roots-and-multi-root-runs.md)) and one diagnostic model
  ([ADR 0008](0008-rule-and-diagnostic-model-rule-ids-and-severities.md)).
* skif is a clean rebuild, not a sherif release. It reads no `sherif` key in `package.json`, accepts no sherif rule id
  that it renamed and offers no sherif command-line compatibility. Code is ported only where it is still the best
  implementation, and ported code is credited ([ADR 0033](0033-attribution-and-vendoring.md)).

### What skif never does

* It never executes user JavaScript. Tool configurations are read from the static AST only
  ([ADR 0016](0016-config-evaluation-and-incomplete-entries.md)). skif never spawns Node, never `require`s a config and
  never runs a package script.
* It opens no network connection, except in `skif update`, which asks the package registry when invoked
  ([ADR 0026](0026-skif-update.md)).
* The library spawns no process. The CLI spawns exactly two kinds: the package manager's install after `--fix`, which
  can be turned off ([ADR 0024](0024-cli-contract.md)), and `git` to read a baseline at a revision for
  `skif baseline check --base` ([ADR 0023](0023-baselines.md)).
* It has no compatibility reader for another tool's configuration. `skif migrate` translates a sherif, syncpack or knip
  configuration once ([ADR 0025](0025-skif-migrate.md)).

### Lockfiles and formatting

* The presence of a lockfile detects the package manager ([ADR
  0006](0006-workspace-discovery-roots-and-multi-root-runs.md)). Lockfile contents are read on demand only by the rules
  and policy scopes that need resolved versions: `pnpm-lock.yaml` in lockfile format 9 and `bun.lock` ([ADR
  0011](0011-lockfile-and-toolchain-rules.md)).
* Formatting is out of scope, except `package.json`: the opt-in `package-json-format` rule and `skif format`
  ([ADR 0009](0009-fixes-edits-and-package-json-formatting.md)). Fixes preserve the layout of the files they edit.

### Targets

Measured as [ADR 0029](0029-performance-caching-and-benchmarks.md) describes:

* the manifest and policy rules on 500 packages in under 50 ms;
* JS analysis at least 5x faster than knip on the same corpus repository;
* parsing and building a module graph of 10,000 files in under 2 s.

Parity with knip and syncpack is measured as a drift number on a pinned corpus and is never a release gate
([ADR 0021](0021-knip-and-syncpack-comparison-harness.md)).

### Releases

Releases are not staged. A release contains everything the tool has; no record or document promises a feature for a
later release ([ADR 0027](0027-distribution-and-release.md)).

### Consequences

* Good, because one tool, one configuration and one report replace three tools, and a CI pipeline drops two Node
  toolchains.
* Good, because a host such as poly gets the same rules as the CLI, in process, with no output parsing.
* Good, because refusing to execute JavaScript makes skif safe on untrusted pull requests and deterministic across
  machines.
* Bad, because re-implementing syncpack and knip semantics means tracking two moving upstreams; the comparison harness
  measures the drift rather than assuming parity.
* Bad, because static configuration reading cannot see values computed at runtime, so some configurations knip
  understands are understood only in part, and skif says so instead of guessing.
* Bad, because sherif users migrate their configuration once; `skif migrate` does it for them.

### Confirmation

* `skif-bench run` (`mise run bench`) measures every target above and exits non-zero when one fails
  ([ADR 0029](0029-performance-caching-and-benchmarks.md)).
* `install_is_not_run_without_fix_without_changes_or_with_no_install` in `crates/skif-cli/tests/cli.rs` runs the CLI
  with a recording package-manager shim on `PATH` and asserts that the install runs only after a fix that changed files.
* `scripts/check-dependency-tree.sh` keeps oxc, the only parser of user JavaScript, behind the JS analysis crates, and
  no crate depends on a JavaScript runtime.

## Pros and Cons of the Options

### One library-first Rust dependency linter covering all three kinds of check

* Good, because one workspace discovery, one specifier model, one configuration and one fix pipeline serve all three
  kinds of check, so every rule is configured, suppressed and fixed the same way.
* Good, because one native binary parallelises across checks and files, which the targets need.
* Bad, because the scope is large: two upstream specifications and a plugin ecosystem to follow.

### Extend sherif and keep its configuration and command line

* Good, because existing sherif users keep their setup.
* Bad, because sherif's rules print, prompt, exit and write files at paths relative to the working directory, so
  nothing could be embedded, and fixing that is the rebuild.

### Wrap syncpack and knip and merge their output

* Good, because it reuses two mature tools with their full coverage.
* Bad, because it needs Node, pays two start-ups and knip's configuration execution on every run, and cannot meet the
  targets or be embedded.

### Three separate Rust tools, one per kind of check

* Good, because each tool stays small.
* Bad, because the checks share discovery, specifiers, configuration, diagnostics and fixes; splitting them duplicates
  all of it and gives the user three configurations again.
