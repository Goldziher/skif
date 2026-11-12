---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Files the JS Analysis Reads: The Walk, the Git Index and Fixture Directories

## Context and Problem Statement

The JS analysis needs the source files of every package to find entries, project files and unused files, and several
rules list directories too: the toolchain scan, the Dockerfiles beside a lockfile and the search for Bazel `BUILD`
files. Walking a large work tree costs one `opendir` per directory, while the git index already lists every tracked
file. The index cannot tell whether a directory holds an untracked file, and an untracked file that imports a tracked
one decides whether that file is used; a deleting fix based on an incomplete listing removes a file in use. Large
repositories also keep directories of test fixtures that are deliberately unused, broken or importing packages the
project never installs, and reporting them buries every real finding.

Which files does the JS analysis list, from where, and which directories does it read but not report?

## Decision Drivers

* The default run is exact: it finds every file the walk finds, whatever git did since
* A faster listing is available for checkouts where it is safe, such as CI, and is chosen knowingly
* Reading the index adds no dependency and starts no process
* The JS analysis and the rules agree on whether the index is used
* Fixture directories do not drown real findings, and knip's behaviour stays one setting away

## Considered Options

* Walk by default, list from the git index under `[js] discovery = "git"` with a directory-time check, and silence
  the reporting rules in fixture directories
* Use the index by default
* Run `git ls-files`
* Report fixture directories as knip does

## Decision Outcome

Chosen option: "Walk by default, list from the git index under `[js] discovery = "git"` with a directory-time check,
and silence the reporting rules in fixture directories", because the default stays exact, the index saves the walk
where a checkout is fresh, one gate decides for every listing, and the default is right for repositories with fixture
directories.

### Discovery modes

* `[js] discovery` is `auto` (the default), `walk` or `git`. `auto` and `walk` walk the tree: the index records no time
  at which a directory was last listed in full, so it cannot be exact for an untracked file.
* `git` lists tracked directories from the index, and lists from disk any directory whose modification time is at or
  after the index file's (equal times count as modified), so files created or deleted since the last index write are
  found. On a file system without modification times the index alone is used. It stays blind to an untracked file
  created before the last index write in a directory untouched since, so under `git` `unused-files` offers no fix
  that deletes a file.
* `--no-gitignore` forces the walk, because the index lists no ignored file.
* The walk's rules (ignore files, hidden and build directories) apply to either listing, since they act on the
  listing.

### The index reader

* The reader lives in `skif-workspace`: `TrackedFiles` (the tracked files below a root, by directory),
  `IndexedFs` (any file system with those directories listed from the index), `IndexCache` (parsed indexes, reused
  while the index file keeps its modification time) and `git_dirs` and `git_common_dir`. `TrackedFiles::for_discovery`
  holds the one gate.
* It finds `.git` as a directory, a `gitdir:` file or a linked work tree, reads index formats 2 to 4, and verifies the
  SHA-1 trailer; an all-zero trailer (`index.skipHash`) is accepted, leaving bounds checks as the only defence against a
  torn read. A `sha256` object format, split and sparse indexes and truncated files are refused, and the run walks.
* Submodules and `skip-worktree` entries are left out. A tracked symbolic link is listed as the disk lists it, by its
  target, and a conflicted file is listed once. The SHA-1 is computed in the crate, so no dependency is added.

### File system layers

* `IndexedFs` only lists. Every existence check and read goes to the wrapped file system, so a deleted file is absent
  and a resolved import of a tracked file costs one `stat`.
* `CachedFs` memoises `kind` (file, directory or neither, negative answers included) and listings. A directory whose
  children are probed repeatedly is listed after a few probes and answers the rest from the listing, except
  `node_modules` and scope directories, whose listing costs a `stat` per link; a name the listing lacks but a sibling
  spells in another case is asked of the disk.
* After the graph is built, a project file nothing reaches is confirmed with one `stat` before `unused-files` reports
  it.

### One listing for the run

* `LintContext::listing_fs` is the run's file system with the tracked directories listed from the index when the gate
  says so, built once per run; `LintContext::fs` serves every read. The toolchain scan, the Dockerfiles read beside a
  lockfile and the `BUILD` search list through `listing_fs` ([ADR 0011](0011-lockfile-and-toolchain-rules.md)).
