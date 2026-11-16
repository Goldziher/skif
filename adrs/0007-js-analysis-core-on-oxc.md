---
status: accepted
date: 2026-11-16
decision-makers: ["Na'aman Hirschfeld"]
---

# JS Analysis Core on oxc

## Context and Problem Statement

knip finds unused files, exports and dependencies and unlisted or unresolved imports in JavaScript and TypeScript
projects. It runs on Node, executes every tool's configuration, and is slow on large monorepos. skif reports the same
issue types inside one Rust process, next to its manifest rules. That needs a parser, a resolver that behaves as Node,
TypeScript and the bundlers do, a module graph, a listing of the files to read, and a way to keep the fast-moving oxc
crates from spreading through the code base. poly, a host that embeds skif, patches the oxc crates to a git copy, so
both must resolve to the same oxc release. Most of an untyped run is oxc's parse of every source file, and long-lived
hosts and multi-root runs parse the same unchanged files again and again.

Which semantics does the JS analysis follow, on which crates, how does it resolve and own files, which files does it
list, and what does it cache?

## Decision Drivers

* knip's results are the reference users compare against; agreement is measured, not assumed
* No Node and no executed configuration, so the analysis is safe on untrusted pull requests and runs in parallel
* oxc upgrades touch as few crates as possible and never the public API, and skif and poly share one oxc release
* Resolution follows the compiler where the compiler and knip disagree
* A cache never changes output, and its invalidation is correct by construction

## Considered Options

* knip semantics implemented natively on oxc, confined to the JS crates in lockstep with poly, with a content-keyed
  parse cache
* Run knip and translate its output, which keeps Node and executes configuration
* Port knip's code, which inherits its structure and licence obligations without its runtime
* Another parser stack (swc, tree-sitter), under which poly's oxc patch does not resolve

## Decision Outcome

Chosen option: "knip semantics implemented natively on oxc, confined to the JS crates in lockstep with poly, with a
content-keyed parse cache", because it removes Node, the binding boundary and config execution, keeps oxc upgrades
inside the JS crates, and is the only option under which poly's patch of the oxc crates resolves.

### knip as the specification

* knip, at the release recorded in `ATTRIBUTIONS.md`, is the specification: its documentation, issue types and
  observed output define correct behaviour. No knip code is copied
  ([ADR 0002](0002-crates-library-api-and-upstream-code.md)).
* Agreement with knip is a measure, not the goal. Where both tools report a file in use, skif fixes its own finding,
  and the comparison harness records the deliberate differences
  ([ADR 0014](0014-benchmarks-and-the-comparison-harness.md)).
* Each knip issue type maps to the skif rules that report the same findings, in the data file
  `crates/skif-migrate/src/knip-types.txt`, which migration and `--fix-type` share. `[js] production` and
  `--production` are knip's production mode.

### The JS crates and oxc

* `skif-js-syntax` is the only crate that names oxc parser types. Per file it produces a skif-owned `ModuleInfo`:
  imports and exports, `require()` and static `import()`, `import.meta.glob`, JSDoc tags, members and removal ranges,
  with in-file references resolved by `oxc_semantic`. It also holds the static configuration evaluator
  ([ADR 0008](0008-plugins-config-evaluation-and-commands.md)).
* `skif-js-plugins` holds the plugin table and the script parser; `skif-js` holds the module graph, resolution on
  `oxc_resolver`, the JS rules as a `RuleProvider` and the parallel worklist that builds the graph.
* No other published crate, `skif-js-plugins` included, depends on oxc directly, and no oxc type appears in a public
  item. The unpublished `skif-bench`
  parses with oxc to mirror knip's reading of a module.
* The oxc parser crates are caret requirements on the release poly pins by revision; `oxc_resolver` is a separate
  project outside the lockstep. An oxc bump is two coordinated pull requests, skif first.

### Resolution

* Resolution follows the compiler's order. The `paths` of `[js.workspaces.<dir>.paths]`, then those of the importer's
  `tsconfig.json` or `jsconfig.json`, are tried before lookup directories, installed packages and workspace members. A
  pattern whose targets all miss falls through to the package lookup, as TypeScript does. A `tsconfig` reached through
  `references` in another package lends its `paths`, output directories and conditions to no package that lists that
  package as a dependency.
* A mapping into a member's directory or into `node_modules` is an import of that package; a mapping into the
  importer's own sources is a local import that uses no dependency; a mapping to a declaration file only types the
  import, which still uses the listed package. A symbolic link to a file the analysis also finds is that file.
* Computed specifiers resolve only as far as they are static: a template literal that starts with a relative path is a
  glob of the files it can name, and any other computed specifier is an unknown target. Globs in configurations are
  approximated towards more entries, never fewer.
* A bundler's `resolve.extensions` apply to the innermost package that declares them, and stylesheet specifiers resolve
  with a stylesheet resolver. Every resolver set of a run shares one `oxc_resolver` cache.

### Declaration files

* Declaration files follow TypeScript's split between scripts and modules. A declaration file is an entry when it
  applies without an import: a script, a module that augments the global scope or declares a module, or an exportless
  module with a side-effect import. The parser records the fact, and the graph parses declaration files first.
