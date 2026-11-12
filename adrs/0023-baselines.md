---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Baselines

## Context and Problem Statement

A team that adopts skif on an existing repository meets findings it cannot fix at once. Turning rules off hides later
findings too; a baseline records the findings that exist when it is written so a run fails only on later ones, and
shrinks as they are fixed. A baseline must silence exactly what it records: a version finding by its specifier, a file
finding by its file, and nothing in packages added later. It must also be reviewable, so that a pull request cannot
grow it unseen.

How is a baseline stored, written, pruned and checked?

## Decision Drivers

* A baseline uses the suppression skif already has, with reasons and `--require-ignore-reasons`
* An entry silences what the finding is about and no more
* Matching cannot drift between a lint run and the baseline commands
* A baseline may shrink freely; growth needs a reviewed change
* A baseline the configuration does not apply is an error, not a silent no-op

## Considered Options

* `[[ignore]]` entries in a separate file that the root configuration extends, maintained by `skif baseline
  write|prune|check`
* A fingerprint file of findings that the CLI subtracts from the report
* Inline suppression comments at each finding

## Decision Outcome

Chosen option: "`[[ignore]]` entries in a separate file that the root configuration extends, maintained by `skif
baseline write|prune|check`", because the entries are ordinary configuration with reasons, reached through `extends`
with no key or lint flag of its own, and the lint run applies them with the code it already has.

### The baseline file

* `skif.baseline.toml` in the workspace root, or the file `--file` names, holds only `[[ignore]]` entries, each with
  `rules`, `packages`, `dependencies`, `files` and `reason` (`baseline`, or `--reason`; a reason edited by hand is
  kept). Any other key is a configuration error, so the commands can rewrite the file whole.
* Glob syntax in a recorded value is escaped, so an entry matches the value literally. Entries are sorted and
  deduplicated by rules, packages, dependencies and files.
* A package is recorded by name, or by directory when another package has the same name.

### What an entry records

* A finding of a `version_scoped` rule (`RuleMeta::version_scoped`: `version-mismatch`, `semver-range-mismatch`,
  `policy-conflict`, `unsynced-dependency-family`, `incompatible-peer-dependencies`, `catalog-mismatch`,
  `catalog-version-divergence`) records `name@specifier`, so a third version fails the run again. Any other finding
  records the dependency name.
* A finding about no dependency, located in a file other than a `package.json`, records its package and that file.
  `[[ignore]] files` matches the finding's root-relative location.
* Errors and warnings are recorded; `info` never fails a run. A finding that would need a workspace-wide entry (no
  package, dependency or file) or that lies in an unnamed root without a file is not recorded; `write` lists it as
  `unrecorded`, and the lint run still reports it.

### The commands

* `write` lints, adds an entry for every recorded finding no existing entry matches, keeps stale entries, and adds the
  baseline's path to the `extends` of the root configuration with `toml_edit` (creating a `skif.toml` when there is
  none). Running it again with the same findings changes nothing.
* `prune` removes the entries that match no finding of the run, and keeps an emptied file so `extends` still
  resolves.
* `write` and `prune` take the lint flags that change findings (configuration, rules, ignores, JS modes, filters),
  applied as a lint run applies them. They exit `2` when no configuration of the run extends the baseline or its
  entries do not all reach the merged `[[ignore]]` list.
* `check` lints nothing. It exits `1` when the baseline has an entry that the earlier version lacks, compared by rules,
  packages, dependencies and files: `--base <REF>` reads the file at a git revision with `git show`, `--base-file`
  reads a file. A bad revision is exit `2`. `check --base` is the one place besides the post-fix install where the CLI
  spawns a process ([ADR 0024](0024-cli-contract.md)).
* Matching goes through `skif_lint::ignore_matches`, which the lint run uses too and the facade re-exports.

### Several roots

* All commands resolve roots as a lint run does: `-r`, or a `PATH` without a `package.json` at or above it, covers
  every root, with one baseline file per root. `--file` and `--base-file` are usage errors with several roots.
* Roots are linted together, so they credit each other's uses as in `skif -r`; findings are recorded per root before
  the cross-root settling.

### Output

* `--format human|json`. The JSON document has `version`, `skif`, `command`, `file`, `entries`, `added` and
  `removed`; `write` adds `extended` and `unrecorded`, `write` and `prune` add `written`, `check` adds `base`, and a
  multi-root run lists one member per root under `roots`.

### Consequences

* Good, because the baseline is plain configuration: `skif config --show` lists it, and its entries have reasons.
* Good, because a version entry is exact, a file entry covers its file only, and a dependency bump leaves a
  non-version finding baselined.
* Good, because `check --base origin/main` in CI lets the baseline shrink and never grow without review.
* Good, because the baseline of a CI job that lints with flags matches what the job reports.
* Bad, because a file entry covers every finding of its rule in that file, so a later unused export in a file that
  already has one passes.
* Bad, because comments inside the baseline file are lost on the next `write` or `prune`.
* Bad, because `check --base` needs `git` on `PATH` and the revision fetched.

### Confirmation

* `crates/skif-cli/tests/baseline.rs`: `write_records_the_findings_and_the_lint_run_is_clean_after_it`,
  `writing_again_changes_nothing`, `a_baselined_version_still_fails_when_a_third_version_appears`,
  `prune_removes_the_entries_a_fix_left_unmatched`, `check_fails_when_the_baseline_grew_and_passes_when_it_shrank`,
  `check_against_a_git_revision`, `a_finding_in_an_unnamed_root_is_not_recorded`,
  `a_finding_that_is_not_about_the_version_survives_a_bump`, `a_finding_about_a_file_is_recorded_for_that_file_only`,
  `a_reset_of_ignore_that_drops_the_baseline_is_an_error`, `the_lint_flags_decide_what_is_recorded`,
  `every_root_below_a_directory_without_a_manifest_gets_its_own_baseline` and
  `a_file_argument_needs_a_single_root`.
* `crates/skif-lint/src/ignore.rs`: `ignore_matches_lists_the_findings_of_each_entry`,
  `name_at_specifier_matches_the_recorded_specifier` and `a_files_selector_matches_the_location_relative_to_the_root`.
* `crates/skif-rules/tests/rules.rs`: `only_the_rules_about_a_version_are_version_scoped`.

## Pros and Cons of the Options

### `[[ignore]]` entries in a separate file that the root configuration extends, maintained by `skif baseline write|prune|check`

* Good, because no configuration key, matching code or lint flag of its own is needed.
* Bad, because `RuleMeta` carries a flag that every rule reporting specifier findings must set.

### A fingerprint file of findings that the CLI subtracts from the report

* Good, because each finding is matched exactly.
* Bad, because fingerprints break on unrelated edits, carry no reasons, and the library would not see them.

### Inline suppression comments at each finding

* Good, because the suppression sits next to the code.
* Bad, because `package.json` has no comments, and a baseline spread over many files cannot be checked for growth.
