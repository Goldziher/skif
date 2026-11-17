---
status: accepted
date: 2026-11-17
decision-makers: ["Na'aman Hirschfeld"]
---

# Crates, Library API and Upstream Code

## Context and Problem Statement

skif is hosted by other programs: poly (a polyglot linter that runs many engines in one process over a content cache),
the napi addon, and the CLI's language and MCP servers. A host calls a function, gets diagnostics and proposed edits
back, and decides itself what to print, cache and write. A library that exits, prints, prompts, reads the environment
or the working directory, or writes files cannot be hosted that way.

skif depends on oxc, which is heavy, moves fast and is unwanted by hosts that need only manifest and policy rules, and
on tsrs, a pre-release port of the TypeScript checker that needs unsafe code at its boundary. It vendors, ports and
re-implements code from MIT, ISC and Apache-2.0 upstreams whose notices must travel with every artefact. How is the code
split into crates and published, what contract does the library offer, and how is upstream code used and credited?

## Decision Drivers

* Embedding without the CLI, manifest and policy rules without oxc, and JS analysis without tsrs
* No library code knows the process's working directory, environment or terminal; all reads go through an abstraction
* No third-party type in the public API that would make a dependency upgrade a breaking change, and no oxc type at all
* One semver promise, so internal refactors stay cheap
* Every copied line traceable to its upstream, commit and licence, with invariants checked by machines

## Considered Options

* A layered Cargo workspace of pure-function library crates behind one semver-stable facade with an allow-list of
  third-party types, and upstream code depended on, vendored, re-implemented or referenced with one attributions file
* One library crate plus a binary, rejected because oxc would reach every consumer and features do not stop modules
  from entangling
* Embed by spawning the CLI with `--format json`, rejected because a host pays a process per run, cannot share its cache
  or buffers and parses output
* Expose oxc's AST and module record to hosts, rejected because host and skif would have to agree on one oxc release

## Decision Outcome

Chosen option: "A layered Cargo workspace of pure-function library crates behind one semver-stable facade with an
allow-list of third-party types, and upstream code depended on, vendored, re-implemented or referenced with one
attributions file", because every crate boundary maps to a real pressure, a library that never exits, prints or writes
can be hosted in process and tested against an in-memory file system, and every licence obligation sits in one file
every release ships.

### Layering

* The repository is a Cargo workspace under `crates/` with a virtual root manifest that owns `[workspace.package]`,
  `[workspace.dependencies]`, `[workspace.lints]` and the profiles. The crate graph is acyclic and layered.
* `skif-core` holds plain data and the `FileSystem` and `CacheStore` traits, depends on no workspace crate and on no
  parser, so every crate can name its types without pulling in a parser.
* Only `skif-js-syntax`, `skif-js` and the unpublished benchmark harness depend on an oxc crate directly; every other
  crate reaches oxc only through `skif-js-syntax`.
* `skif-js-types` is the only crate that depends on tsrs, and `skif-js` reaches it only behind its `type-aware`
  feature ([ADR 0010](0010-type-aware-analysis-on-tsrs.md)).
* Terminal concerns (clap, colour, prompts, file writes, exit codes) live only in `skif-cli`. `skif-node` calls into
  `skif-cli` to run the same command line in process.
* `unsafe_code` is forbidden in `[workspace.lints]`. `skif-js-types` and `skif-node` deny it instead: `skif-node` for the
  Node-API glue `napi-derive` generates, and `skif-js-types` for the one hand-written unsafe call, which frees a tsrs
  program and is allowed where it is made with its reason beside it.
* There is no shared test-helper crate; fixtures and helpers sit next to the tests that use them.

### Dependencies and publishing

* Every dependency, external and internal, is declared once in `[workspace.dependencies]` and inherited with
  `workspace = true`, so a feature set applies to every crate that uses it.
* An internal dependency carries both `path` and an exact `=` version, so a published crate resolves exactly the sibling
  it was built with. The oxc crates share one release under caret ranges (`oxc_resolver` has its own line); the tsrs
  crates are exact `=` pins to one version.
* All crates share one version ([ADR 0015](0015-distribution-release-and-supported-versions.md)).
* Every crate except `skif-node` (which reaches users only through npm) and the benchmark harness is published to
  crates.io, leaves first in topological order, by a script that skips a version already published, so a re-run
  completes a partial release.
* `skif` is the only crate with a stability promise. The other crates are published so that `skif` and `skif-cli` can
  depend on them; their documentation says they are not an API. `cargo-semver-checks` runs advisory on every published
  crate, because the facade re-exports the others.
* Every published crate ships copies of the root `LICENSE` and `ATTRIBUTIONS.md` and lists them in `include`.
* Types shared between workers or kept by hosts as long-lived values assert `Send + Sync` at compile time.

### Pure functions

* Every crate except `skif-cli` and `skif-cache` behaves as a pure function of its inputs. None exits the process,
  panics on user input, prints, prompts, reads the environment, the working directory or global state, or writes files.
