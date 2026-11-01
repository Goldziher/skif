---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# Library-First Embedding: The CLI Is One Consumer of the Library

## Context and Problem Statement

poly, the polyglot linter used across these projects, is skif's first embedding host. poly runs
many engines in one process, in parallel, over a cache keyed on file contents, and reports through
its own diagnostic and edit types. To host skif it needs to call a function, get diagnostics and
proposed edits back, and decide itself what to print, cache and write.

The inherited code cannot be called that way. It exits the process from collection code
(`std::process::exit(1)` in `collect.rs` and seven times in `main.rs`), prints from rules and the
printer module, prompts the user from inside a rule (`inquire::Select` in
`multiple_dependency_versions`), reads and writes files at cwd-relative paths, and has no type that
represents a finding without also representing its fix having been applied.

poly also pins oxc from git at a specific revision. A library that exposes oxc types, or forces a
crates.io oxc onto it, would make the two impossible to build together.

## Decision Drivers

* poly must call a function and get diagnostics and proposed edits back, deciding itself what to print, cache and write
* The inherited code exits the process, prints, prompts and uses cwd-relative paths from inside rules
* poly pins oxc from git, so no oxc type may appear in a public signature

## Considered Options

* Pure-function library crates, with the CLI as one consumer of the library
* Embed by spawning the CLI with `--format json`
* Expose oxc's `Program` and `ModuleRecord` to hosts that already use oxc
* A callback-based reporter instead of a returned `Report`

## Decision Outcome

Chosen option: "Pure-function library crates, with the CLI as one consumer of the library", because a library that never exits, prints, prompts, reads the environment or writes files can be hosted in-process by poly, tested against an in-memory file system, and cannot reintroduce the cwd-relative fix bug class.

Every crate except `skif-cli` is a library that behaves like a pure function of its inputs.

Library crates **must not**:

* call `std::process::exit`, `panic!` on user input, or `abort`;
* print to stdout or stderr, or prompt;
* read environment variables, the current working directory, or any global or thread-local state;
* write files.

Library crates **must**:

* take every root path explicitly; every path in a diagnostic or edit is relative to the workspace
  root the caller passed, never to the process cwd;
* return `Result` with typed errors (`thiserror`) at every fallible boundary;
* make all public types that cross a thread boundary `Send + Sync`, asserted by a compile-time
  test per crate (`const _: fn() = || { fn assert<T: Send + Sync>() {} assert::<Linter>(); };`);
* read files only through the `FileSystem` trait in `skif-workspace` ([ADR 0005](0005-workspace-discovery.md)), with an OS
  implementation as the default and an in-memory one for tests and for hosts that hold unsaved
  buffers;
* run parallel work on a caller-supplied rayon `ThreadPool` when one is given (`Linter::run_with`),
  and on the global pool otherwise.

**Fixes are data.** A rule returns `Fix` values; `skif::apply_fixes` turns the selected fixes into a
`FixPlan` of `FileChange { path, edits: Vec<TextEdit> }` ([ADR 0008](0008-fix-and-edit-model.md)) and returns it. Writing the plan
to disk is the CLI's job, or the host's. An interactive choice between fix alternatives is a CLI
feature built on the alternatives a rule returns, not a prompt inside the rule.

**No oxc type appears in a public signature.** `skif-js-syntax` converts oxc's AST and module record
into skif-owned `ModuleInfo` values before returning ([ADR 0010](0010-js-analysis-on-oxc.md)). Spans cross the boundary as
`skif_core::Span` (byte offsets plus 1-based line and column), which is the shape poly's own `Span`
uses, so a host maps them without re-reading the file.

The facade `skif` exposes the embedding surface:

```rust
let config = skif::load_config(root, overrides)?;
let workspace = skif::Workspace::discover_with_fs(root, &config, fs)?;
let linter = skif::Linter::new(&config)?;
let report = linter.run_with(&workspace, &pool)?;
let plan = skif::apply_fixes(&workspace, &report, &selection)?;
```

Cargo features on `skif` select what is compiled:

| Feature | Default | Brings in |
| --- | --- | --- |
| `manifest` | yes | manifest rules |
| `policy` | yes | version, semver and catalog policies, `skif-specifier` |
| `js-analysis` | no | `skif-js`, `skif-js-plugins`, `skif-js-syntax`, oxc |
| `fix` | no | `skif-edit` and `apply_fixes` |
| `yaml-fix` | no | YAML editing for `pnpm-workspace.yaml` (implies `fix`) |
| `report` | no | `skif-report` renderers |
| `schema` | no | JSON Schema generation for `skif.toml` |

`skif-cli` enables all of them. `js-analysis` is off by default precisely because poly brings its
own oxc; a host turns it on only when it can accept skif's pinned oxc.

### Consequences

* Good, because poly can run skif as an engine with no subprocess, no output parsing and its own cache, and the
  cwd-relative fix bug class is gone by construction: no library code knows what the cwd is.
* Good, because tests can run every rule against an in-memory file system, in parallel, with no fixture
  directories on disk for the simple cases.
* Good, because returned edits make fixes reviewable and composable: a host can show a diff, merge skif's edits
  with its own, or refuse a fix, without skif knowing.
* Bad, because every capability that would be one line in a CLI (read an env var, print a warning) has to be
  threaded through a type. Non-fatal problems become diagnostics or a `warnings` list on `Report`.
* Bad, because the `FileSystem` trait is a public API and adds an indirection to every read; a host that
  implements it badly can make skif slow.
* Bad, because keeping oxc types out of the API costs a conversion pass in `skif-js-syntax` on every file.

### Confirmation

* A compile-time test per crate asserts `Send + Sync` for the public types that cross a thread boundary.
* Tests run every rule against the in-memory `FileSystem` implementation.
* Reviewers reject library code that calls `std::process::exit`, prints, prompts, reads the environment or cwd, or writes files.

## Pros and Cons of the Options

### Pure-function library crates, with the CLI as one consumer of the library

* Good, because it satisfies the decision drivers above; the benefits are listed under Consequences.

### Embed by spawning the CLI with `--format json`

* Bad, because it costs a process per run, a binary version that can drift from the host, and no access to edits or to in-memory buffers.

### Expose oxc's `Program` and `ModuleRecord` to hosts that already use oxc

* Bad, because it ties skif's semver to oxc's weekly breaking releases and breaks the moment the host's oxc revision differs from skif's.

### A callback-based reporter instead of a returned `Report`

* Bad, because a returned value is deterministic, sortable and trivially testable; streaming can be layered on later without changing the rules. (rejected for the primary API)

## More Information

* Related decisions: [ADR 0005](0005-workspace-discovery.md), [ADR 0008](0008-fix-and-edit-model.md), [ADR 0010](0010-js-analysis-on-oxc.md)
* See the [ADR index](index.md).