* One `IndexCache` serves a run: `RunOptions::with_index_cache` passes a host's, the facade's `Linter` keeps one, and
  the JS provider uses it, so the rules and the JS analysis read and parse the index once, also across the roots of a
  multi-root run.
* Under `git` these listings share the blind spot: a lockfile whose only consumer is an untracked `BUILD` file or
  Dockerfile created before the last index write may be reported by `stray-lockfile`.

### Fixture directories

* A file is in a fixture directory when any directory of its path relative to the workspace root is named `fixtures`
  or `__fixtures__` (`skif_config::FIXTURE_DIRECTORIES`, also the first entries of the `discover-ignore` default).
  Other names of test data are not included; `[js] ignore` is the place for them.
* There, `unused-files` reports no file, and the export, type, namespace and member rules, `unresolved-imports` and
  `unlisted-dependencies` report nothing. `duplicate-exports`, `unparseable-files` and `circular-imports` still see a
  fixture that an entry reaches.
* A fixture file is neither an entry nor ignored: when an entry reaches it, its imports count; when nothing does, it is
  not read, so a dependency only an unreached fixture imports is still unused.
* `[js] include-fixtures` (default `false`, top level only) reports fixture directories like any other.

### Path-valued lists

`ignore-workspaces` and the glob keys of `[js.workspaces]` match directories with `*` stopping at `/`, and name lists
keep `*` crossing `/` ([ADR 0005](0005-the-js-table-ignore-lists-and-suppression-reasons.md)).

### Consequences

* Good, because the default run finds every untracked file and never offers to delete a file such a file uses.
* Good, because `git` spares the walk of the JS discovery, the toolchain scan and the `BUILD` search on a fresh
  checkout.
* Good, because the JS analysis and the rules decide from one gate and read the index once.
* Good, because repositories with fixture directories lose their false `unused-files` findings without configuration.
* Bad, because `auto` pays one `opendir` per directory.
* Bad, because `git` has a blind spot by design and offers no file-deleting fix.
* Bad, because the reader supports SHA-1 indexes without split or sparse extensions only.
* Bad, because a dead file in a fixture directory is not reported unless asked, and skif disagrees with knip there.

### Confirmation

* `crates/skif-js/tests/git_discovery.rs`: `an_untracked_file_older_than_the_last_index_write_is_found`,
  `the_index_listing_sees_files_created_or_deleted_since_the_index_was_written`,
  `unused_files_listed_from_the_index_offer_no_deletion`, `untracked_files_and_directories_are_found` and
  `a_file_deleted_but_not_staged_is_neither_unused_nor_resolved`.
* `crates/skif-workspace/src/git_index.rs`: `without_modification_times_the_index_alone_is_trusted`,
  `sha1_matches_the_published_vectors`, `a_sha256_repository_is_refused`,
  `a_conflicted_file_is_listed_once_and_sparse_entries_are_left_out`,
  `tracked_symbolic_links_are_listed_like_the_disk_lists_them` and
  `a_linked_work_tree_reads_the_configuration_of_the_common_directory`.
* `crates/skif-rules/src/manifest/stray_lockfile.rs`: `under_git_discovery_the_toolchain_scan_lists_from_the_git_index`,
  `under_git_discovery_a_dockerfile_beside_a_lockfile_is_listed_from_the_git_index` and
  `under_git_discovery_the_bazel_listing_comes_from_the_git_index`.
* `crates/skif-js/tests/fixtures.rs`: `files_in_fixture_directories_are_not_unused_by_default` and
  `include_fixtures_reports_fixture_directories_like_any_other`; `crates/skif-config/tests/config.rs`:
  `root_discovery_skips_the_fixture_directories_the_js_analysis_knows` and
  `root_only_js_keys_are_rejected_in_an_override`.

## Pros and Cons of the Options

### Walk by default, list from the git index under `[js] discovery = "git"` with a directory-time check, and silence the reporting rules in fixture directories

* Good, because the default is exact and the fast path is explicit.
* Bad, because the fast path has to be chosen and has a documented blind spot.

### Use the index by default

* Good, because every run would skip the walk.
* Bad, because no check makes the index exact for untracked files, so a default run could delete a file in use.

### Run `git ls-files`

* Good, because git knows its own index best.
* Bad, because it starts a process, needs git installed, and still cannot list untracked files cheaply.

### Report fixture directories as knip does

* Good, because skif would agree with knip on them.
* Bad, because the findings of large repositories would be dominated by deliberately unused files.
