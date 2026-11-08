---
status: accepted
date: 2026-11-08
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0056", "0064"]
---

# A Module Cache and Unsaved Buffers in the Language Server

## Context and Problem Statement

[ADR 0056](0056-editor-and-agent-integration-mcp-server-watch-mode-and-language-server.md) shipped `skif mcp`,
`--watch` and `skif lsp`, and deferred two things issue #44 still asks for: `--cache`, which persists analysis
between runs, and linting the editor's unsaved buffers. [ADR 0064](0064-code-actions-in-the-language-server.md)
added code actions, but it still lints only what is on disk. It handles `didChange` only to mark a document dirty,
and it withholds every fix for a dirty document.

Both changes touch the same seam. The JS analysis reads every source file through the `FileSystem` trait and parses
it with `skif_js_syntax::parse_module` (or `parse_stylesheet`) into a `ModuleInfo`, in the parallel step of
`skif_js::graph::process`. Everything after that is cheap compared with oxc's parse and semantic pass: resolving
imports, building the graph, computing usage and running the rules. A long-lived host (watch, the language server, the
MCP server) parses the same unchanged files again on every run. A language server that lints buffers also needs the
JS analysis and workspace discovery to read the buffer instead of the file.

What does skif cache, where is the cache kept, and how does the language server lint text that is not on disk
without the library doing I/O or the output depending on the cache?

## Decision Drivers

* The cache never changes output: a run with a cold cache, a warm cache or no cache gives the same report
* Invalidation must be correct by construction, not by tracking which inputs a result depended on
* The library stays I/O-free ([ADR 0003](0003-library-first-embedding.md)); persisting a cache is the CLI's job
* No oxc type, and no new third-party type, in the facade ([ADR 0032](0032-third-party-types-in-the-facade-api.md))
* No new crate in the dependency tree and no license exception
* A fix offered in the editor must apply at positions that hold the text it was planned against

## Considered Options

* Cache each file's parse result, keyed by its content, behind a `ModuleCache` the host gives the linter
* Cache the whole report, keyed by the inputs of the run
* Cache each file's parse result, keyed by path, size and modification time
* Lint buffers by writing them to a shadow directory
* Lint buffers through an overlay `FileSystem` (chosen for unsaved buffers, with the first option)

## Decision Outcome

Chosen option: "Cache each file's parse result, keyed by its content, behind a `ModuleCache` the host gives the
linter", because a parse result is a pure function of the file's name, its text and the parse options, together
with "Lint buffers through an overlay `FileSystem`" for unsaved buffers. A key that covers all three inputs can never
return a stale result, and everything else
(configuration, manifests, lockfiles, the git index, plugin configs, resolution, the graph and the rules) is computed
again on every run, as it is today.

### The cache unit and its key

The unit is the `ModuleInfo` that `parse_module` or `parse_stylesheet` returns for one file. The textual imports
that `add_textual_imports` adds afterwards are not part of it: they are recomputed from the text, which a run reads
anyway. The ambient-module scan of `.d.ts` files outside the graph (`collect_ambient_modules`) uses the same cache
with its own parse options.

A `ModuleKey` is a 128-bit SipHash-1-3, with fixed keys, of these inputs in order:

1. `CACHE_SCHEMA` (a `u32` in `skif-js-syntax`) and the `skif-js-syntax` crate version
2. the parser: module or stylesheet
3. the two `ParseOptions` flags, `in_file_references` and `member_accesses`
4. the file name (the last path component, which selects the language, `.d.ts` included)
5. the length of the text and the text itself

The directory is not part of the key, so identical files share an entry and moving a checkout keeps the cache
valid. The hash comes from the `siphasher` crate, which `phf_shared` already brings into the tree. A direct
dependency adds no crate and no license. Taking 128 bits makes a collision between two texts of one file negligible.
The 64-bit FNV `content_hash` that fix plans use is too narrow for a key that outlives a run.

`CACHE_SCHEMA` is raised whenever the parse output changes, so a cache written by a development build at the same
version is not trusted. A test holds the schema to that: it parses a fixed probe corpus, hashes the serialized
results and compares the hash with `CACHE_SCHEMA_DIGEST`, a constant beside the schema. When parse output changes,
the test fails and asks for both constants to be updated.

### The library boundary

`skif-js-syntax` owns the cache types, since it owns `ModuleInfo`:

