---
status: accepted
date: 2026-11-16
decision-makers: ["Na'aman Hirschfeld"]
---

# Mission, Scope and Non-Goals

## Context and Problem Statement

A JavaScript monorepo needs three kinds of dependency hygiene, and each usually comes from a different tool with its own
runtime, configuration format and speed:

1. Workspace and manifest rules: every package has a `package.json`, workspace patterns match something, the root is
   `private`, dependencies are sorted. sherif covers this, in Rust, with a fixed rule set.
2. Version policy: one version of a dependency across the repository, pinned ranges, `workspace:` for internal
   packages, catalogs. syncpack covers this with version and semver groups.
3. Usage analysis: dependencies listed but never imported, imports of unlisted packages, unused files, exports and
   members. knip covers this as a Node CLI that loads tool configurations by executing them.

Running all three means two Node toolchains, three configuration formats and three start-ups, and none of them embeds
in another linter as a library. What is skif, what does it cover, and what does it refuse to do?

## Decision Drivers

* One run, one configuration file and one diagnostic model for all three kinds of check
* An embeddable library, not a binary whose rules print, prompt and exit
* Safe on untrusted pull requests: no execution of user JavaScript and no network access while linting
* Measurable performance targets
* A release ships what the tool has, not a staged plan

## Considered Options

* One library-first Rust dependency linter covering all three kinds of check
* Extend sherif and keep its configuration and command line, rejected because its rules print, prompt and exit, so
  nothing embeds
* Wrap syncpack and knip and merge their output, rejected because it needs Node, executes configurations and cannot meet
  the targets
* Three separate Rust tools, rejected because the checks share discovery, specifiers, configuration, diagnostics and
  fixes

## Decision Outcome

Chosen option: "One library-first Rust dependency linter covering all three kinds of check", because it replaces three
tools, three runtimes and three configuration formats with one library, one configuration file and one diagnostic model,
and it can promise never to execute user code.

### What skif is

* One Rust tool, built as a library first with a thin CLI
  ([ADR 0002](0002-crates-library-api-and-upstream-code.md)), with three kinds of check:
  * sherif-style manifest and workspace rules;
  * syncpack-style version, semver-range and catalog policies
    ([ADR 0006](0006-version-policies-lockfiles-toolchains-and-updates.md));
  * knip-style JavaScript and TypeScript analysis on oxc ([ADR 0007](0007-js-analysis-core-on-oxc.md)), without a
    type checker ([ADR 0010](0010-type-aware-analysis-on-tsrs.md)).
* All three share one configuration file
  ([ADR 0003](0003-configuration-suppressions-baselines-and-migration.md)), one workspace model
  ([ADR 0004](0004-workspaces-roots-and-package-managers.md)) and one diagnostic model
  ([ADR 0005](0005-rules-diagnostics-and-fixes.md)).
* skif is a clean rebuild, not a sherif release. It reads no `sherif` key in `package.json`, accepts no sherif rule id it
  renamed and offers no sherif command-line compatibility. Code is ported only where it is still the best
  implementation, and ported code is credited ([ADR 0002](0002-crates-library-api-and-upstream-code.md)).

### What skif never does

* It never executes user JavaScript. Tool configurations are read from the static AST only
  ([ADR 0008](0008-plugins-config-evaluation-and-commands.md)). skif never spawns Node, never `require`s a config and
  never runs a package script.
* It opens no network connection and resolves no version while linting or fixing. `skif update` is the one command that
  contacts the package registry, and only when invoked
  ([ADR 0006](0006-version-policies-lockfiles-toolchains-and-updates.md)). The library has no HTTP client.
* The library starts no process. The CLI starts exactly two kinds: the package manager's install after a fix that
  changed files, which can be turned off ([ADR 0012](0012-cli-contract-and-reporting.md)), and `git show` to read a
  baseline at a revision for `skif baseline check --base`
  ([ADR 0003](0003-configuration-suppressions-baselines-and-migration.md)).
* It has no compatibility reader for another tool's configuration. `skif migrate` translates a sherif, syncpack or knip
  configuration once ([ADR 0003](0003-configuration-suppressions-baselines-and-migration.md)).

### Lockfiles and formatting

* The presence of a lockfile helps detect the package manager
  ([ADR 0004](0004-workspaces-roots-and-package-managers.md)). Lockfile contents are read on demand, only by the rules
  and policy scopes that need resolved versions, and only for `pnpm-lock.yaml` in lockfile format 9 and `bun.lock`
  ([ADR 0006](0006-version-policies-lockfiles-toolchains-and-updates.md)).
* Formatting is out of scope, except `package.json`: the opt-in `package-json-format` rule and `skif format`
  ([ADR 0005](0005-rules-diagnostics-and-fixes.md)). Fixes preserve the layout of the files they edit.

### Targets

Measured by the benchmark harness ([ADR 0014](0014-benchmarks-and-the-comparison-harness.md)):

* the manifest and policy rules on 500 packages in under 50 ms;
* JS analysis at least 5x faster than knip on the same corpus repository;
* parsing and building a module graph of 10,000 files in under 2 s.

Parity with knip and syncpack is measured as drift on a pinned corpus and is never a release gate
([ADR 0014](0014-benchmarks-and-the-comparison-harness.md)).

### Releases

Releases are not staged. A release contains everything the tool has; no record or document promises a feature for a
later release ([ADR 0015](0015-distribution-release-and-supported-versions.md)).

### Consequences

* Good, because one tool, one configuration and one report replace three tools, and a CI pipeline drops two Node
  toolchains.
* Good, because a host such as poly gets the same rules as the CLI, in process, with no output parsing.
* Good, because refusing to execute JavaScript or use the network makes skif safe on untrusted pull requests and
  deterministic across machines.
* Bad, because re-implementing syncpack and knip semantics means tracking two moving upstreams; the comparison harness
  measures the drift rather than assuming parity.
* Bad, because static configuration reading cannot see values computed at runtime, so some configurations are
  understood only in part, and skif says so instead of guessing.
* Bad, because sherif users migrate their configuration once; `skif migrate` does it for them.

### Confirmation

* `skif-bench run` (`mise run bench`) measures every target above and exits non-zero when one fails.
* `crates/skif-cli/tests/cli.rs` runs the CLI with a recording package-manager shim on `PATH` and asserts that the
  install runs only after a fix that changed files.
* `scripts/check-dependency-tree.sh` keeps oxc, the parser of user JavaScript and TypeScript, behind the JS analysis
  crates.
