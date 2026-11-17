---
status: accepted
date: 2026-11-17
decision-makers: ["Na'aman Hirschfeld"]
---

# JS Analysis Core on oxc

## Context and Problem Statement

knip finds unused files, exports and dependencies and unlisted or unresolved imports in JavaScript and TypeScript
projects. It runs on Node, executes every tool's configuration, and is slow on large monorepos. skif reports the same
issue types inside one Rust process, next to its manifest rules. That needs a parser, a resolver that behaves as Node,
TypeScript and the bundlers do, a module graph, a listing of the files to read, and a way to keep the fast-moving oxc
crates from spreading through the code base. poly, a host that embeds skif, patches the oxc crates to a git copy, so
both must resolve to the same oxc release. Most of a run is oxc's parse of every source file, and long-lived
hosts and multi-root runs parse the same unchanged files again and again. An opt-in cache helps only users who ask for
it, and poly, which runs skif in process over an in-memory file system and caches only the whole verdict, runs skif
cold again after any edit.

Which semantics does the JS analysis follow, on which crates, how does it resolve and own files, which files does it
list, and what does it cache, where, and who owns the store?

## Decision Drivers

* knip's results are the reference users compare against; agreement is measured, not assumed
* No Node and no executed configuration, so the analysis is safe on untrusted pull requests and runs in parallel
* oxc upgrades touch as few crates as possible and never the public API, and skif and poly share one oxc release
* Resolution follows the compiler where the compiler and knip disagree
* A cache never changes findings or fixes: a damaged, foreign or stale entry is a miss, and invalidation is correct by
  construction
* The host owns where a cache lives; the library caches only when given a store and does no I/O for it
  ([ADR 0002](0002-crates-library-api-and-upstream-code.md))
* Every host can keep warm results by default, and a cold run pays little for writing them

## Considered Options

* knip semantics implemented natively on oxc, confined to the JS crates in lockstep with poly, with content-keyed
  caching through a host-owned store
* Run knip and translate its output, which keeps Node and executes configuration
* Port knip's code, which inherits its structure and licence obligations without its runtime
* Another parser stack (swc, tree-sitter), under which poly's oxc patch does not resolve
* Keep the opt-in, CLI-only parse cache, because default runs, the npm addon and poly would parse every file on every
  run
* A host callback that skif calls to read and write entries, because poly's component interface has no host callback
* A store held in the run options, because a store outlives a run and a long-lived host would hand it over on every
  call
* A write-side `CacheDir` trait each host implements, because every host would re-implement skif's layout, eviction and
  integrity checks

## Decision Outcome

Chosen option: "knip semantics implemented natively on oxc, confined to the JS crates in lockstep with poly, with
content-keyed caching through a host-owned store", because it removes Node, the binding boundary and config execution,
keeps oxc upgrades inside the JS crates, is the only option under which poly's patch of the oxc crates resolves, and
lets every host, poly included, keep warm results in a store it owns while the library does no I/O.

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

### The cache store

* `skif-core` defines `CacheStore` beside `FileSystem`, with `get(namespace, key)` and `put(namespace, key, value)`.
  Any failure of `get` is a miss; a failed `put` is the host's to log. A `CacheNamespace` is a name and a schema number
  (`modules`) and stays a separate parameter, so a host lays out and reports entries per
  namespace.
* The linter holds the store, not the run options, because a store outlives a run: `Linter::with_cache_store` gives
  it and `Linter::with_cache_salt` adds the host's opaque salt. A linter without a store keeps nothing beyond the run.
  Caching by default is host policy, never the library's.
