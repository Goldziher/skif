---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0008", "0042", "0053", "0087"]
---

# Untracked Files Under the Default Discovery, Workspace Files Discovery Cannot Read, and Fixes That Converge in One Run

## Context and Problem Statement

An audit of the 0.3.0 core found four places where skif's default behaviour gives a wrong answer and says nothing.

1. `[js] discovery = "auto"`, the default, lists a tracked directory from the git index when the directory is older
   than `.git/index` ([ADR 0042](0042-git-index-discovery-corrections-directory-times-lazy-existence-and-checksum.md)).
   The index records tracked files only, so a directory that gained an untracked file before the index was last
   written (any `git add`, `git commit`, `git checkout` or refreshing `git status` writes it) is served without the
   file. ADR 0042 records this as a limit and calls `auto` exact for "edit, run, add, run"; the equally common
   "create a file, commit something else, run" is not. On a repository where `src/index.ts` uses `a` from
   `src/lib.ts` and an untracked `scripts/run.ts` (an `entry` glob) imports `helper` from `src/lib.ts` and `only` from
   `src/only.ts`, the run after committing another file reports `unused-exports helper` and `unused-files
   src/only.ts`, and `--fix --allow-remove-files` deletes `src/only.ts`, a tracked file the untracked one uses.
2. Discovery collects a member `package.json` that is not valid JSON or cannot be read, a workspace pattern that
   cannot be used and a `.yarnrc.yml` that is not valid YAML in `Workspace::manifest_errors` and goes on, so the rest
   of the workspace is linted. Nothing reads that list: a workspace whose member `packages/e/package.json` holds
   `{"name":"e", "version": "1.0.0",, }` is "No issues found", exit 0, where sherif fails with "Failed to collect
   package".
3. One `--fix` run skips every value fix (`version-mismatch`, `semver-range-mismatch`, `catalog-not-used`) in an
   object that `unsorted-dependencies` also sorts, as a conflict: the sort rewrites the members the value fix edits,
   so the facade drops the later fix ([ADR 0008](0008-fix-and-edit-model.md)) and only a second run applies it. Any
   hand-edited manifest is unsorted, so `--fix` exits 1 with fixable findings left in the common case.
4. `incomplete-packages` in the JSON report ([ADR 0053](0053-incomplete-packages-in-the-json-report.md)) does not
   say whether a package is unjudged as a whole or only within the scope a config narrows it to
   ([ADR 0065](0065-narrowed-scope-for-incomplete-entries.md)), so a consumer, the knip comparison among them, has to
   treat every listed package as unjudged.

A fifth point is structural: the toolchain rules carry their own `FileSystem`, passed to
`toolchain_provider(fs)` ([ADR 0087](0087-toolchain-pins-outside-package-json-and-the-toolchain-pin-drift-rule.md)),
while the rules over a lockfile and patch files read through `LintContext::fs`, the file system of the run. Two ways
to reach files mean two places a host has to wire.

How do the default discovery, a broken workspace file, a fix inside a sorted object and an incomplete package get a
correct, visible answer?

## Decision Drivers

* The default must give the result of the walk; speed that costs correctness is opt-in
* A file skif could not read must fail the run, as sherif does, without stopping the lint of the rest
* One `--fix` run should leave nothing a second run would fix
* A machine-readable report must say what was not judged
* Rules reach files one way

## Considered Options

* Walk by default and keep the index for `discovery = "git"`; report unreadable workspace files as a rule; plan a value fix and a sort together; add the scope to the JSON; read toolchain files through the run
* Read git's untracked cache (`UNTR`) and fall back to the walk without it
* Keep the index for `auto` and refuse destructive fixes on an index listing only
* Fail the run with exit 2 on an unreadable workspace file
* Repeat apply and re-lint in the CLI until no fix applies

## Decision Outcome

Chosen option: "Walk by default and keep the index for `discovery = "git"`; report unreadable workspace files as a rule; plan a value fix and a sort together; add the scope to the JSON; read toolchain files through the run", because each part makes the default answer correct with the smallest change, and none of them depends on state skif cannot observe.