* `skif-cache`, the disk cache store behind the facade feature `cache-disk`, is the one library crate that reads and
  writes files outside `FileSystem`, scoped to its own cache directory. Without it the CLI, the npm addon and every
  embedder that wants skif's own layout would each re-implement the layout, eviction and integrity rules
  ([ADR 0007](0007-js-analysis-core-on-oxc.md)). It reads no environment: the host passes the location and the
  platform cache directory.
* The one exception to "no environment input" is the type checker: the `TSRS_*` and `RAYON_NUM_THREADS` variables a
  user sets reach tsrs. skif passes the lib path and checker count explicitly and never sets or clears these variables.
* The library prints only into a timing sink the host installs; the CLI installs one for `SKIF_TIMINGS` and
  `--performance`.
* Every root path is explicit. Every path in a diagnostic or an edit is relative to the workspace root the caller passed.
* Every fallible boundary returns `Result` with typed errors.
* All I/O goes through `FileSystem`: `OsFs` on disk (the only library type outside `skif-cache` that touches
  `std::fs`), `MemoryFs` for tests, and overlays such as `BufferFs` (a host's unsaved buffers) and `CachedFs`.
* Cache entries go through the `CacheStore` a host gives the linter (`Linter::with_cache_store`). `CacheStore` and
  `CacheNamespace` are public API of `skif-core`, re-exported by the facade; keys cross it as `[u8; 32]` and values as
  `Arc<[u8]>`, so no hashing or storage crate's type is in the API. A linter given no store caches nothing beyond the
  run, and one given a store writes no file itself. poly implements `CacheStore` over its own cache.
* Parallel work runs on an `Executor` the host passes, on a dedicated pool of a given size, or on the calling thread
  alone when one thread is asked for.
* Fixes are data: rules return them, `apply_fixes` returns a `FixPlan`, and writing it is the CLI's or the host's job
  ([ADR 0005](0005-rules-diagnostics-and-fixes.md)). `Linter::new` validates every rule id and option, so a host never
  sees an option error at lint time.

### Features

* `manifest` is the default: the manifest and workspace rules and the version, semver and catalog policies.
* `js-analysis` is opt-in, so a host that pins its own oxc is never forced onto skif's. `type-aware` (implies
  `js-analysis`) adds the tsrs checker; it is opt-in for the same reason, and a build without it reports every package
  untyped with the reason `not-built`.
* `plain-ptrs` makes tsrs use plain pointers instead of its fixed address-space reservation, for hosts whose address
  space holds reservations of their own, such as Node.js; it has an effect only with `type-aware`.
* `cache-disk` is opt-in and adds `skif-cache` for hosts that want skif's disk store.
* `fix`, `yaml-fix`, `report`, `schema` and `update` are opt-in and map onto crate boundaries or crate features
  (`schema` is a `skif-config` feature, `update` is `manifest` plus `fix`).
* The CLI enables every feature except `plain-ptrs`, with `type-aware` behind its own default feature. The npm addon
  enables `type-aware` and `plain-ptrs`.

### Third-party types in the public API

* A public item of `skif`, or of a crate it re-exports from, may name a type from `serde`, `serde_json` (a `Value` in
  JSON and YAML fix actions and manifest values), `toml` (the `Table` a provider validates options from), `indexmap`
  (the ordered maps of `Config`) and `nodejs_semver` (parsed ranges), and from nothing else outside skif and the
  standard library.
* `oxc_*` and `rayon` types never appear. A thread pool crosses the API as an `Executor`, and spans cross it as
  `skif_core::Span`, whose line, column and byte range are the shape poly's span and edit use.
* A new major release of an allow-listed crate is a breaking release of `skif`.
* The list is written once, as `<!-- allow-list: ... -->` in the `skif` crate documentation.
* Public enums and result types that will grow are `#[non_exhaustive]`.

### Upstream code

Each upstream is used in exactly one of four ways:

* Depend on a published crate when its API fits skif's constraints as it is: thread safety, no global state, no I/O in
  library code. oxc, `oxc_resolver`, `jsonc-parser`, `yamlpatch`, `nodejs-semver` and tsrs are dependencies.
* Vendor when the code is right but its shape is not and the changes are not upstream concerns. syncpack's specifier
  crate lives in `skif-specifier` at a recorded commit, changed only by the modification list in `ATTRIBUTIONS.md`:
  `Rc` becomes `Arc`, the thread-local caches go (a caller-owned cache interns specifiers instead), `node-semver` is
  replaced by `nodejs-semver`, validation is eager so an oversized number yields `Unsupported` instead of a panic, and
  `Ord` breaks ties on the raw string so it agrees with `Eq`. A re-vendor is one commit that moves the commit,
  re-applies the list and updates the attribution.
