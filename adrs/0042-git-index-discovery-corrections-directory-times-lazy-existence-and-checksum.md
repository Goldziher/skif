---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# Git Index Discovery Corrections: Directory Times, Lazy Existence Checks and a Verified SHA-1 Index

## Context and Problem Statement

[ADR 0038](0038-git-index-file-discovery-and-resolver-probe-avoidance.md) lists tracked directories from `.git/index`. An audit of the shipped behaviour found four statements of the record that the code contradicts or that do not hold:

1. The default run lists a tracked file that was deleted from disk but not staged, which is the normal working state after `rm`. The file is reported by `unused-files`, a fix of `--fix-unsafe` deletes a file and the re-lint reports it again, and an import of the deleted file is silently resolved. A file created and not yet added is invisible to listings, so entries and `unused-files` differ from `walk` and from knip without any signal.
2. "Symbolic links in the index are not listed" and "Regular files and symbolic links are listed" stand side by side. The code keeps them, but as neither a file nor a directory, while a directory listing of the real file system reports a link to a file as a file. A tracked link to a source file was therefore invisible to listings.
3. "`is_file`, `is_dir` and `kind` of a tracked path are answered from the tree" and "the resolver no longer stats names inside them" claim savings that exist only for positive answers, while the record also sends every negative answer to the wrapped file system. A positive answer for a deleted file is wrong, which is the first finding.
4. "Other hash widths make the reader refuse the index" is not implemented: the entry size and the trailer are hard coded, and a SHA-256 index is refused only because the misaligned parse happens to fail. `auto` and `git` also behave identically, although the record presents them as different. The index is read and parsed once per analysed root instead of once per run.

## Decision Drivers

* The default must give the result of the walk, not a faster approximation of it
* Reading the index must stay cheaper than walking: no `opendir` for a directory that cannot have changed
* A torn index, a rewritten index and a repository with another hash must never produce a wrong listing
* ADR 0038 is locked, so the corrections are recorded here and in the effective statements of the index

## Considered Options

* Compare the modification time of each tracked directory with the index file and list a changed directory from disk
* Read git's untracked cache (`UNTR` extension)
* Make `walk` the default and keep the index for `git` only
* Keep the index listing and print a notice

## Decision Outcome

Chosen option: "Compare the modification time of each tracked directory with the index file and list a changed directory from disk", because it needs one `stat` per directory instead of one `opendir`, finds untracked and deleted files, and keeps the walk's rules.

**Listing.** `IndexedFs` serves the listing of a tracked directory in one of two ways. In `auto` (the default) it takes the modification time of the directory and of `.git/index`: a directory modified at or after the index time is listed with `read_dir` of the wrapped file system, everything else from the index. Equal times count as modified (the racy-git rule), and a directory whose time cannot be read is listed from disk. A listing from disk contains untracked files and directories and omits deleted files; an untracked directory is not in the index, so it is walked normally. The ignore rules, hidden and build directories and the other rules of the walk apply to either listing unchanged, because they act on the listing. In `git` the index alone is used, without any time check; it is faster and blind to files created or deleted since the last `git add`. `walk` is unchanged. `auto` falls back to the walk when the file system reports no modification time for the index; both index modes fall back to the walk when there is no usable index. The decision per directory is made once per run.

**Limit.** The index records no time at which a directory was last listed completely, so a directory is judged unchanged when it is older than the index file. An untracked file created before the last rewrite of the index (`git add` of another file, a `git status` that refreshes stat data) in a directory not touched since is not seen by `auto`. `walk` is the exact mode, and `auto` is exact for the sequence edit, run, add, run that a working session produces. Git's untracked cache would remove the limit and is not read.

**Existence.** `IndexedFs` no longer answers `is_file`, `is_dir` and `kind` from the tree; every one is asked of the wrapped file system, which memoises it (`CachedFs`). A tracked path costs one `stat` when it is used, and a deleted file is absent. This replaces the claim that the resolver no longer stats names inside tracked directories: what the index saves is the `opendir` of discovery and of plugin config searches. After the graph is built, a project file that nothing reaches is confirmed with one `stat` before it is reported by `unused-files`, because a directory judged unchanged may still list a file deleted before the last index rewrite.

