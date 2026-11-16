---
status: accepted
date: 2026-11-16
decision-makers: ["Na'aman Hirschfeld"]
---

# Rules, Diagnostics and Fixes

## Context and Problem Statement

Rule ids are what users type: in `[rules]`, in `[[ignore]]`, in CI scripts that read JSON, in SARIF and in baselines.
Once published they cannot change without breaking all of those. skif's rules come from several crates, and the JS
rules live in a crate most hosts do not compile, so the registry cannot be a hard-coded list, and severity and
suppression must behave the same for every rule, a host's own included.

Many findings have a mechanical repair: a version to align, a map to sort, a stray lockfile to delete. Re-serializing a
manifest loses formatting, a round-trip of `pnpm-workspace.yaml` loses comments, some problems have several valid
repairs, and a prompt cannot run inside a library, a hook or an editor. How are rules identified, registered and run,
what does a finding look like, how severe is each by default, and how are repairs described, selected and applied?

## Decision Drivers

* Rule ids are public API, self-describing and permanent
* Severity and suppression are applied centrally, and output is deterministic across runs and thread counts
* A first run on a real repository fails on dependency problems, not on dead-code heuristics
* A fix never changes a byte it does not have to, and a user or host sees the change before it happens
* `--fix` applies only what cannot break a build; riskier moves and file deletion need explicit opt-ins

## Considered Options

* Frozen kebab-case ids, a `RuleProvider` registry and a central emitter with tiered default severities, and structural
  fix operations returned as a format-preserving plan with safety judged by the move
* Keep or alias sherif's ids, rejected because several are misspelled or unclear and aliases keep them alive
* Rules emit text edits directly, rejected because a rule would have to know byte layout and two fixes in one file could
  not be planned together
* Write files from the library, rejected because a host could neither preview nor compose fixes

## Decision Outcome

Chosen option: "Frozen kebab-case ids, a `RuleProvider` registry and a central emitter with tiered default severities,
and structural fix operations returned as a format-preserving plan with safety judged by the move", because ids are
readable and permanent, one emitter makes severity and suppression uniform, editing through a CST keeps every untouched
byte, and judging each move lets `--fix` do the low-risk case while every risky move still needs `--fix-unsafe`.

### Rule ids

* Every rule has a kebab-case id that never changes once released; a change of meaning gets a new id. An id names the
  problem in the singular when each finding is one instance (`local-path-dependency`) and in the plural when it mirrors
  an upstream issue type users know (`unused-dependencies`).
* `rule-ids.txt` lists every released id and is never shortened. sherif ids that skif renamed are retired and not
  accepted; `crates/skif-rules/tests/retired-rule-ids.txt` lists each with its replacement, and the rule documentation
  names the sherif id of each renamed rule.

### Rules, providers and the emitter

* A `Rule` reports its `RuleMeta`, parses its options and checks a `LintContext`. A `RuleProvider` reports the metadata
  of its rules, validates their options and runs the enabled ones; a host with rules of its own implements it.
  `skif-rules` and `skif-js` are providers, and the facade assembles the registry from those its features enable.
* The registry rejects a duplicate id and is the one source for `skif rules`, the schema's rule list and the generated
  `docs/rules.md`. It validates configured ids and options when `Linter::new` runs.
* `RuleMeta` holds the id, the category, the default severity (none means off by default), the fixability, a one-line
  summary, and whether a finding is about the specifier its dependency is written with, which tells baselines how to key
  it ([ADR 0003](0003-configuration-suppressions-baselines-and-migration.md)).
* A rule emits every problem it finds and nothing else. The `Emitter`, forked per rule and merged afterwards, applies
  the effective severity (the configured level, else a severity the rule proposed for the finding, else the rule's
  default, then override levels and caps and `[lint] max-level`), the `[[ignore]]` entries and
  `[workspace] ignore-packages`. A rule whose level is `off` does not run. The toolchain rules also honour comment
  suppressions in the files they read ([ADR 0006](0006-version-policies-lockfiles-toolchains-and-updates.md)).

### The diagnostic

* A `Diagnostic` holds the rule, a severity (`Error`, `Warning`, `Info`), a one-line message, help, a location with an
  optional span, related locations, the package, the dependency and its specifier, a `detail` discriminator, and its
  fixes, the first preferred. `detail` carries machine-readable state, such as the syncpack state behind a policy
  finding, so the id set stays small.
* `Info` never fails a run.
* Findings are sorted by path, span start, rule id and message, so output is identical across runs and thread counts.
* A span holds half-open UTF-8 byte offsets and 1-based lines and columns counting Unicode scalar values; a line index
  built once per file also gives UTF-16 columns for the formats that count that way. Paths are root-relative with `/`.

### Default severities

* Dependency correctness is an error; heuristics whose precision depends on entry-point and binary detection, which
  varies most between projects, are warnings; a few analyses are opt-in. Among the JS
  rules, `unused-dependencies`, `unlisted-dependencies`, `unresolved-imports` and `stale-mocks` are errors, the unused
  file, export, member and dev-dependency rules are warnings, and `circular-imports` and `unused-subpath-exports` are
  off.
* The generated `docs/rules.md` states the default of every rule.
* `invalid-workspace-file` reports a broken member manifest as an error finding, so it exits 1 like any error finding,
  not 2 like a failed run.