* `ModuleKey` is a `Copy + Ord` value with `to_bytes() -> [u8; 16]` and `from_bytes`.
* `CachedModule` wraps a `ModuleInfo` with private fields and derives `Serialize` and `Deserialize`
  (`#[serde(transparent)]`). A host can store it without seeing `ModuleInfo`, and the facade does not make the
  fields of `ModuleInfo` a public contract.
* `trait ModuleCache: Send + Sync` has `fn get(&self, key: ModuleKey) -> Option<CachedModule>` and
  `fn put(&self, key: ModuleKey, module: CachedModule)`. Both are called from the parallel parse step. A failed
  lookup or a payload that does not decode is a miss, never an error.
* `MemoryModuleCache` is the in-process implementation. It is a mutex-guarded `BTreeMap` of `Arc<CachedModule>`
  with a mark for each entry used since the last prune. `prune_unused()` drops the unmarked entries and clears the
  marks, so a long-lived host keeps only what its last run read.
* `parse_cached(cache, path, source, options)` looks up the key, parses on a miss, puts the result and returns
  it. Both parse call sites in `skif-js` go through it.

The cache goes to the linter, not to `RunOptions`. `RunOptions` becomes `skif_lint::RunOptions` and `LintContext`.
Those types are agnostic of providers and cannot name a `skif-js-syntax` type unless `skif-lint` depends on the JS
crates. The JS provider is built per `Linter` together with its file system, and a cache of parse results is the
same kind of input as the git `IndexCache` the provider already owns. `JsProvider::with_module_cache(Arc<dyn
ModuleCache>)` stores it. `JsAnalysis::build_cached` takes it with the index cache and the extra entries, and
`Inputs` passes it to `graph::process`. The facade adds `Linter::with_module_cache(config, fs, modules)` under
`js-analysis` and re-exports `ModuleCache`, `ModuleKey`, `CachedModule` and `MemoryModuleCache`. `Linter::with_fs`,
`builtin_providers`, `explain_usage` and `trace_dependency` are unchanged and run without a cache. A host keeps the
`Arc` and passes it to every linter it builds, so the cache lives as long as the host decides.

With `SKIF_TIMINGS`, the graph step reports `js module cache <root>: H hits, M misses` beside the read, parse and
resolve times.

### `--cache` and `--cache-location`

`skif --cache` persists the cache between runs. It is opt-in, as in eslint and knip, so the default run's behaviour
and the files it writes are unchanged. `--cache-location <DIR>` names the directory and requires `--cache`; a
relative path is relative to the working directory. The default is `node_modules/.cache/skif/` below the directory
the report's paths are relative to (the workspace root, or the invocation directory of a multi-root run), the place
JS tools use by convention and that repositories already ignore. The directory is created when missing. A cache is
not a setting of the project, so there is no `skif.toml` key.

The store is one file, `modules.bin`, owned by `skif-cli` (`DiskModuleCache`):

```text
magic    8 bytes   "SKIFMOD\0"
format   u32 LE    1
version  u32 LE length, then UTF-8: the skif-js-syntax version and CACHE_SCHEMA, as "0.1.1+1"
count    u32 LE
records  count times: key [u8; 16], length u32 LE, payload (serde_json of CachedModule)
```

Records are sorted by key, so the file is a deterministic function of its entries. Loading reads the file once and
indexes the records without decoding them. `get` decodes a payload on demand, on the parse step's threads, and
`put` encodes on them too. Payloads are JSON because `serde_json` is already a dependency and only hits are
decoded. The format number lets a later record switch to a binary codec if a profile shows decoding matters.

* A missing file is an empty cache. A file with a wrong magic, format or version, a truncated record, a length past
  the end, or keys out of order is ignored as a whole and replaced on the next write. A single payload that does not
  decode is a miss. With `--debug` the reason is noted on stderr; otherwise the cache is silent.
* The file is written after the run, whatever its exit code, and only when the run read or wrote an entry and the
  set of entries changed. A fully warm run rewrites nothing. The file keeps exactly the entries the run used, so
  edits do not make it grow. A narrower run (`--production`, one root of a multi-root repository) drops the entries
  it did not use, which the next full run parses again.
* The write goes to `modules.bin.<pid>.tmp` in the same directory, then renames over `modules.bin`, so a concurrent
  run reads either the old file or the new one. The last writer wins and nothing is locked.
* A write failure prints `warning: cannot write the module cache to <path>: <reason>` on stderr and does not change
  the exit code. In JSON mode stdout is untouched.
* `--fix` and `--dry-run` use the cache like any run. The re-lint after a fix misses on the files the fix changed.

### Long-lived hosts