**Discovery.** `auto` walks the directories. The index cannot tell whether a directory older than it holds an
untracked file: it records no time at which a directory was last listed in full, and without git's untracked cache
nothing else does, so there is no case in which `auto` could use the index and stay exact. `git` now does what `auto`
did under ADR 0042: tracked directories come from the index, and a directory modified at or after the index time is
listed from disk, so files created or deleted after the last index write are found; on a file system without
modification times every tracked directory is listed from the index. The former `git`, the index without any
directory check, is gone: it was blind to every new file for the saving of one `stat` per directory. `git` suits a
fresh checkout, as in CI, and remains blind to an untracked file older than the last index write, so its
`unused-files` findings offer no fix that deletes the file, and the help says why. `walk` is unchanged and equals
`auto`.

**Workspace files discovery cannot read.** `invalid-workspace-file` (category `workspace`, default `error`, no fix)
reports every entry of `Workspace::manifest_errors`: a member `package.json` that is not valid JSON, at the character
of the syntax error, or cannot be read; a workspace pattern that cannot be used (a `..` that leaves the root, too many
brace alternatives), at the pattern where it is written; and a `.yarnrc.yml` that is not valid YAML. Discovery is
unchanged: the package, the pattern's matches or the Yarn catalogs are left out, and the rest of the workspace is
linted. A finding is not a failure to run, so the exit code is `1`
([ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md)), not `2`; the JSON, SARIF and editor
output carry it like any finding, and a project that keeps a broken fixture member on purpose turns the rule off or
narrows the patterns. Migration from sherif leaves the rule on its default (sherif fails on such a file too); the
other migrators record it as a check they did not run.

**Fixes inside a sorted object.** When the edits of a fix overlap the accepted edits of a JSON or YAML file, the
facade plans the operations of the file together before it calls the fix a conflict. `skif-edit` already rebases a
change inside a member onto the text a `SortKeys` moves, so a value fix and the sort of its object form one
non-overlapping plan, and both are accepted, whichever comes first in the report. Two edits of the same node still
overlap in the joint plan and stay a conflict, as ADR 0008 says.

**Scope in the JSON report.** Each `incomplete-packages[]` entry gains `scope`: `"package"` when no file of the
package is judged unused (the scope is not narrowed, or a computed `import()` widened it), or `{"within": [globs]}`
with the globs, relative to the package `dir`, of the only files left unjudged. `IncompleteScope` moves from
`skif-js` to `skif-core` so that `IncompletePackage` can carry it; `skif_js::IncompleteScope` re-exports it. The
field is additive and `version` stays `1`. The human note names the globs after a narrowed package.

**One way to read files.** The toolchain rules read through `LintContext::fs`, like the lockfile rules.
`toolchain_provider()` takes no file system, and `skif_rules::providers()` includes it, so a host registers every
built-in provider in one call and passes its file system once, through `RunOptions::with_fs` (the `skif` facade does
so for `Linter::with_fs`).

### Consequences

* Good, because the default run finds every untracked file whatever git did since it was created, so no fix deletes
  a file such a file uses.
* Good, because a broken member manifest fails the run where it is broken, in every output format, and the rest of
  the workspace is still checked.
* Good, because a single `--fix` leaves no fixable finding in the common case of an unsorted manifest with a version
  to align.
* Good, because a consumer of the JSON report can tell an unjudged package from one judged outside a narrowed scope.
* Bad, because `auto` loses the index's speed: a run lists every directory with `opendir`, which ADR 0042 avoided for
  unchanged ones. A large repository that wants it back on a fresh checkout sets `discovery = "git"`.
* Bad, because `git` no longer has a mode that skips the directory checks, and it offers no file-deleting fix.
* Bad, because a workspace that lints today with a broken member now fails, which is the point but is a change of the
  exit code for such repositories.
* Bad, because the joint plan of a file is computed again for every later fix that overlaps it, which costs a parse
  of the file per such fix.

### Confirmation