* Re-implement when skif needs the behaviour, not the code, or the code is in another language: syncpack's
  version-group state machine and all of knip. The upstream at a recorded commit is the specification, and its output
  on the corpus is the oracle ([ADR 0014](0014-benchmarks-and-the-comparison-harness.md)).
* Reference only, reading for ideas with no code, tests or data copied: fallow.

### Attribution

* Every vendored file starts with a header naming the upstream, the file, the commit and the licence, or marks itself as
  added for skif. Code ported from sherif and re-implemented from knip carries no per-file header; its section covers
  it. A test or fixture adapted from knip names the knip fixture and commit in a header, or in a sibling `ATTRIBUTION`
  note for data that cannot hold comments.
* `ATTRIBUTIONS.md` has one section per upstream that code or data came from: source, commit, licence, authors and
  copyright, location, purpose, a `### Modifications` list and the full licence text. A section is added in the change
  that first copies from an upstream and removed only when nothing from it remains.
* A dependency whose licence requires a NOTICE has its section too. The tsrs section states that no tsrs or TypeScript
  source is copied and carries tsrs's full NOTICE, because the binaries and the npm addon embed tsrs and TypeScript's
  library declarations.
* skif's `LICENSE` is MIT and names skif's author only; upstream notices are never merged into it. Every crate, release
  archive and npm package ships `LICENSE` and `ATTRIBUTIONS.md`.
* The `deny.toml` licence allow-list holds permissive licences only. A dependency outside it is avoided rather than
  excepted; a licence is added only for a dependency the analysis cannot do without, with a comment naming it.

### Consistency checks

`mise run consistency` and the CI `consistency` job run the repository guards, each a script under `scripts/` with its
own tests in `scripts/test-check-scripts.sh`:

* `check-versions.sh`: one version across crates, internal pins, npm packages, hook pins and the pre-commit launcher;
* `check-pins.sh`: oxc caret ranges on one release, tsrs exact pins to one version, every member dependency inherited;
* `check-headers.sh`: vendored-file headers and the required `ATTRIBUTIONS.md` sections;
* `check-package-contents.sh`: each crate's `LICENSE` and `ATTRIBUTIONS.md` equal the root copies and are packaged;
* `check-rule-ids.sh`: released rule ids are never dropped ([ADR 0005](0005-rules-diagnostics-and-fixes.md));
* `check-dependency-tree.sh`: the layering above;
* `check-schema.sh`: the committed `schema/skif.schema.json` equals `skif schema`;
* the documentation guards `check-doc-pins.sh`, `check-tool-version-docs.sh`, `check-plugin-list.sh` and
  `check-support-matrix.sh`, owned by the records of their topic.

The `deny` job bans duplicate versions (with documented skips), unknown registries and git sources, and checks
licences and advisories.

### Consequences

* Good, because a host that wants only manifest and policy rules compiles no JavaScript parser, and one that wants JS
  analysis without types compiles no type checker.
* Good, because poly runs skif as an engine with no subprocess and keeps skif's cache entries in its own cache, and
  fixes cannot be written relative to the wrong directory.
* Good, because every rule can be tested against `MemoryFs`, and one stable crate is a clear semver surface.
* Good, because every vendored line can be traced to a source, commit and licence, and every artefact carries the
  notices.
* Bad, because many published crates make a permanent publishing matrix; the ordered, idempotent publish script is
  required, and published internals invite direct use despite the documentation.
* Bad, because every host-level concern (an environment variable, a warning) has to be threaded through a type, and
  keeping oxc out of the API costs a conversion pass.
* Bad, because a user's `TSRS_*` or `RAYON_NUM_THREADS` variable can change a typed run.
* Bad, because `skif-cache` makes "no file writes" a rule with a scoped exception that review has to keep scoped.
* Bad, because the shell checks read TOML textually, and the source scan of the allow-list misses a leak behind a type
  alias or macro output, so review stays the backstop.

### Confirmation

* `crates/skif/tests/public_api.rs` scans the public items of `skif` and every crate it re-exports from for types outside
  the allow-list, proves the scan is not vacuous, and requires exactly one record with this record's allow-list heading
  naming every listed crate.
* The doctests of `crates/skif/src/lib.rs` run discovery, configuration, a multi-root discovery and a lint against
  `MemoryFs`; `crates/skif/tests/threads.rs` and `crates/skif-workspace/tests/memory.rs` cover the single-thread path and
  the independence from the working directory.
* The CI `features` job checks and tests `skif` with no feature and with each feature alone except `type-aware` and
  `plain-ptrs`.
* The scripts above pass in the `consistency` job, and `scripts/test-check-scripts.sh` proves each fails on a broken
  input; `scripts/test-release-scripts.sh` covers the publish order and the crates.io probe with stubs.
* The `build` job runs `cargo package --workspace --locked`, which fails for a path dependency without a version.
* `scripts/check-archive.sh` and `scripts/check-npm-tarballs.sh` require `LICENSE` and the attributions, with the tsrs
  section, in every release archive and npm package.
