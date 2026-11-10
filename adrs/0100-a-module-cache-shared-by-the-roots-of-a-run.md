---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0066"]
---

# A Module Cache Shared by the Roots of a Run

## Context and Problem Statement

[ADR 0066](0066-a-module-cache-and-unsaved-buffers-in-the-language-server.md) caches the `ModuleInfo` of each source
file behind the `ModuleCache` trait. A host gives the cache to `Linter::with_module_cache`, and `--watch`, `skif lsp`,
`skif mcp` and `--cache` do. Every other linter, including those of a plain `skif -r`, parses without one.

A multi-root run lints each root with its own analysis. A directory of shared code outside the roots (a `shared/`
that several standalone packages import with `../shared`) is reached by every root that imports it, and each of them
parses all of it again: with 40 roots, the shared code is parsed 40 times. The roots are linted in parallel, so the
roots reach a shared file at about the same time. Even a cache shared by all of them misses for each until the first
parse is stored, and they all parse it.

Most multi-root repositories share little code across roots. There a cache over every file costs without paying
back: it keeps each root's parse results until the run ends, copies each into the store and out again, and takes a
lock per file. Coalescing concurrent parses of one key needs coordination. Where that coordination lives decides
whether a `ModuleCache` that wraps another can silently lose it.

Which files does the cache of a multi-root run hold, how do concurrent lookups of one file wait for one parse, and
what changes in the contract ADR 0066 gave hosts?

## Decision Drivers

* A multi-root run parses code its roots share once, not once per root, also while the roots run in parallel
* A run whose roots share no code, and every single-root run, costs what it did without the shared cache
* The cache never changes a report, as in ADR 0066
* A host's `ModuleCache` implementation stays two methods; coalescing does not depend on how a cache is wrapped
* The coordination of one file never blocks or wakes the lookups of another, and it cannot deadlock the parse step

## Considered Options

* A run cache of the files more than one root can reach, with a parse coalescer beside the cache
* A run cache of every file
* Coalescing as a provided `ModuleCache` method that wrappers forward
* One lock and one condition variable for all keys being parsed
* No shared cache: each root parses what it reaches

## Decision Outcome

Chosen option: "A run cache of the files more than one root can reach, with a parse coalescer beside the cache",
because it parses shared code once, leaves a run without shared code as it was, and keeps the trait as ADR 0066 left
it.

### The policy, in one place

`skif_js::SharedModules` is what the linters of one run parse through. The facade re-exports it.
`SharedModules::for_run(roots, host)` holds the whole policy. `roots` are the workspace roots of the run's targets,
and `host` is the host's cache if it keeps one:

| `host` | roots | Result |
| --- | --- | --- |
| given | one | the host's cache for every file, no coalescing (as `Linter::with_module_cache`) |
| given | several | the host's cache for every file, coalesced |
| none | one | `None`: no cache, as before |
| none | several | a run cache in memory for the files more than one root can reach, coalesced |

The facade wraps it as `Linter::shared_modules(targets, host)`. `Linter::with_shared_modules(config, fs, &modules)`
builds one linter over the result, and `JsProvider::with_shared_modules` takes it in `skif-js`.
`Linter::for_targets(targets, fs)` builds one linter per target through `Linter::shared_modules(targets, None)`, and
`Linter::for_targets_with_cache` passes the host's cache. The command line keeps building one linter per distinct
configuration and passes each the same `Linter::shared_modules` result. The library and the CLI therefore share one
policy, and a single target pays nothing new. `Linter::with_fs` and `Linter::with_module_cache` are unchanged.
`explain_usage` and `trace_dependency` still run without a cache.

### What the run cache holds

The run cache holds only files that can be shared. A file is local to the root being linted when it lies below that
root and below no other root of the run nested in it. A local file is parsed outside the cache. It is not hashed,
looked up, stored or copied. Every other file goes through the cache:

* a file outside the root, such as shared code outside every root or a sibling root's file,
* a file of a root nested in the one being linted, which that root reaches across its boundary.

