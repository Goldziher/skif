---
status: accepted
date: 2026-11-04
decision-makers: ["Na'aman Hirschfeld"]
---

# Harness Known Deviations and Comparison Scope: Where skif Differs From syncpack and knip by Design

## Context and Problem Statement

[ADR 0009](0009-version-and-catalog-policies.md) fixes skif's policy semantics to syncpack's and states a few departures in passing, but lists no differences and does not name the default dependency scope. [ADR 0026](0026-parity-and-diff-tasks-on-a-pinned-corpus.md) adds the corpus comparison, and its first runs showed that raw one-sided counts mix three things: intended differences, harness mistakes (syncpack and knip run in a different scope than skif) and real defects. With hundreds of intended differences in the counts, a drift check on them hides the defects. Both records are locked, so the corrections live here.

## Decision Drivers

* A one-sided count must mean "unexplained" so that a gate on it is meaningful
* The harness must compare the tools in the same scope, not in each tool's default
* Intended differences must be named, justified and countable, not silently filtered
* Locked records stay unedited

## Considered Options

* Name the intended differences, classify them from evidence in the harness, and align the comparison scope
* Edit ADR 0009 and ADR 0026
* Keep raw counts and widen the drift tolerance

## Decision Outcome

Chosen option: "Name the intended differences, classify them from evidence in the harness, and align the comparison scope", because it keeps the records immutable and makes the remaining one-sided counts mean something.

**Default dependency scope (ADR 0009).** Without `[policy] dependency-types`, skif's version, semver-range and catalog policies cover `dependencies` and `devDependencies`. Peer and optional ranges state a compatibility window, so aligning them is opt-in. A package's own version always joins the policies. `skif-bench syncpack-diff` therefore runs syncpack with `--dependency-types prod,dev,local` (the translated list plus `local` for a fixture that sets one): plain `prod,dev` would drop syncpack's `local` instance and with it the own-version comparison.

**Known deviations from syncpack.** A one-sided finding is a known deviation when the manifests show it is one of these:

| Id | Difference |
| --- | --- |
| `satisfied-local-range` | A range the local version satisfies (`^5.1.0` for local `5.1.4`, `*`) is valid for skif. syncpack requires byte equality with the local version for a non-workspace specifier. |
| `missing-local-version` | A package without a `version`, or with an invalid one such as `0.0.0-PLACEHOLDER`, is not a local instance for skif. syncpack reports `InvalidLocalVersion` and `DependsOnInvalidLocalPackage`. |
| `floating-specifier` | `latest` and `*` never become the alignment target; skif reports an unfixable `NonSemverMismatch` on the floating side. syncpack ranks them highest and flags the instance against itself. |
| `shorthand-ranking` | skif ranks ranges by the lowest version they admit, so `^14` never outranks `^14.1.2`. syncpack lets the shorthand win. |
| `npm-alias` | `npm:other@x` is a separate dependency for skif and is not aligned with the plain dependency. |
| `unread-by-syncpack` | skif follows the workspace pattern semantics of the package manager (trailing slashes, dot-directories); syncpack does not read those packages, so findings in them have no counterpart. |

Peer and optional dependencies are not catalogued and peer windows are not aligned; the shared default scope keeps them out of the comparison.

**Comparison scope for syncpack.** The corpus runs use an empty syncpack configuration, because skif ignores syncpack settings and a legacy `syncpack` key in a `package.json` makes syncpack 15 refuse to run. Fixture configuration paths are absolute. syncpack has no Rush support, so for a repository with a `rush.json` every `projectFolder` is passed as a `--source`. The policy rules compared include `unresolved-catalog-references`.

**Comparison scope for knip.** knip runs with an empty configuration file, so a repository's own ignores and disabled issue types do not hide findings; `--repo-knip-config` restores them for a look, and neither it nor `--include-test-files` may be combined with a baseline. Enum and namespace members are keyed by the member name, without the `Enum.` or `ns.` qualifier on either side. knip's stdout may carry banners and log lines around its JSON; the first object that is a knip report is used, and a failure names the parse error rather than an unrelated stderr line. A repository sets `recursive = true` in `corpus.toml` where nested roots are not workspace members (skif then runs with `--recursive`; paths stay relative to the start directory), and may set `install-command` and `install-timeout-secs`.

**Known deviations from knip.** In the `files` category, a file only knip reports is a known deviation when it is outside skif's project set (`non-code-file`: styles, markup, data, docs), in a fixture directory skif skips (`fixture-file`) or a test file skif treats as an entry (`test-file`). The last two count as ordinary differences with `--include-test-files`.

**Reporting and drift.** Known deviations are removed from the one-sided counts, reported per target and deviation id in their own column and section with the reason, and recorded in the baselines as counts under `known_deviations`. The drift check does not compare them. What remains one-sided is unexplained.

### Consequences

* Good, because a one-sided count is a lead worth reading, and a regression in it fails the check.
* Good, because every intended difference has a reason in one place.
* Bad, because the classification reads manifests and is itself an approximation (it can hide a real defect that looks like a deviation), so the deviations are counted and visible, not dropped.
* Bad, because a deviation that is later fixed in skif must be removed from the table here and in the harness.

### Confirmation

* The `skif-bench` unit tests cover the classification of each syncpack deviation from manifests, the file classification, the enum and namespace key stripping, the tolerant parse of knip's stdout, the `rush.json` reader and the dependency-type translation.
* `mise run parity` and `mise run diff:syncpack` print the known deviations apart from the counts.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Name the intended differences, classify them from evidence in the harness, and align the comparison scope

* Good, because the gate is on the unexplained rest.
* Bad, because the classifier must be kept in step with skif's behaviour.

### Edit ADR 0009 and ADR 0026

* Good, because the facts would sit next to the decisions they correct.
* Bad, because it breaks the immutability that `adrlint.lock` enforces.

### Keep raw counts and widen the drift tolerance

* Good, because it needs no work.
* Bad, because a tolerance wide enough for hundreds of intended differences hides real regressions.

## More Information

* Amends: [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0026](0026-parity-and-diff-tasks-on-a-pinned-corpus.md)
* Related decisions: [ADR 0023](0023-root-discovery-semantics-nested-roots-rush-and-fixture-directories.md)
* See the [ADR index](index.md).