* `--watch` builds its cache once and passes it to every round: a `MemoryModuleCache`, or with `--cache` a
  `DiskModuleCache` loaded at start and written after every round. After each round the cache is pruned to the
  entries that round used. A `--cache-location` below the watched directory is added to the files the scan leaves
  out, as the report files are; the default location is under `node_modules`, which the scan never enters.
* `skif lsp` keeps one `MemoryModuleCache` per root and prunes it after each lint of that root.
* `skif mcp` keeps one `MemoryModuleCache` per canonical `path` and `recursive` pair of the `lint` tool and prunes it
  after each call. `explain_usage` stays uncached.

The language server and the MCP server never read or write `modules.bin`: a server's process lives long enough to
warm its own cache.

### Unsaved buffers in the language server

The server lints the text of open documents, not only the saved files.

* **Document sync.** `initialize` advertises `textDocumentSync.change: 1` (full) instead of incremental. The
  server needs the text now, and a full text needs no UTF-16 range arithmetic that could drift from the client's
  buffer. The text is bounded by the 64 MiB message limit of ADR 0056. The server keeps, per open document, its
  canonical path, its text and its version: from `didOpen` (`textDocument.text`), replaced by every `didChange`
  (the last content change carries the full text) and dropped on `didClose`. `didSave` keeps the text, since it now
  matches the file.
* **The overlay.** `BufferFs<F>` in `skif-core` is a `FileSystem` over a base file system and an immutable
  `BTreeMap<PathBuf, Arc<str>>` of buffers. `read_to_string` and `read_bytes` return the buffer for a path that has
  one and that the base reports as a file. Every other method, including `read_dir`, `kind`, `canonicalize` and
  `modified`, forwards to the base. A buffer whose file does not exist on disk is ignored until it is saved, so
  directory listings and the git index never disagree with the overlay. The server builds a snapshot per lint from
  the open documents, so nothing changes during a run, as `Linter` requires. `BufferFs` is pure and lives in the
  library, re-exported by the facade, because every editor host needs the same thing.
* **The lint.** `lint_root` takes the file system and the module cache: discovery (`roots::resolve`,
  `Workspace::discover_with_fs`) and the linters (`Linters::build_with`, over `Linter::with_module_cache`) all read
  through the overlay. Unsaved manifests, `pnpm-workspace.yaml`, `skif.toml` and source files are therefore linted
  as typed. Diagnostic positions are computed against the overlay's text, not `std::fs`.
* **Debounce.** `didChange` schedules a lint of the document's root 250 milliseconds after the root's last change.
  `didOpen` and `didSave` lint at once and cancel the root's pending lint. A reader thread (`std::thread::scope`)
  passes frames to the main loop over a channel. The loop waits with `recv_timeout` until the earliest deadline, so
  a burst of keystrokes causes one lint. Messages that arrive during a lint are handled after it, in order. Pending
  lints also run before `shutdown` is answered and when the input ends, so tests are deterministic without sleeping.
  A request does not flush pending lints: code actions are answered from the last lint, and the rule below keeps
  them correct.
* **Code actions for unsaved documents.** These are offered instead of withheld. For each root, the server
  records the `content_hash` of every document text the last lint read. A plan is offered when every file it edits
  either has no open document, or has an open document whose current text has the hash the lint recorded. In both
  cases the plan's `base_hash` must equal the hash of the current text. `apply_fixes` plans through the same
  overlay, so the edits are positions in the buffer. The `dirty` set of ADR 0064 is replaced by this comparison.
  When the client advertises `workspace.workspaceEdit.documentChanges`, the edit of an open document is a
  `TextDocumentEdit` carrying the document's version, so the client refuses it if the user typed in between.
  Otherwise the edit is `changes`, as before. Plans that create or delete files stay withheld.

Files changed on disk outside the editor are seen on the next lint of their root, as before.
`workspace/didChangeWatchedFiles` is still not handled.

### Not here

The VS Code extension stays in a separate repository, as ADR 0056 decided. Cancelling a running lint when a newer
change arrives needs the lint on a worker thread with `RunOptions::with_cancel`. That is left for a later record if
a lint is measured to block the editor.

### Consequences

* Good, because invalidation needs no dependency tracking: a changed file has a new key, and everything that is not
  a parse is recomputed.
* Good, because the library does no I/O. The disk format belongs to the CLI, and an embedder can supply its own
  store.
* Good, because unsaved manifests and sources get findings as they are typed, and fixes for them are offered at
  positions that are checked against the buffer.