A nested root's own files are local to it, even though an enclosing root also contains them. The enclosing root
stores the ones it reaches, and the nested root parses its own files without a lookup. A file that one root owns and
another root reaches is therefore parsed twice at most: once by its owner, outside the cache, and once through the
cache, which then serves every other root that reaches it. The rule is decided per lookup from the root being
linted (`Workspace::root`) and the list of the run's roots, so a linter that lints several roots with equal
configurations applies it to each.

The run cache is a `MemoryModuleCache` that lives as long as the linters holding the `SharedModules`, and is freed
with them. A host's cache (`--cache`, `--watch`, `skif lsp`, `skif mcp`) holds every file, as before, because it
serves later runs, in which a file's owner finds it too.

`MemoryModuleCache` keeps each entry behind an `Arc` and copies it outside its lock. The copy on a hit and the copy
into the store on a fresh parse remain. The analysis takes the `ModuleInfo` by value and extends it with the textual
imports of the file. Sharing it behind an `Arc` would change `CachedModule`, the trait and the graph's `Module` for
the one case where a copy is cheaper than the parse it replaces. With local files outside the cache, a run without
shared code copies nothing.

### Coalescing beside the cache

`skif_js_syntax::ParseCoalescer` coordinates the parses of one run. It is not a `ModuleCache`.
`ParseCoalescer::parse_cached(cache, path, source, options)` takes the cache as an argument, as `parse_cached` does.
The `ModuleCache` trait keeps exactly ADR 0066's `get` and `put`. A cache that wraps another implements those two
methods and has nothing else to forward. No wrapper can drop the coordination, and no implementation has an obligation
beyond `get` and `put`.

A lookup through the coalescer:

1. asks the cache, and returns a hit at once, with no other lock;
2. otherwise takes the coalescer's map, `Mutex<HashMap<ModuleKey, Arc<Slot>>>`, only to find or insert the key's
   slot. A slot holds the claiming thread's id, a waiter count, a `Mutex<Option<Outcome>>` and its own `Condvar`;
3. if another thread holds the slot, it registers as a waiter under the map's lock, releases the lock and waits on
   the slot's condition variable, so a wait for one key neither blocks nor wakes another;
4. if it inserted the slot, it asks the cache again with the key claimed: a parse that finished since step 1 stored
   its result before releasing its key. On a miss it parses and stores the result, then takes the slot out of the
   map. Only if a thread waits does it hand over the outcome, the parsed module behind an `Arc` or the parse error,
   and wake the waiters. The count of waiters only changes under the map's lock, so it is final once the slot is out
   of the map;
5. if the slot is its own, it parses inline.

A waiter is handed a parse error too, so the waiters of a file that fails do not parse it again one after another.
When the parse panics, a drop guard releases the claim with an outcome of `Abandoned`. The waiters then look up the
key again, one of them claims it and parses, and the panic itself unwinds on the thread that raised it.

Deadlock freedom rests on one invariant, which the type's documentation states: a parse never waits for another
lookup through the coalescer. The parsers run on the calling thread, use no thread pool and do not read the cache,
so a claimer always finishes. A waiter blocks its worker thread, never the claimer's. A re-entrant lookup of a key
the calling thread holds is detected by its thread id and parses inline instead of waiting for itself.

### Timings

With `SKIF_TIMINGS` (`--performance`), each lookup through a cache counts as one of three:

* a hit: served by the cache;
* a miss: parsed by this lookup, or a parse error;
* a wait: handed the parse of another root.

The line is `js module cache <root>: H hits, M misses, W waits`. Local files of a run cache are not lookups, so they
are not counted. The time of a wait goes to a separate `js parse wait <root>` line, printed when the root waited, and
`js parse` keeps only the time spent parsing or looking up. The counters are process-wide and shared by roots linted
in parallel, as before, so only their sums over the run are exact. Whether a lookup hits or waits depends on timing,
but their sum does not. `skif_core::timing::Tally::add` takes the duration the caller measured, since a lookup learns
which tally it belongs to only once it has been answered.

