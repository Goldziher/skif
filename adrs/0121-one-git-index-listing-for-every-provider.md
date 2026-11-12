---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0038", "0095"]
---

# One Git Index Listing for Every Provider

## Context and Problem Statement

[ADR 0038](0038-git-index-file-discovery-and-resolver-probe-avoidance.md) put the reader of `.git/index` in `skif-js`,
and [ADR 0095](0095-untracked-files-member-manifest-errors-and-converging-fixes.md) keeps it for
`[js] discovery = "git"`. Since then other rules list directories for files that workspace discovery does not read:
the toolchain scan of [ADR 0087](0087-toolchain-pins-outside-package-json-and-the-toolchain-pin-drift-rule.md) lists
every package directory, its `docker` and `scripts` subdirectories, `.github/workflows` and the directories its `files`
globs start in, and the lockfile consumers of
[ADR 0116](0116-bazel-convenience-links-module-pins-and-more-lockfile-consumers.md) list the whole Bazel repository
for its `BUILD` files when the files Bazel reads first leave a lockfile unnamed. `skif-rules` does not depend on
`skif-js`, so these listings walk with one `opendir` per directory even when the run was told to trust the index, and
on a large repository the walk for `BUILD` files is the cost the index was introduced to avoid (GitHub issue #200).

Where does the index reader live, and how do the rules reach the listing it builds?

## Decision Drivers

* One reader and one set of rules for when the index may be trusted, whatever lists the directories
* `[js] discovery = "auto" | "git" | "walk"` keeps its meaning; nothing new to configure
* Library crates read only through the `FileSystem` trait
* A run reads and parses an index once, whichever provider asks first

## Considered Options

* Move the reader to `skif-workspace` and give the rules the listing through the lint context
* Move the reader to a new crate
* Keep the reader in `skif-js` and leave the rules to walk

## Decision Outcome

Chosen option: "Move the reader to `skif-workspace` and give the rules the listing through the lint context", because
`skif-workspace` is the lowest crate that both `skif-js` and `skif-lint` depend on and that knows the configuration
the choice depends on, and the lint context already carries what every rule of a run shares.

**Reader.** The index reader, the overlay and their tests move unchanged from `skif-js` to `skif-workspace`, which
exports `TrackedFiles` (the files tracked below one root by directory, with the index time a directory is compared
with), `IndexedFs<F>` (any file system `F`, owned or borrowed, with the directories of `TrackedFiles` listed from the
index), `IndexCache` (the parsed indexes, reused while the index keeps its modification time), and `git_dirs` and
`git_common_dir` (the work tree and git directory of a path). `TrackedFiles::for_discovery` holds the one gate: the
index is used under `[js] discovery = "git"` without `--no-gitignore`, and the walk otherwise, as ADR 0095 decided.
What is read, verified, refused and trusted is ADR 0042's, unchanged.

**Rules.** `LintContext::tracked_files` builds `TrackedFiles` for the workspace root once per run through
`LintContext::fs`, shared like any value of `LintContext::shared`; `LintContext::listing_fs` is `fs` with those
directories listed from the index, or `fs` itself when the gate says walk. The toolchain scan, the Dockerfiles it reads
beside a lockfile and the search for `BUILD` files list through `listing_fs`; every read and existence check still
goes to `fs`. Under `git` these listings therefore have the speed and the blind spot of the JS discovery: an untracked
Dockerfile or `BUILD` file created before the last index write, in a directory nothing has touched since, is not read.
Under `auto` and `walk`, and without a usable index, they walk as before.

**One parse per run.** `RunOptions::with_index_cache` passes a host's `IndexCache` to the run, and
`LintContext::index_cache` gives it to a provider; the JS provider uses it in place of its own. The `skif` facade's
`Linter` keeps one for its runs, so the rules and the JS analysis of a run read and parse the index once.

### Consequences

* Good, because `git` spares the walk of the toolchain scan and of the Bazel `BUILD` search too, and a new rule that
  lists directories gets the listing from the context instead of walking.
* Good, because the JS analysis and the rules decide from one gate, so they cannot disagree about whether the index
  is used.
* Bad, because under `git` a lockfile that only an untracked `BUILD` file or Dockerfile names, created before the
  last index write in an unchanged directory, is reported by `stray-lockfile`; the walk of `auto` finds it.
* Bad, because `skif-workspace` grows a SHA-1 implementation and the overlay, which only the listing uses.
* Neutral, because `skif_js` no longer has the reader; `skif_js::discovery_file_system` is unchanged.

### Confirmation

* `crates/skif-workspace/src/git_index.rs`: the tests of ADRs 0038, 0042 and 0095, moved with the reader.
* `crates/skif-rules/src/manifest/stray_lockfile.rs`:
  `under_git_discovery_the_bazel_listing_comes_from_the_git_index`,
  `under_git_discovery_the_toolchain_scan_lists_from_the_git_index` and
  `under_git_discovery_a_dockerfile_beside_a_lockfile_is_listed_from_the_git_index` lint a work tree whose index
  leaves out the only `BUILD` file or Dockerfile that names a lockfile, and compare `git` with `auto` and `walk`.
* `crates/skif-js/tests/git_discovery.rs` passes unchanged.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Move the reader to `skif-workspace` and give the rules the listing through the lint context

* Good, because no crate gains a dependency and the gate reads the configuration where it is already known.
* Bad, because the crate that discovers workspaces also reads git's index, which is a second concern.

### Move the reader to a new crate

* Good, because the reader stays a crate of its own with one concern.
* Bad, because one more published crate needs the configuration for the gate, or leaves the gate to each caller.

### Keep the reader in `skif-js` and leave the rules to walk

* Good, because nothing changes.
* Bad, because the slowest listings of a run on a large repository ignore the discovery the run was given.

## More Information

* Amends: [ADR 0038](0038-git-index-file-discovery-and-resolver-probe-avoidance.md) (where the reader lives) and
  [ADR 0095](0095-untracked-files-member-manifest-errors-and-converging-fixes.md) (what `discovery = "git"` governs)
* Issue: [#200](https://github.com/Goldziher/skif/issues/200)
* See the [ADR index](index.md).
