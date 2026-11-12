---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Library API and Embedding

## Context and Problem Statement

skif is hosted by other programs: poly (a polyglot linter that runs many engines in one process over a content cache),
the napi addon, and the language and MCP servers of the CLI ([ADR 0028](0028-editors-agents-and-git-hooks.md)). A host
needs to call a function, get diagnostics and proposed edits back, and decide itself what to print, cache and write.

A library that exits the process, prints, prompts, reads the environment or the working directory, or writes files
cannot be hosted that way, and a fix computed against the working directory edits the wrong file when the host runs
elsewhere. poly also pins oxc from git, so a library that exposes oxc types, or forces another oxc release onto the
host, cannot be built together with it. What contract does the library offer its hosts?

## Decision Drivers

* A host calls a function and gets diagnostics and proposed edits back
* No library code knows the process's working directory, environment or terminal
* All reads go through an abstraction a host can back with unsaved buffers
* No third-party type in the public API that would make a dependency upgrade a breaking change, and no oxc type at all
* The rule is checkable by a test, on the stable toolchain

## Considered Options

* Pure-function library crates with the CLI as one consumer, and a stated allow-list of third-party types
* Embed by spawning the CLI with `--format json`
* Expose oxc's AST and module record to hosts that already use oxc
* A callback-based reporter instead of a returned `Report`

## Decision Outcome

Chosen option: "Pure-function library crates with the CLI as one consumer, and a stated allow-list of third-party
types", because a library that never exits, prints, prompts, reads the environment or writes files can be hosted in
process, tested against an in-memory file system, and cannot edit a file relative to the wrong directory.

### Pure functions

* Every crate except `skif-cli` behaves as a pure function of its inputs. None of them exits the process, panics on user
  input, prints, prompts, reads the environment, the working directory or global state, or writes files.
* The library prints only into a timing sink the host installs; the phase timings are off otherwise. The CLI reads
  `SKIF_TIMINGS` and `--performance` and installs the sink.
* Every root path is explicit. Every path in a diagnostic or an edit is relative to the workspace root the caller
  passed, never to the process's working directory.
* Every fallible boundary returns `Result` with typed errors (`thiserror`).
* All I/O goes through the `FileSystem` trait of `skif-core`: `OsFs` on disk, `MemoryFs` for tests, and overlays such as
  `BufferFs` (unsaved buffers) and `CachedFs`. `OsFs` is the only library type that touches `std::fs`.
* Parallel work runs on an `Executor` the host passes, on a dedicated pool of a given size, or on the calling thread
  alone when one thread is asked for.

### Fixes are data

Rules return fixes; `apply_fixes` turns the selected ones into a `FixPlan` of file changes and returns it
([ADR 0009](0009-fixes-edits-and-package-json-formatting.md)). Writing the plan is the CLI's or the host's job. Choosing
interactively between fix alternatives is a CLI feature built on the alternatives a rule returns.

### The facade

The `skif` crate is the embedding surface:

* `find_workspace_root`, `load_config`, `resolve_config` with a `ConfigCache`, and `Workspace::discover_with_fs`;
* `discover_targets`, the one multi-root pipeline ([ADR 0006](0006-workspace-discovery-roots-and-multi-root-runs.md));
* `Linter::new`, `with_fs`, `with_module_cache`, `for_targets` and `shared_modules`; `Linter::new` validates every rule
  id and option, so a host never sees an option error at lint time;
* `Linter::run` and `run_with(workspace, &RunOptions)`, where `RunOptions` carries a cancel flag, the thread count or an
  `Executor`, a subset of rules, the configuration file's path and the files other roots import;
* `apply_fixes`, `settle_shared_config` ([ADR 0005](0005-the-js-table-ignore-lists-and-suppression-reasons.md)) and
  `skif::update` ([ADR 0026](0026-skif-update.md)).

### Features

