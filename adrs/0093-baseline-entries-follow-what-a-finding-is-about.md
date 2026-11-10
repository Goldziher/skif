---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0071", "0007"]
---

# Baseline Entries Follow What a Finding Is About, and Baseline Runs Lint as the CI Run Does

## Context and Problem Statement

[ADR 0071](0071-skif-baseline-a-suppression-baseline-that-can-only-shrink.md) records a finding as `name@specifier`
whenever the finding carries the specifier, on the assumption that only version rules do. `types-in-dependencies`,
`unsorted-dependencies`, `local-path-dependency`, `workspace-protocol` and the catalog rules carry it too. A baselined
`@types/node@^22.0.0` in the `dependencies` of a private package then fails the run again after a bump to `^22.1.0`,
the next `write` records the new specifier, and `check --base origin/main` reports growth: a pull request that only
bumped a dependency fails the ratchet. Fixing a version mismatch reopens a baselined `unsorted-dependencies` finding
the same way.

ADR 0071 also accepted that `unused-files` and `unused-exports` findings record a package-wide entry, so every later
unused file or export of that package passes and `check` sees no growth. The baseline commands lint with none of the
lint flags (a CI run of `skif --strict` or `--production` gets a baseline of other findings) and cannot cover a
directory of several roots, which `skif .` lints. And a root `skif.toml` with `reset = ["ignore"]` drops the entries of
the baseline it extends, while `write` and `prune` report success.

How narrow is an entry for a finding that is not about a version, or not about a dependency at all, and which lint run
do the commands record against?

## Decision Drivers

* A bump of a dependency must not reopen a baselined finding that is not about its version
* A new unused file or export must fail the run even where older ones are baselined
* The baseline is recorded against the findings of the run it protects
* The same matching code decides for the lint run and the baseline commands (ADR 0071)
* Rule metadata, not a list in the CLI, says what a rule's finding is about

## Considered Options

* A `version_scoped` flag on `RuleMeta` and a `files` selector on `[[ignore]]`
* A list of version rules in `skif baseline`, and file and export findings left unrecorded
* `check` treating an entry that replaces another of the same rule, package and dependency name as changed, not added

## Decision Outcome

Chosen option: "A `version_scoped` flag on `RuleMeta` and a `files` selector on `[[ignore]]`", because the rule that
emits a finding knows what it is about, and a `files` selector narrows a file finding exactly with the matching code
the run already uses.

### What an entry records

* `RuleMeta` gains `version_scoped` (set with `RuleMeta::version_scoped()`, not serialized). It marks the rules whose
  finding is about the specifier a dependency is written with: `version-mismatch`, `semver-range-mismatch`,
  `policy-conflict`, `unsynced-dependency-family`, `incompatible-peer-dependencies`, `catalog-mismatch` and
  `catalog-version-divergence`. A finding of one of them is recorded as `name@specifier`, as before. Any other
  finding with a dependency is recorded by the dependency's name alone.
* `[[ignore]]` gains `files`: globs matched against the path of the finding's location, relative to the workspace root
  as diagnostics report it. They are not anchored at the declaring file, like the other `[[ignore]]` selectors and
  unlike the `files` of `[[overrides]]`. An entry with only `files` is valid. The key is omitted from JSON when empty.
* A finding about no dependency that is located in a file other than a `package.json` (`unused-files`,
  `unused-exports`, `circular-imports`, ...) is recorded with its package and that file. In a root package without a
  name of its own, which ADR 0071 leaves unrecorded, the file alone narrows the entry, so it is recorded. Entries are
  compared, sorted and deduplicated by rules, packages, dependencies and files.

### Which run the commands record against

* `write` and `prune` take the lint flags that change the findings (`--config`, `--rule`, the ignore flags, `--no-js`,
  `--production`, `--strict`, `--no-gitignore`, `--include-entry-exports`, the tsconfig flags, `--select`, `-W`,
  `--include`, `--exclude` and its shortcuts, `--tags`, the instance filters), applied as a lint run applies them. A
  filter that narrows the report makes `prune` remove the entries of the findings it leaves out.
* All three commands resolve roots as a lint run does: `-r` covers every root below `PATH`, and so does a `PATH` without
  a `package.json` at or above it. Each root has its own `skif.baseline.toml`; `--file` names the baseline of a single
  root and is a usage error with several, as is `check --base-file`. The roots are linted together, so roots credit each
  other's uses as in `skif -r`. With several roots the JSON document lists one member per root under `roots`.
* `write` resolves the root's configuration again after writing, and `prune` checks it before linting: when no
  configuration file of the run extends the baseline, or its entries are not all in the merged `[[ignore]]` list (a
  `reset = ["ignore"]` applied after it), the command exits `2` naming the cause.

### Consequences

* Good, because a dependency bump leaves a baselined `types-in-dependencies` or `unsorted-dependencies` finding
  baselined, and `check` sees no growth.
* Good, because a baselined unused file or export covers that file only.
* Good, because the baseline of a CI job that runs with flags matches what the job reports.
* Good, because a baseline the configuration does not apply is an error instead of a silent no-op.
* Bad, because a file entry still covers every finding of its rule in that file, so a new unused export in a file
  that already has a baselined one passes.
* Bad, because a version-scoped finding still moves to a new entry when a still misaligned member is bumped.
* Bad, because `RuleMeta` gains a field that every rule outside skif must set when it reports specifier findings.

### Confirmation

* `crates/skif-rules/tests/rules.rs` pins the set of version-scoped rules.
* `crates/skif-lint/src/ignore.rs` tests that a `files` selector matches the location relative to the root.
* `crates/skif-cli/tests/baseline.rs` runs the binary: a baselined `@types/node` survives a bump; an unused file is
  recorded with `files`, and a new one fails; `--production` decides what is recorded; two roots below a directory
  without a manifest get one baseline each; `--file` with two roots is a usage error; `reset = ["ignore"]` fails
  `write` and `prune` with exit `2`.

## Pros and Cons of the Options

### A `version_scoped` flag on `RuleMeta` and a `files` selector on `[[ignore]]`

* Good, because a rule added later says itself whether its finding is about a version.
* Good, because `files` is useful in hand-written entries too.
* Bad, because the configuration model gains a key.

### A list of version rules in `skif baseline`, and file and export findings left unrecorded

* Good, because nothing outside the CLI changes.
* Bad, because the list drifts from the rules, and teams adopting the JS rules, who need the baseline most, could not
  baseline those findings.

### `check` treating a replacing entry as changed

* Good, because a bump of a misaligned version would not count as growth.
* Bad, because the lint run still fails on the bumped finding until `write` runs again, and a real new mismatch of the
  same dependency in the same package would pass the ratchet.