* Any other declaration file is a module, an entry only when the manifest, an `entry` glob, a plugin or a
  `tsconfig.json` `files` list makes it one, and its unused exports and types are judged.
* `unused-files` never reports a declaration file, since one beside its runtime file types it without an import.

### Ownership

* A file belongs to the workspace package whose directory holds it. A root file that a package reaches first through
  `rootDirs` is adopted by that package and passes the owner on to the files it reaches below the same root directory.
* A module keeps the owner it got when first reached. The graph is built in rounds from entries in path order and
  merged in a fixed order, so the owner is the same in every run.

### Files the analysis reads

* `[js] discovery` is `auto` (the default) or `walk`, which walk the tree, or `git`, which lists tracked directories
  from the git index and lists from disk any directory modified at or after the index file. The index cannot know an
  untracked file in a directory untouched since its last write, so the default walks, and under `git` `unused-files`
  offers no deleting fix. `--no-gitignore` forces the walk.
* The index reader lives in `skif-workspace`, reads formats 2 to 4 with SHA-1 verified in the crate, starts no process
  and adds no dependency. An index it cannot read exactly (SHA-256, split or sparse, truncated) makes the run walk.
* One gate, `TrackedFiles::for_discovery`, decides for the whole run: `LintContext::listing_fs` lists with it for the
  rules, and the JS analysis lists with it too, from one `IndexCache` shared across the roots of a multi-root run.
* Fixture directories (`fixtures`, `__fixtures__`, `skif_config::FIXTURE_DIRECTORIES`) are read when an entry reaches
  them, but the reporting rules stay silent there, so deliberately broken test data does not bury real findings.
  `[js] include-fixtures` reports them as knip does.

### The parse cache

* The unit is the `ModuleInfo` parsing one file returns. Everything else (configuration, manifests, lockfiles, plugin
  configs, resolution, type facts, the graph and the rules) is computed on every run, so invalidation needs no
  dependency tracking.
* `ModuleKey` hashes `CACHE_SCHEMA`, the crate version, the parse options, the file name and the text. The directory is
  not part of the key, so identical files share an entry. `CACHE_SCHEMA` is raised whenever parse output changes, and
  a digest of a probe corpus beside it fails until it is. A failed lookup or an entry that does not decode is a miss.
* `skif-js-syntax` owns `ModuleCache` (`get` and `put`), an opaque `CachedModule`, `MemoryModuleCache` and
  `ParseCoalescer`. The cache is given to the linter, not to the run options. Beside it, the `CachedFs` overlay keeps
  a run's directory listings, path probes and small configuration files.
* `SharedModules::for_run` holds the sharing policy: a host cache serves every file; several roots without a host cache
  share an in-memory cache of the files that are not local to the root parsing them, so a run whose roots share no
  code, and every single-root run, pays nothing. `ParseCoalescer` sits beside the cache, so concurrent lookups of one
  key wait for one parse and a wrapping cache cannot drop the coordination.
* `--cache` is opt-in and has no configuration key, because a cache is not a setting of the project. The store is one
  deterministic file below `node_modules/.cache/skif/` by default, written atomically after the run only when its
  entries changed; a failed write is a warning.
* `--watch`, `skif lsp` and `skif mcp` keep in-memory caches across runs, pruned to what the last run used. The servers
  never read or write the store.

### Consequences

* Good, because parsing, resolution and analysis run in one process with the parser and resolver knip itself uses, and
  running skif on an untrusted pull request reads files and executes nothing.
* Good, because an oxc upgrade touches only the JS crates, and drift from poly fails skif's scheduled CI first.
* Good, because `paths` resolve in the compiler's order, and module declaration files have their unused types reported.
* Good, because no cache, a cold cache and a warm cache give the same report, and long-lived hosts and multi-root runs
  parse only what changed or what is shared once.
* Bad, because knip's edge cases are rediscovered through the comparison, not inherited.
* Bad, because skif's oxc upgrades follow poly's cadence, and the lockstep check needs the network.
* Bad, because the default walk pays one `opendir` per directory, and `git` has a blind spot by design.
* Bad, because a warm run still reads and hashes every file, and `CACHE_SCHEMA` relies on a test and review.

### Confirmation

* `scripts/check-oxc-git-patch.sh`, run by `.github/workflows/oxc-lockstep.yml` and `mise run oxc:lockstep`, and
  `scripts/check-pins.sh` hold the oxc lockstep; `scripts/check-dependency-tree.sh` holds which crates depend on oxc.
* `crates/skif-js/tests/paths_before_packages.rs`, `crates/skif-js/tests/fp_resolve.rs` and
  `crates/skif-js/tests/graph_fixes.rs` cover resolution order, ownership and declaration-file entries.
* `crates/skif-js/tests/git_discovery.rs`, `crates/skif-workspace/src/git_index.rs` and
  `crates/skif-js/tests/fixtures.rs` cover the listing modes, the index reader and fixture directories.
* `crates/skif-js-syntax/src/cache.rs` holds the schema digest and the coalescer; `crates/skif/tests/module_cache.rs`
  and `crates/skif-cli/tests/cache.rs` prove that no cache, a cold cache and a warm cache report alike.