**Links and conflicts.** A tracked symbolic link is listed as the real file system lists it: a file or a directory according to its target, with the link flag set, and neither when the target is missing. A conflicted file (stages 1 to 3) is listed once, so a merge in progress does not remove it from the run. Submodules and `skip-worktree` entries stay out, as in ADR 0038.

**Index integrity.** The reader verifies the trailer of the index as the SHA-1 of the bytes before it and refuses the index otherwise, so a torn or concurrently rewritten file is never trusted. An all-zero trailer (`index.skipHash`, which `feature.manyFiles` sets) is accepted, because it says nothing. A repository whose `extensions.objectformat` is `sha256`, read from the `config` of the git directory or, for a linked work tree, of the common directory, is refused before the index is read; a SHA-256 index would also fail the checksum. The SHA-1 is computed in the crate, so `skif-js` still adds no dependency.

**One parse per run.** The parsed index is cached per index file by `JsProvider` and reused for the next root while the modification time of the file is unchanged, so the roots of a multi-root run read and parse it once. The paths are kept in one buffer, not one allocation per tracked path, and a root below the work tree builds only its own subtree from it. A file system without modification times is never cached.

**Directory-valued lists.** `ignore-workspaces` and the glob keys of `[js.workspaces]` match directories with `*` stopping at `/`, as every other path glob in skif does and as knip's matcher does. Name lists such as `ignore-dependencies` keep `*` crossing `/` so that `@types/*` matches `@types/node`. The workspace keys are compiled once per run, an exact key wins over a glob key, and among glob keys the first in file order wins.

### Consequences

* Good, because the default `auto` run gives the result of the walk after an `rm`, a new file or a `--fix` pass, with one `stat` per tracked directory instead of one `opendir`.
* Good, because a deleted file is no longer reported, fixed twice or resolved.
* Good, because a SHA-256 repository, a torn index and a link to a source file are handled by a rule instead of by accident.
* Bad, because `auto` is exact only up to the limit above and not equal to `walk` there.
* Bad, because each resolved import of a tracked file costs one `stat` that ADR 0038 claimed to save.
* Bad, because `git` is blind by definition and must be chosen knowingly.

### Confirmation

* `git_index.rs` tests cover a deleted unstaged file, a new untracked file, a new untracked directory, a directory modified at the index time, a directory without a time, `git` ignoring the times, a later pass after a fix, one index read per cache and per rewrite, the checksum (torn, all-zero, 32-byte trailer), `objectformat` in the repository and common directory configuration, SHA-1 vectors, conflicted stages and the listing of tracked links against the real file system.
* `tests/git_discovery.rs` runs the lint on a real repository with `walk`, `auto` and `git`, and compares the diagnostics of `auto` with those of the walk after each of the changes above.
* The `disk_tree` bench asserts that every mode reports the diagnostics of the walk before timing it and measures analysis, listing of every directory and name probing through the overlay.
* `cached_fs.rs` counts one lookup for `kind` on a file system that answers it in one call, and separately the two checks of a file system that does not.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Compare the modification time of each tracked directory with the index file and list a changed directory from disk

* Good, because it costs a `stat` per directory and finds what the index does not know.
* Bad, because it cannot see an untracked file older than the last index rewrite.

### Read git's untracked cache (`UNTR` extension)

* Good, because it records when each directory was last listed.
* Bad, because it exists only when `core.untrackedCache` is on and is a further extension to parse and keep in step.

### Make `walk` the default and keep the index for `git` only

* Good, because it is always exact.
* Bad, because it gives up the speed that is the point of the index.

### Keep the index listing and print a notice

* Good, because it changes nothing.
* Bad, because the result still differs from knip and from CI, and a notice is not a fix.

## More Information

* Amends: [ADR 0038](0038-git-index-file-discovery-and-resolver-probe-avoidance.md)
* See the [ADR index](index.md).