* `manifest` is the default. It brings in `skif-rules`: the manifest and workspace rules and the version, semver and
  catalog policies. There is no separate policy feature.
* `js-analysis`, `fix`, `yaml-fix` (implies `fix`), `report`, `schema` and `update` (implies `manifest` and `fix`) are
  opt-in.
* `js-analysis` is off by default so that a host that pins its own oxc is never forced onto skif's.
* The CLI enables every feature.

### Third-party types in the public API

* A public item of `skif`, or of a crate it re-exports from, may name a type from `serde`, `serde_json` (a `Value` in
  JSON and YAML fix actions and manifest values), `toml` (the `Table` a provider validates options from), `indexmap`
  (the ordered maps of `Config`) and `nodejs_semver` (parsed ranges), and from nothing else outside skif and the
  standard library.
* `oxc_*` and `rayon` types never appear. A thread pool crosses the API as an `Executor`, and spans cross it as
  `skif_core::Span`, whose line and column fields and byte range are the shape poly's own span and edit use, so a host
  maps them without re-reading the file.
* A new major release of an allow-listed crate is a breaking release of `skif`.
* The list is written once, as `<!-- allow-list: ... -->` in the `skif` crate documentation.
* Public enums and result types that will grow are `#[non_exhaustive]`, so adding a variant or a field is not breaking.

### Consequences

* Good, because poly runs skif as an engine with no subprocess, no output parsing and its own cache.
* Good, because the bug class of fixes written relative to the working directory is gone by construction.
* Good, because every rule can be tested against `MemoryFs`, in parallel, with no fixture directory on disk.
* Good, because returned edits let a host show a diff, merge skif's edits with its own or refuse a fix.
* Good, because the allow-list is in one place and a test fails when a type outside it enters the API.
* Bad, because every capability that would be one line in a CLI (an environment variable, a warning on stderr) has to be
  threaded through a type.
* Bad, because the `FileSystem` trait is public API and adds an indirection to every read.
* Bad, because keeping oxc out of the API costs a conversion pass in `skif-js-syntax`.
* Bad, because the source scan that checks the allow-list misses a leak behind a type alias or macro output; review of a
  re-export stays the backstop.

### Confirmation

* `crates/skif/tests/public_api.rs`: `the_public_surface_names_only_allow_listed_third_party_crates` scans the public
  items of `skif` and of every crate it re-exports from;
  `the_audit_flags_a_type_from_a_crate_outside_the_allow_list` and
  `the_audit_flags_a_foreign_type_in_a_trait_method_and_a_re_export` prove the scan is not vacuous; and
  `the_documented_allow_list_is_the_one_the_adr_records` compares the scan's list with the crate documentation.
* The doctests of `crates/skif/src/lib.rs` run discovery, configuration, a multi-root discovery and a lint against
  `MemoryFs`.
* `crates/skif/tests/threads.rs`: `one_thread_runs_discovery_and_graph_building_on_the_calling_thread`.
* `crates/skif-workspace/tests/memory.rs`: `discover_with_fs_never_consults_the_current_directory`.

## Pros and Cons of the Options

### Pure-function library crates with the CLI as one consumer, and a stated allow-list of third-party types

* Good, because the CLI, the addon, the servers and poly all use the same API, so none of them can drift from the
  others.
* Bad, because the discipline costs plumbing for every host-level concern.

### Embed by spawning the CLI with `--format json`

* Good, because it needs no library API at all.
* Bad, because the host pays a process start per run, cannot share its cache or its buffers, and parses output.

### Expose oxc's AST and module record to hosts that already use oxc

* Good, because a host that parses the file already could skip a parse.
* Bad, because the host and skif would have to agree on one oxc release, and every oxc upgrade would break the API.

### A callback-based reporter instead of a returned `Report`

* Good, because a host could stream findings as they appear.
* Bad, because deterministic order needs every finding before sorting, and callbacks into host code make the run's
  threading the host's problem.