* Good, because watch, the language server and the MCP server parse only what changed after their first run.
* Bad, because a warm run still reads and hashes every file, and resolution, the graph and the rules are not
  cached. The gain is bounded by the parse share of a run, which `SKIF_TIMINGS` shows.
* Bad, because a narrower run prunes entries a full run needs, so alternating runs re-parse files.
* Bad, because full document sync sends the whole document on every change.
* Bad, because the server lints the whole root after each burst of typing and answers nothing while it runs.
* Bad, because `CACHE_SCHEMA` depends on a test and on review to be raised when parse output changes.

### Confirmation

* `skif-js-syntax`: unit tests show the key changes with each input (text, file name, options, parser) and not with
  the directory. A round-trip test serializes the `ModuleInfo` of every fixture source of the crate and asserts that
  deserializing it gives an equal value. `the_cache_schema_matches_the_parse_output` checks the probe-corpus digest
  against `CACHE_SCHEMA_DIGEST`. `MemoryModuleCache` tests cover get, put and `prune_unused`.
* `skif-js`: a counting `ModuleCache` over a `MemoryFs` sees a miss for every module on the first build and a hit on
  the second. After one file is edited, only that file misses. The analyses compare equal.
* `skif` (`tests/module_cache.rs`): on the JS fixtures, the report without a cache, with a cold `MemoryModuleCache`
  and with the warm one are equal. A `BufferFs` with no buffers gives the same report as the base file system. A
  buffer of a manifest changes the report as writing the file would.
* `skif-cli` unit tests for `DiskModuleCache`: write then load round-trips. A truncated file, a wrong magic, another
  version and an unsorted file each load as empty. A fully warm run does not rewrite the file. Pruning keeps exactly
  the used entries.
* `crates/skif-cli/tests/cache.rs`: `--format json` without a cache, with `--cache --cache-location <tmp>` cold and
  then warm, gives three identical stdouts and creates `modules.bin`. A corrupted `modules.bin` gives the same stdout
  and is rewritten. `--cache-location` without `--cache` is a usage error with exit code 2.
* `crates/skif-cli/tests/lsp.rs`: `initialize` advertises full sync. After `didOpen` and a `didChange` that
  introduces a manifest problem, `shutdown` flushes the pending lint and the `publishDiagnostics` before its answer
  reports the problem at the buffer's position, while the file on disk is unchanged. A code action for that problem
  edits the buffer's positions. After a further `didChange` with no lint in between, the action is withheld. With
  `documentChanges` support, the edit carries the document version.
* `every_long_flag_is_in_the_cli_reference` fails when `--cache` or `--cache-location` is missing from the CLI
  reference.

## Pros and Cons of the Options

### Cache each file's parse result, keyed by its content, behind a `ModuleCache` the host gives the linter

* Good, because the key covers every input of the cached function, so a hit is always right.
* Good, because the expensive step is cached and the cheap, context-dependent steps are not.
* Bad, because every file is still read and hashed on a warm run.

### Cache the whole report, keyed by the inputs of the run

* Good, because a warm run with no changes would cost almost nothing.
* Bad, because the inputs of a report are every config, manifest, lockfile, ignore file, plugin config and git
  index entry the run consulted. Missing one returns a stale report, which breaks the first driver.

### Cache each file's parse result, keyed by path, size and modification time

* Good, because a hit needs only a `stat`, not a read.
* Bad, because modification times are coarse and are rewritten by checkouts. The text is needed for the textual
  imports anyway, and the `BufferFs` of the language server has no modification time for a buffer.

### Lint buffers by writing them to a shadow directory

* Good, because the library would need no change.
* Bad, because it copies the root, or links it, on every change. Paths in diagnostics would have to be mapped back,
  and the server would write files.

### Lint buffers through an overlay `FileSystem`

* Good, because discovery and the JS analysis already read only through the trait, so one wrapper covers manifests,
  configs and sources.
* Bad, because new files that were never saved are not linted until they exist on disk.

## More Information

* Related decisions: [ADR 0003](0003-library-first-embedding.md), [ADR 0010](0010-js-analysis-on-oxc.md),
  [ADR 0032](0032-third-party-types-in-the-facade-api.md), [ADR 0038](0038-git-index-file-discovery-and-resolver-probe-avoidance.md),
  [ADR 0056](0056-editor-and-agent-integration-mcp-server-watch-mode-and-language-server.md),
  [ADR 0064](0064-code-actions-in-the-language-server.md)
* Issue: [#44](https://github.com/Goldziher/skif/issues/44)
* See the [ADR index](index.md).
