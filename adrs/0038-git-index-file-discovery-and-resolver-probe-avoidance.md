---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# Git Index File Discovery and Resolver Probe Avoidance: How the JS Analysis Stops Walking the Work Tree

## Context and Problem Statement

On a large pnpm monorepo (about 94 000 tracked files, 23 000 directories outside `node_modules`) the JS analysis is bound by file system calls. Two thirds of them come from the directory walk of file discovery (`opendir` and `getdents` per directory), a quarter from the resolver probing file names (`stat` per extension and index candidate) in directories nothing has listed, and the rest from reads and plugin config searches. `git ls-files` lists the same files in a fraction of the time because the index already holds them.

## Decision Drivers

* The JS analysis must be substantially faster than knip on large repositories
* Library crates must not spawn processes and should add no dependency
* The ignore rules of skif (`.gitignore`, `[js] ignore`, hidden and build directories, package-root-only directories, generated files) keep applying to what is listed
* A file that cannot be listed must still be reachable by path (`node_modules`, generated and ignored sources)

## Considered Options

* Read `.git/index` (formats 2 to 4) with a small reader in `skif-js` and serve tracked directories from it
* Depend on `gix-index`
* Spawn `git ls-files`
* Keep walking and only cache more

## Decision Outcome

Chosen option: "Read `.git/index` with a small reader in `skif-js` and serve tracked directories from it", because it needs no dependency and no process, and the walk keeps its rules.

`TrackedTree::load` finds the closest `.git` (a directory, or a file with `gitdir:` for linked work trees and submodules) at or above the analysis root, reads `<git dir>/index` and builds, for every tracked directory below the root, a sorted listing. Regular files and symbolic links are listed; submodule entries, conflicted duplicates and `skip-worktree` entries are not. Split (`link`) and sparse (`sdir`) indexes, other hash widths and truncated files make the reader refuse the index, and the analysis walks. The tree is also refused when nothing below the root is tracked.

`IndexedFs` wraps the run's file system. A listing of a tracked directory comes from memory; `is_file`, `is_dir` and `kind` of a tracked path are answered from the tree; everything else, including every negative answer, goes to the wrapped file system. The directory walk of discovery and the config searches of the plugins (`find_files`) run unchanged against it, so they list tracked files only and make no system call for directories; ignore files and manifests are still read from disk.

`[js] discovery` is `auto` (the default), `git` or `walk`. `auto` and `git` use the index when there is a usable one and walk otherwise; `walk` always walks. The index lists tracked files only: a file that was never `git add`ed is not found by listings (it is still resolved and followed when a tracked file imports it, and entry globs, `unused-files` and plugin searches do not see it), and a tracked file deleted from disk but not staged is listed until the index is updated; reading it is treated as an empty module, not as a parse error. Untracked files are not taken from git's untracked cache, which is optional and needs a `stat` per directory to validate. Symbolic links in the index are not listed.

`FileSystem` gains `kind(path) -> PathKind`, one lookup that tells file, directory or neither; `OsFs` answers it with one `metadata` call. `CachedFs` keeps the answer of `kind` (negative ones included) instead of separate file and directory answers, so a directory probed as a file and then as a directory costs one `stat`, not two, and the resolver adapter asks `kind`. `CachedFs` also lists a directory once three of its children were probed without a cached listing, except `node_modules` and `@scope` directories whose listing costs a `stat` per symbolic link, and answers the remaining probes from the listing. On a case-insensitive disk a probe in another case then misses where a `stat` would have found the file.

The remaining changes are structural. The resolver finds the scopes of an importer by walking its ancestors in a map instead of testing every scope, skips the lookup-directory probe when no scope has lookup directories, and keys its memo without allocating on a hit. `Setup::new` and the installed-manifest reads of `DependencyUsage` run per package in parallel. `entry_files` indexes candidates by owner and finds the files below a package directory by binary search on the sorted paths. `tsconfig` files are parsed once per run in the dependency pass. Plugin glob patterns are compiled once per process. `declares_module` is a set lookup.

### Consequences

* Good, because discovery makes no system call for tracked directories and the resolver no longer stats names inside them.
* Good, because skif adds no dependency and starts no process.
* Bad, because the default run no longer lists untracked files; `[js] discovery = "walk"` restores that.
* Bad, because the index reader supports SHA-1 indexes without split or sparse extensions only.
* Bad, because `FileSystem` has a new method with a default, which implementations that answer `is_file` and `is_dir` through it must override together.

### Confirmation

* `git_index.rs` tests build indexes of versions 2, 3 and 4 and assert the listing, the refusals, the root below the work tree and the fall-through of the overlay.
* The `cached_fs.rs` tests count the lookups that reach the wrapped file system for `kind`, repeated probes and `node_modules`.
* The `disk_tree` bench compares the walk with the index on a generated tree of more than 20 000 directories.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Read `.git/index` (formats 2 to 4) with a small reader in `skif-js` and serve tracked directories from it

* Good, because there is no dependency and no process.
* Bad, because the reader must be kept in step with the index format.

### Depend on `gix-index`

* Good, because it handles every extension and hash width.
* Bad, because it pulls a large tree of crates into a linter that needs the path list only.

### Spawn `git ls-files`

* Good, because git does the parsing.
* Bad, because library crates must not spawn processes and `git` may be absent.

### Keep walking and only cache more

* Good, because it lists untracked files.
* Bad, because the cost is the `opendir` per directory, which no cache removes.

## More Information

* Amends: [ADR 0010](0010-js-analysis-on-oxc.md)
* See the [ADR index](index.md).