* `crates/skif-js/tests/git_discovery.rs`: `an_untracked_file_older_than_the_last_index_write_is_found` commits
  another file after creating an untracked importer and compares `auto` with the walk;
  `the_index_listing_sees_files_created_or_deleted_since_the_index_was_written` and
  `unused_files_listed_from_the_index_offer_no_deletion` cover `git`.
* `crates/skif-js/src/git_index.rs`: `without_modification_times_the_index_alone_is_trusted` and the existing
  directory-time tests.
* `crates/skif-rules/src/manifest/invalid_workspace_file.rs` unit tests locate an invalid member manifest at its
  syntax error, an unusable pattern at the pattern and an invalid `.yarnrc.yml`;
  `an_invalid_member_manifest_fails_the_run` in `crates/skif-cli/tests/cli.rs` asserts exit `1` in human and JSON
  mode.
* `a_value_fix_and_a_sort_of_the_same_object_both_apply` in `crates/skif/tests/fix.rs` plans both orders;
  `overlapping_fix_is_skipped_and_the_earlier_one_wins` still holds for two values of one node;
  `one_fix_run_aligns_and_sorts_the_same_dependencies` in `crates/skif-cli/tests/cli.rs` runs one `--fix`.
* `json_incomplete_packages_say_whether_the_whole_package_is_unjudged` in `crates/skif-report/tests/render.rs`, the
  `matches_serde_json` test of the JSON writer and `incomplete_packages_carry_the_scope_left_unjudged` in
  `crates/skif-js/tests/explain_absence.rs`.
* `crates/skif-rules/tests/rules.rs` lists the toolchain provider among `providers()`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Walk by default and keep the index for `discovery = "git"`; report unreadable workspace files as a rule; plan a value fix and a sort together; add the scope to the JSON; read toolchain files through the run

* Good, because every default answer is exact, and each change is local to the crate that owns it.
* Bad, because the default run is slower on a large work tree than under ADR 0042.

### Read git's untracked cache (`UNTR`) and fall back to the walk without it

* Good, because the cache records when git last listed each directory and which untracked entries it held, so a
  directory it vouches for could be served without `opendir`.
* Bad, because git writes it only with `core.untrackedCache` (or `feature.manyFiles`), so most repositories would
  walk anyway, and trusting it means parsing EWAH bitmaps and checking the stat data of every directory and the hash of
  every `.gitignore` it depends on. It remains the way to give `auto` the index's speed back.

### Keep the index for `auto` and refuse destructive fixes on an index listing only

* Good, because the default keeps its speed.
* Bad, because the false `unused-exports` and `unused-files` findings stay; only the deletion is prevented.

### Fail the run with exit 2 on an unreadable workspace file

* Good, because nothing could turn it off.
* Bad, because exit `2` means skif could not run, while the rest of the workspace was linted; the location of the
  error would only be in an error message, not in SARIF or an editor; and the run would stop reporting the findings
  of every other package.

### Repeat apply and re-lint in the CLI until no fix applies

* Good, because it converges for any pair of conflicting fixes, not only sorts.
* Bad, because a fix run then writes the files several times, `--dry-run` and the library plan would still differ
  from what `--fix` does, and a prompt for alternatives could be asked again on every pass.

## More Information

* Amends: [ADR 0042](0042-git-index-discovery-corrections-directory-times-lazy-existence-and-checksum.md) (the
  "Listing" and "Limit" paragraphs: `auto` walks; `git` lists from the index with the directory check),
  [ADR 0008](0008-fix-and-edit-model.md) (an overlap is a conflict only when the joint plan of the file also
  overlaps), [ADR 0053](0053-incomplete-packages-in-the-json-report.md) (the `scope` of an incomplete package),
  [ADR 0087](0087-toolchain-pins-outside-package-json-and-the-toolchain-pin-drift-rule.md) (the toolchain rules read
  through the run and `providers()` includes them)
* Related decisions: [ADR 0016](0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md),
  [ADR 0038](0038-git-index-file-discovery-and-resolver-probe-avoidance.md),
  [ADR 0052](0052-file-deletion-opt-in-fix-types-and-member-removal-fixes.md),
  [ADR 0065](0065-narrowed-scope-for-incomplete-entries.md)
* See the [ADR index](index.md).