### The fix model

* A fix is `Fix { title, safety, ops }`. `FixOp` is structural and format-agnostic: JSON and YAML operations on paths,
  a `Text` replacement for the rare change that is not structural, and file creation and deletion. Its values are
  skif-owned, and rules say what should change, never at which byte offset.
* `skif-edit` turns operations into text edits: JSON through a CST that keeps whitespace, key order, line endings and
  comments, with an inserted entry copying its siblings' layout; YAML through `yamlpatch` behind the `yaml-fix` feature,
  which keeps comments.
* The operations on one file are planned together. A value fix inside an object that a sort of the same object moves is
  rebased onto the moved text, so both apply in one run. An overlap that remains is a conflict, and the later fix is
  skipped with a reason, never merged.
* `apply_fixes` returns a `FixPlan` of one change per file (modify, create or delete), sorted by path, and touches no
  file system. The CLI writes it atomically per file, refuses a file that changed after planning, and `--dry-run` prints
  it as a diff instead ([ADR 0012](0012-cli-contract-and-reporting.md)). skif runs no formatter after fixing.

### Selection

* A rule returns every alternative fix in order, labelled by what it does. When configuration decides the answer
  (`[policy] prefer`, `--select highest|lowest`), the decided alternative is marked default. Without a pick or a default
  the finding is skipped as `NeedsChoice`; no library crate prompts, and the CLI turns `NeedsChoice` into a prompt when
  stdin is a terminal.
* `--fix-type` limits a run to the fixes of some rules or knip issue types.
* A fix writes the specifier its rule chose verbatim and never rewrites a `catalog:` reference to a version.

### Safety

* `Safe` means a fix stays within what the current declaration already promises: sorting, removing an empty map,
  setting `private: true` on the root, and an alignment that is a compatible upgrade to an explicit target, keeping the
  specifier's shape, never going lower and staying inside the window `^` itself allows.
* Safety is judged per move, not per rule: the same rule can produce `Safe` and `Unsafe` fixes. Every downgrade, every
  removal of code or configuration, and every move whose outcome skif chooses is `Unsafe`. `--select` decides which
  alternative is taken, never its safety. There are two safety levels.
* `--fix` applies `Safe` fixes; `--fix-unsafe` applies `Safe` and `Unsafe` fixes. A fix that deletes a file is skipped
  unless `--allow-remove-files` is given, whatever its safety. A fix run is refused in CI
  ([ADR 0012](0012-cli-contract-and-reporting.md)).
* Removal fixes keep the layout: a member alone on its line goes with its line and its comments, and there is no fix
  where removal could change behaviour, such as renumbering a later enum member or dropping an expression with effects.

### `package-json-format`

* Formatting `package.json` is a `manifest` rule, off by default, configured by a root-only `[format]` table, so it has
  the same suppression, output and exit codes as every rule. `skif format` runs that rule alone and applies its fixes;
  `--check` reports without writing.
* The defaults are syncpack's, except where syncpack would lose or change data: keys sort in byte order, the order
  `unsorted-dependencies` uses; a shorthand collapse happens only when nothing is lost; nothing is reordered across a
  comment; and only moving `types` first and `default` last among `exports` conditions is safe, since condition order
  is resolution behaviour.
* Without `indent` the layout is kept; with it, the file must be laid out as `JSON.stringify` would. The fix is one
  minimal text replacement, and formatting twice gives the bytes of formatting once.

### Consequences

* Good, because severity and suppression behave identically for every rule, and the JS rules plug in without the core
  knowing them.
* Good, because one diagnostic type feeds every renderer and host, and a first run fails on dependency problems.
* Good, because bytes outside an edit are identical after a fix, and comments in YAML files survive.
* Good, because a host previews, reviews or composes fixes, and `--fix` aligns versions within each major while a new
  major, a downgrade or a deletion needs an explicit opt-in.
* Bad, because freezing ids makes naming mistakes permanent, and renaming broke sherif configurations outright.
* Bad, because sorting after collection shows no finding until every rule has finished.
* Bad, because a skipped conflicting fix needs a second `--fix` run, and a removal can leave a blank line or an unused
  import for the formatter or the next run.

### Confirmation

* `crates/skif-rules/tests/rule_ids.rs` requires every released id to stay registered and every retired id to point at
  a registered replacement; `scripts/check-rule-ids.sh` compares `rule-ids.txt`, `docs/rules.md` and the registered
  metadata.
* `crates/skif/tests/docs.rs` regenerates `docs/rules.md` from the registry and fails on any difference.
* `crates/skif-lint/tests/registry.rs`, `crates/skif-lint/tests/severity.rs`, `crates/skif-lint/tests/ignore.rs` and
  `crates/skif-lint/tests/run.rs` cover registration, severity, suppression and deterministic order;
  `crates/skif-core/src/span.rs` covers line and column counting.
* `crates/skif/tests/fix.rs`, `crates/skif-cli/tests/cli.rs` and `crates/skif-cli/src/write.rs` cover planning,
  conflicts, safety selection, file removal, the CI refusal and atomic writes.
* `crates/skif-edit/tests/package_json_format.rs` snapshots the formatting fixtures, and the conformance fixtures under
  `fixtures/policy`, run by `crates/skif/tests/policy_conformance.rs`, mark which policy fixes are `Safe`.