### Consequences

* Good, because shared code is parsed once per run, also when the roots reach it at the same moment.
* Good, because a run without shared code, and every single-root run, does no hashing, locking, storing or copying
  it did not do before.
* Good, because the trait is unchanged: hosts' caches keep working and cannot lose coalescing by wrapping.
* Good, because a failed parse is handed to the waiters instead of being repeated by each.
* Bad, because a file one root owns and another reaches is parsed twice, once by each.
* Bad, because the run cache keeps the shared files' results until the run ends.
* Bad, because a hit still copies a `ModuleInfo`.
* Bad, because the deadlock freedom of the coalescer depends on the parsers staying free of thread pools and cache
  lookups, which the type's documentation and review keep true.

### Confirmation

* `skif-js-syntax` (`cache.rs`): with a claimer that blocks until a second lookup of the same key is registered as
  waiting, and a waiter whose own parse panics, the waiter is handed the claimer's module, the store sees one `put` and
  no hit, and the coalescer is empty afterwards. A claimer that returns a parse error hands that error to the waiter,
  stores nothing and releases the key, so the next lookup parses. A claimer that panics leaves the waiter to parse
  once and store its own result. Eight threads asking for one key, the claimer holding it until the seven others wait,
  parse once. A lookup of a key its own thread holds parses inline. `ParseCoalescer::parse_cached` stores once and
  returns `Unsupported` for a file it cannot parse.
* `skif-js` (`shared_modules.rs`): one root without a host cache shares nothing; only several roots coalesce; a host's
  cache holds every file. Over the roots `/r`, `/r/nested` and `/r/sibling`, each root parses its own files outside
  the run cache and finds there the files of other roots and of shared code that another root stored.
* `skif` (`tests/module_cache.rs`): linters from `Linter::for_targets_with_cache` linting three roots in parallel store
  each file once; `Linter::for_targets` reports what a linter per target reports; `Linter::shared_modules` is `None`
  only for one target without a host cache.
* `crates/skif-cli/tests/cache.rs`: `skif -r --performance` over three roots that import one shared file parses that
  file once and serves it twice without `--cache`, parses all four files once with a cold `--cache` and none with a
  warm one, and the three reports are equal.
* `crates/skif/benches/ondisk.rs`: `shared_closure_on_disk` lints 10 and 40 roots over one shared directory with a
  linter per root and with `Linter::for_targets`; `recursive_on_disk/cached_for_targets` lints the generated
  repository, whose roots share little code, as `cached` does with the run cache added.
* `adrlint lint adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### A run cache of the files more than one root can reach, with a parse coalescer beside the cache

* Good, because only files that can be shared pay for the cache.
* Good, because coalescing is explicit at the call site and independent of the cache's implementation.
* Bad, because a file owned by one root and reached by another is not shared.

### A run cache of every file

* Good, because every file reached by two roots, owned or not, is parsed once.
* Bad, because it keeps and copies every parse result of the run, and a repository whose roots share no code pays
  for it in memory and time.

### Coalescing as a provided `ModuleCache` method that wrappers forward

* Good, because `parse_cached` keeps one entry point.
* Bad, because a wrapper that implements only `get` and `put`, such as a counting or logging layer, silently gets the
  provided method and loses coalescing. Every implementation has to know to forward it, and the trait changes.

### One lock and one condition variable for all keys being parsed

* Good, because it is the smallest implementation.
* Bad, because every finished parse wakes every waiter of every key. A lookup under the lock that asks the cache again
  serializes the cache behind the coordination.

### No shared cache: each root parses what it reaches

* Good, because nothing changes.
* Bad, because shared code costs its parse once per root.

## More Information

* Related decisions: [ADR 0003](0003-library-first-embedding.md),
  [ADR 0054](0054-several-reports-in-one-run.md),
  [ADR 0066](0066-a-module-cache-and-unsaved-buffers-in-the-language-server.md)
* Issue: [#144](https://github.com/Goldziher/skif/issues/144)
* See the [ADR index](index.md).