* Keys are 32-byte blake3 digests skif computes over the cache version, the namespace schema, a digest of the
  configuration the entry depends on, root-relative paths, content digests and the host salt. A key never holds a
  modification time or an absolute path, so hook snapshots and worktrees of one tree share entries. Hosts put a build
  salt (the executable's stat on non-release builds) into the salt.
* skif wraps every value in an envelope it writes and verifies: magic, namespace, schema, key echo, length and
  checksum. A truncated, flipped or foreign value is a miss, never a wrong result, whatever the host stores. The
  envelope is integrity, not authentication, so who may write a store is host policy
  ([ADR 0015](0015-distribution-release-and-supported-versions.md)).
* With a store supplied, skif writes no file of its own.
* The report carries per-namespace counters (`hits`, `misses`, `bytes_written`), so a host or a test can assert that a
  warm run hit every entry without reading the store. Apart from the counters, no store, a cold store, a warm store and
  a corrupt or adversarial store give the same report.

### Cache layers

* L1, per-file module facts: the `ModuleInfo` parsing one file returns, in the `modules` namespace with a binary
  payload. It saves oxc's parse and semantic pass, most of a run; reading and hashing every file remains.
  Everything else (configuration, manifests, lockfiles, plugin configs, resolution, the graph and the rules) is computed
  on every run, so L1 needs no dependency tracking. The `modules` schema, `CACHE_SCHEMA`, is raised whenever parse
  output changes, and a digest of a probe corpus beside it fails until it is.
* There is no typed layer: skif has no type checker ([ADR 0010](0010-type-aware-analysis-on-tsrs.md)).
* L2, lockfile and plugin-config caches, only after a measurement shows they pay.
* L3, a CLI-only stat fast path (path to stat and content key, with poly's racy-timestamp rule), comes last because it
  changes the `FileSystem` trait. Stat records are never trusted across checkouts.

### In-memory caches

* `skif-js-syntax` owns `ModuleCache` (`get` and `put`), an opaque `CachedModule`, `MemoryModuleCache` and
  `ParseCoalescer`; an adapter serves `ModuleCache` from the linter's store. Beside them, the `CachedFs` overlay keeps a
  run's directory listings, path probes and small configuration files.
* `SharedModules::for_run` holds the sharing policy: a host cache serves every file; several roots without a host cache
  share an in-memory cache of the files that are not local to the root parsing them, so a run whose roots share no
  code, and every single-root run, pays nothing. `ParseCoalescer` sits beside the cache, so concurrent lookups of one
  key wait for one parse and a wrapping cache cannot drop the coordination.

### Hosts: the CLI, the npm addon and poly

* The settings are `enabled`, `location` and `max-size`. Flags (`--no-cache`, `--cache`, `--cache-location`,
  `--cache-max-size`) beat the environment (`SKIF_CACHE=0|1`, `SKIF_CACHE_DIR`, `SKIF_CACHE_MAX_SIZE`), which beats
  `[cache]` in the root configuration ([ADR 0003](0003-configuration-suppressions-baselines-and-migration.md)), which
  beats the defaults: on, 512 MiB. No cache setting enters a key.
* The default location is `$TEST_TMPDIR/skif` under `bazel test` ([ADR 0011](0011-nx-lerna-and-bazel.md)); else
  `<base>/node_modules/.cache/skif` when `node_modules` exists, which skif never creates; else the per-user cache
  directory (`$XDG_CACHE_HOME/skif`, `~/Library/Caches/skif`, `%LOCALAPPDATA%\skif`); else memory for the run.
* One location serves many bases, each in `<location>/<base-key>/`, the key taken from the base path relative to the
  git top level, or absolute outside git. A directory skif creates gets a `CACHEDIR.TAG` and a `.gitignore` of `*`.
* Each store keeps, per namespace, what its base's last run used. After the write, a location over its cap drops
  other bases' stores, least recently modified first.
* An unreadable, corrupt or foreign store is empty and a bad entry is a miss. A failed write is silent when the cache
  came from the defaults (one line under `--debug`) and a warning when it was asked for. The cache never changes the
  exit code or the findings. A write renames atomically behind a try-lock and is skipped on contention.
* With `CI` set and no explicit location, the CLI reads the store and never writes it, since nothing would restore it.
* `skif cache stats` and `skif cache clean` report and remove the store; the MCP server mirrors them
  ([ADR 0013](0013-editors-agents-and-git-hooks.md)).
* `--watch` reads the store once and writes it after each round. `skif lsp` and `skif mcp` keep a memory cache per
  root, seeded read-only from the store on the first lint and pruned to what the last run used, and never write the
  store. The npm addon caches by default (`cache?: boolean | { location?, maxSize? }`) and keeps a memory cache per
  root across calls in a long-lived process.
* The disk store is the `skif-cache` crate behind the facade feature `cache-disk`: `DiskStore::open`, `finish_run`,
  `stats`, `clean` and the pure `default_location(base, user_cache_dir)`, which takes the platform directory from its
  host. The CLI, the addon and any embedder that wants skif's layout share it
  ([ADR 0002](0002-crates-library-api-and-upstream-code.md)).
* polylint runs skif in process over its own file system and passes `cache: {dir, salt}` to its component, which
  implements `CacheStore` over poly-cache. `poly cache stats`, `clean` and `gc` cover skif's entries, so skif needs no
  stats or clean API for poly.

### Consequences

* Good, because parsing, resolution and analysis run in one process with the parser and resolver knip itself uses, and
  running skif on an untrusted pull request reads files and executes nothing.
* Good, because an oxc upgrade touches only the JS crates, and drift from poly fails skif's scheduled CI first.
* Good, because `paths` resolve in the compiler's order, and module declaration files have their unused types reported.
* Good, because no store, a cold, a warm and a damaged store give the same findings, every host starts warm by
  default, poly re-runs only what changed instead of all of skif, and long-lived hosts and multi-root runs parse only
  what changed or what is shared once.
* Bad, because knip's edge cases are rediscovered through the comparison, not inherited.
* Bad, because skif's oxc upgrades follow poly's cadence, and the lockstep check needs the network.
* Bad, because the default walk pays one `opendir` per directory, and `git` has a blind spot by design.
* Bad, because a warm run still reads and hashes every file until the stat fast path, and the namespace schemas rely
  on tests and review.
* Bad, because the default writes below `node_modules/.cache` or the user's cache directory, bounded only by the cap,
  and the envelope does not authenticate, so keeping untrusted writers out of a store is each host's policy.

### Confirmation

* `scripts/check-oxc-git-patch.sh`, run by `.github/workflows/oxc-lockstep.yml` and `mise run oxc:lockstep`, and
  `scripts/check-pins.sh` hold the oxc lockstep; `scripts/check-dependency-tree.sh` holds which crates depend on oxc.
* `crates/skif-js/tests/paths_before_packages.rs`, `crates/skif-js/tests/fp_resolve.rs` and
  `crates/skif-js/tests/graph_fixes.rs` cover resolution order, ownership and declaration-file entries.
* `crates/skif-js/tests/git_discovery.rs`, `crates/skif-workspace/src/git_index.rs` and
  `crates/skif-js/tests/fixtures.rs` cover the listing modes, the index reader and fixture directories.
* `crates/skif-js-syntax/src/cache.rs` holds the schema digest and the coalescer; `crates/skif/tests/module_cache.rs`
  and `crates/skif-cli/tests/cache.rs` prove that no cache, a cold cache and a warm cache report alike.
* Library contract tests run without a store and with a cold, a warm, a corrupted and an adversarial store and require
  the same report apart from the counters; the CLI tests cover the settings' precedence, the location chain, the CI rule
  and `skif cache`.
* `skif-bench cache-diff` compares uncached, cold, warm and seeded-edit runs on the corpus, in CI
  ([ADR 0014](0014-benchmarks-and-the-comparison-harness.md)).
