---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Supported Versions and the End-to-End Matrix: Upstream End of Life, Tested Layouts

## Context and Problem Statement

skif runs on Node.js and Bun through the npm addon, is built with Rust, and analyses workspaces of npm, pnpm, Yarn
(Classic and 2+) and Bun. skif's view of a workspace depends on the installed layout: it reads installed packages
from `node_modules` when there is one, from the lockfile under Plug'n'Play, and decides member binaries from the
linker ([ADR 0012](0012-package-manager-semantics-pnpm-yarn-bun-npm-rush.md)). Without a rule, a version floor is
raised arbitrarily or never, and every old toolchain or untested layout becomes an implicit promise.

Which versions does skif support, and how does CI prove it?

## Decision Drivers

* A supported version or layout is one that CI installs and lints; a promise without a test is not a promise
* Upstream knows best when it stops fixing a version, and the rule must also cover upstreams without a policy
* Older versions that happen to work are not blocked
* The matrix stays bounded: one job per manager, version slot and operating system
* Scenarios are shared across managers, and a difference between linkers is asserted where it occurs

## Considered Options

* Support what upstream still supports (else the latest two majors), and test every shipped linker, the newest slot
  on Linux, macOS and Windows
* Support every version back to a fixed date
* Support only the latest version of everything, on default layouts and Linux

## Decision Outcome

Chosen option: "Support what upstream still supports (else the latest two majors), and test every shipped linker,
the newest slot on Linux, macOS and Windows", because it ties the matrix to facts someone else maintains and verifies
the code paths for every layout and operating system that skif has.

### The rule

* skif supports a version of a runtime, language or tool only while it is not past its upstream end of life. Where
  upstream publishes no end-of-life policy, skif supports the latest two majors.
* A version is dropped in the first release after it leaves support, with a `Changed` entry in `CHANGELOG.md`.
* Support means CI tests it. An older version may work but is not tested and not blocked (`engines` fields are
  advisory and skif never refuses to run), and a bug that reproduces only on it is not fixed.

### Applied

* Node.js: the non-EoL even-numbered lines per this support policy; odd-numbered lines never. The floor is Node 22,
  so `engines.node` is `>=22`, and it is raised when Node 22 reaches end of life.
* Bun: 1.x, tested on the newest release; the package declares `>=1`.
* Rust: the MSRV is `rust-version` in the workspace `Cargo.toml`; CI builds on it and on stable, and raising it is a
  `Changed` entry.
* npm, pnpm and Yarn 2+: the latest two majors.
* Yarn Classic: its newest 1.x, tested and fixed. It has one major that upstream maintains, so the two-majors rule
  does not apply; it leaves support when upstream announces its end of life.
* TypeScript and oxc follow the oxc policy of
  [ADR 0013](0013-js-analysis-core-oxc-module-graph-and-resolution.md).

### The end-to-end matrix

* One job per manager and slot: npm, pnpm and Yarn 2+ on the newest and the previous major, Yarn Classic and Bun on
  their newest 1.x. `scripts/e2e/resolve-pm-version.sh` resolves the versions from the registry.
* The newest slot of every manager runs on Linux, macOS and Windows; the previous majors run on Linux.
* Each job runs every scenario under every linker of its manager: npm hoisted; pnpm isolated and `nodeLinker:
  hoisted`; Yarn 2+ Plug'n'Play, `nodeLinker: node-modules` and `nodeLinker: pnpm`; Yarn Classic hoisted; Bun's
  default, `linker = "hoisted"` and `linker = "isolated"`. npm's `install-strategy=linked` is read by skif
  ([ADR 0017](0017-scripts-binaries-and-working-directories.md)) but not in the matrix.
* A scenario under `fixtures/e2e/<name>` holds the files every manager shares, overlays per manager and an
  expectation with overrides per manager and per linker. The runner writes each manager's own forms and sets
  `packageManager` to the version under test. Each install is checked for the layout its linker produces, so a
  scenario cannot pass on the wrong one.
* A difference between linkers is an override that states its reason. An override that records a skif bug rather
  than the correct answer names the issue in `known_issue`, which the runner prints apart from passing checks.

### Where the versions are documented

The installation guide of the website has the "Supported versions" section. The README states the managers and the
tested linkers in its support table and links to that section. `scripts/check-support-matrix.sh` builds the expected
text from the CI matrix, the linker list and the npm addon runtimes, and fails when the README's support table, the
comparison guide or the installation guide's table differs.

### Consequences

* Good, because the test matrix follows upstream and shrinks on a schedule.
* Good, because users on a slightly older toolchain are not locked out.
* Good, because Yarn Classic, every shipped linker, macOS and Windows are installed and linted on every change, and
  the previous-major slots test the version they name.
* Good, because a scenario is written once and generated per manager, and the documented matrix cannot drift from CI.
* Bad, because "latest two majors" for a tool with no policy is skif's judgement, not upstream's.
* Bad, because the matrix is large, and a finding Yarn Classic reports differently from npm becomes a bug to fix
  rather than a known gap.

### Confirmation

* `npm-package/package.json` declares `engines.node` as `>=22` and `engines.bun` as `>=1`.
* The `verify_npm` job of `.github/workflows/publish.yaml` runs the published package under every supported Node line
  and the newest Bun.
* The `e2e` job of `.github/workflows/ci.yml` lists the managers, slots and operating systems, with the exclusions
  above, and `e2e_linkers` in `scripts/e2e/lib.sh` the linkers.
* `scripts/test-e2e-scripts.sh` checks the slots of `resolve-pm-version.sh`, the generated projects of every manager
  and linker, and that every scenario's expectation evaluates.
* `scripts/check-support-matrix.sh` passes.

## Pros and Cons of the Options

### Support what upstream still supports (else the latest two majors), and test every shipped linker, the newest slot on Linux, macOS and Windows

* Good, because the support statement is a test result, not a promise.
* Bad, because each manager, slot and operating system is a CI job to keep green.

### Support every version back to a fixed date

* Good, because users know exactly how far back skif goes.
* Bad, because skif would test versions their own upstream no longer fixes, and the date needs its own policy.

### Support only the latest version of everything, on default layouts and Linux

* Good, because the matrix is small.
* Bad, because most repositories run an older major, a non-default linker or another operating system, and those
  code paths would be unverified.
