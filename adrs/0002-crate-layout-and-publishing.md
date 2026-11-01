---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# Crate Layout and Publishing: Fourteen Crates, One Stable Facade

## Context and Problem Statement

The inherited code is one binary crate. Its rules, workspace collection, JSON handling, printer
and installer share a module tree, so nothing can be reused without the CLI, and a change to
terminal output recompiles the rule logic.

skif has to be embeddable ([ADR 0003](0003-library-first-embedding.md)), has a heavy and fast-moving dependency in oxc that most
consumers will not want ([ADR 0010](0010-js-analysis-on-oxc.md)), vendors upstream code that must stay isolated and attributed
([ADR 0006](0006-specifier-and-semver-model.md), [ADR 0015](0015-attribution-and-vendoring.md)), and publishes to crates.io as well as six other channels ([ADR 0013](0013-distribution-and-naming.md)). Those
four pressures each argue for a crate boundary at a specific place.

## Decision Drivers

* skif has to be embeddable without the CLI
* oxc is heavy and fast-moving, and most consumers will not want it
* Vendored upstream code must stay isolated and attributed
* Every crate must be publishable to crates.io alongside six other channels

## Considered Options

* A fourteen-crate Cargo workspace with one semver-stable facade
* One library crate plus a binary
* Unpublished internal crates with everything re-exported from `skif`
* Binary in the `skif` crate

## Decision Outcome

Chosen option: "A fourteen-crate Cargo workspace with one semver-stable facade", because each of the four pressures (embedding, oxc isolation, vendored code, crates.io publishing) argues for a crate boundary at a specific place, and a single stability promise on `skif` keeps internal refactors cheap.

The repository is a **Cargo workspace of fourteen crates** under `crates/`, resolver 3, with a
virtual root manifest that owns `[workspace.package]`, `[workspace.dependencies]`,
`[workspace.lints]` and the release profile.

| Crate | Responsibility |
| --- | --- |
| `skif-core` | Plain data (`Diagnostic`, `Span`, `Fix`, `FixOp`, `TextEdit`, `FileChange`, `FixPlan`, `RuleMeta`, `Report`) and the `FileSystem` trait with its `MemoryFs` and `OsFs` implementations. `OsFs` is the only type in the library crates that touches `std::fs` ([ADR 0003](0003-library-first-embedding.md)). |
| `skif-specifier` | Vendored syncpack specifier parser ([ADR 0006](0006-specifier-and-semver-model.md)). |
| `skif-config` | Typed, rule-agnostic `Config`, the compiled `Selector`, configuration loading and discovery through `FileSystem`, CLI overrides, JSON Schema ([ADR 0004](0004-configuration-model.md)). |
| `skif-edit` | `FixOp` → `TextEdit` with format-preserving JSON and YAML editing ([ADR 0008](0008-fix-and-edit-model.md)). |
| `skif-workspace` | Package-manager detection, workspace-root and workspace discovery, catalogs, packages addressed by `PackageId`, dependency entries with pre-parsed specifiers ([ADR 0005](0005-workspace-discovery.md)). |
| `skif-lint` | `Rule`, `RuleProvider`, `Registry` (which validates rule options), `LintContext`, `Emitter`, parallel run ([ADR 0007](0007-rule-and-diagnostic-model.md)). |
| `skif-rules` | `manifest/` rules and `policy/` rules, each rule with its own option type ([ADR 0009](0009-version-and-catalog-policies.md)). |
| `skif-js-syntax` | The oxc boundary: source file → `ModuleInfo` ([ADR 0010](0010-js-analysis-on-oxc.md)). |
| `skif-js-plugins` | `Plugin` trait, declarative plugin table, AST config resolvers ([ADR 0011](0011-plugin-model.md)). |
| `skif-js` | Module graph and the JS rules as a `RuleProvider` ([ADR 0010](0010-js-analysis-on-oxc.md)). |
| `skif-report` | human, json, sarif and github renderers ([ADR 0012](0012-reporting.md)). |
| `skif` | Library facade: the only semver-stable API. |
| `skif-cli` | The `skif` binary. |
| `skif-testing` | Fixture and redaction helpers for tests; `publish = false`. |

The dependency graph is acyclic and layered. Direct workspace dependencies:

| Crate | Depends on |
| --- | --- |
| `skif-core` | — |
| `skif-specifier` | — |
| `skif-config` | `skif-core` |
| `skif-edit` | `skif-core` |
| `skif-workspace` | `skif-core`, `skif-config`, `skif-specifier` |
| `skif-lint` | `skif-core`, `skif-config`, `skif-workspace` |
| `skif-rules` | `skif-lint`, `skif-specifier` (and their dependencies) |
| `skif-js-syntax` | `skif-core` |
| `skif-js-plugins` | `skif-js-syntax`, `skif-config` |
| `skif-js` | `skif-js-plugins`, `skif-lint`, `skif-workspace` |
| `skif-report` | `skif-core` |
| `skif` | everything above, the optional ones behind features ([ADR 0003](0003-library-first-embedding.md)) |
| `skif-cli` | `skif` with every feature enabled |
| `skif-testing` | `skif-core`, `skif-workspace` (dev-dependency only) |

Rules return fixes as `FixOp` values defined in `skif-core`; only the facade's `fix` feature pulls
in `skif-edit` to turn them into text edits and a `FixPlan` ([ADR 0008](0008-fix-and-edit-model.md)), so no rule crate depends on an editor.
Because `FileSystem` lives in `skif-core`, `skif-config` can load and discover configuration through
it without depending on `skif-workspace`, and `skif-workspace` can read the configuration's
`Selector`s without a cycle.

Rules for the graph:

* **`skif-core` depends on nothing in the workspace** and on no parser. Every other crate can name
  its types and read files through `FileSystem` without pulling in a parser.
* **Public enums that will grow are `#[non_exhaustive]`** (`FixOp`, `Category`, `Fixability`,
  `FileChange`, `JsonAction`, `YamlAction`, `PackageManager`, `DependencyKind` and the error enums), so adding a variant is not a breaking
  change of an internal crate, and every public type is `Send + Sync` by a compile-time assertion
  ([ADR 0003](0003-library-first-embedding.md)).
* **oxc is reachable only through `skif-js-syntax`, `skif-js-plugins` and `skif-js`.** No other
  crate lists an `oxc_*` dependency; the `js-analysis` feature of the facade is the single switch
  that brings them in ([ADR 0003](0003-library-first-embedding.md), [ADR 0010](0010-js-analysis-on-oxc.md)).
* **Terminal concerns live only in `skif-cli`**: clap, colour, prompts, file writes, exit codes.

**Every dependency, external and internal, is declared once in `[workspace.dependencies]`** and
referenced as `dep.workspace = true`; no crate names a version or feature set of its own. Internal
dependencies carry **both `path` and an exact `version`**
(`skif-core = { path = "crates/skif-core", version = "=0.1.0" }`), so a local build uses the path and
a published crate resolves exactly the sibling version it was built with; `cargo package` requires a
`version` on every path dependency. Feature sets such as `serde_json`'s `preserve_order` therefore
apply to every crate that uses the dependency, not only to the ones that remembered to ask. All
crates share one version ([ADR 0013](0013-distribution-and-naming.md)).

**All crates except `skif-testing` are published to crates.io**, leaves first in topological
order: `skif-specifier`, `skif-core`, `skif-config`, `skif-edit`, `skif-workspace`, `skif-lint`,
`skif-rules`, `skif-js-syntax`, `skif-js-plugins`, `skif-js`, `skif-report`, `skif`, `skif-cli`.
The publish job probes crates.io before each crate and skips one that already exists, and backs off
on HTTP 429, because crates.io rate-limits new crate names and a first release of thirteen crates
hits that limit.

**`skif` is the only crate with a stability promise.** The internal crates are published so that
`skif` and `skif-cli` can depend on them from crates.io, not as APIs; their documentation says so,
and a breaking change inside them is not a breaking change of skif as long as the facade is
unchanged. The binary is installed with `cargo install skif-cli` (or `cargo binstall skif-cli`),
and the installed executable is named `skif`.

### Consequences

* Good, because A host that wants only manifest and policy rules compiles no parser for JavaScript; the oxc
  dependency tree is opt-in.
* Good, because upstream code is physically separate: `skif-specifier` holds only vendored code and its
  attribution headers, which keeps license review to one directory.
* Good, because one stable crate gives a clear semver surface; internal refactors across thirteen crates do not
  need major version bumps.
* Good, because the facade's feature flags map onto crate boundaries, so a disabled feature removes whole crates
  from the build rather than `cfg`-gating code inside them.
* Bad, because thirteen published crates is a slow first release and a permanent publishing matrix; the
  idempotent, ordered publish job is a requirement, not a convenience.
* Bad, because publishing internals invites people to depend on them despite the documentation. The exact
  `=` version pins between siblings keep a mismatched set from resolving, which limits the damage.
* Bad, because more crates means more `Cargo.toml` files to keep consistent; workspace inheritance of package
  metadata, lints and dependencies is mandatory for every crate.

### Confirmation

* The `consistency` job in `.github/workflows/ci.yml` (repository checks implemented by scripts under `scripts/`, added with the workspace refactor) runs a check that every `Cargo.toml` under `crates/` declares dependencies only as `dep.workspace = true` and that every internal path dependency in the root `Cargo.toml` has an exact `=` version.
* The `build` job runs `cargo package --workspace --locked`, which fails for a path dependency without a `version`.
* The `deny` job (`mise run deny`) fails on a duplicate or banned dependency, and the `consistency` job ([ADR 0013](0013-distribution-and-naming.md)) fails when two crate versions disagree.
* `cargo tree -p skif --no-default-features -e normal` contains no `oxc_*` crate, and `cargo tree -p skif-core -e normal` lists no workspace crate; both are asserted by a script in the `consistency` job.
* The publish job ([ADR 0013](0013-distribution-and-naming.md)) probes crates.io before each crate and backs off on HTTP 429; its dry run on a pull request is the check that the publish order matches the dependency graph above.

## Pros and Cons of the Options

### A fourteen-crate Cargo workspace with one semver-stable facade

* Good, because every boundary maps to a real pressure: a consumer that wants only manifest rules compiles no oxc, vendored code is isolated to one directory, and the CLI's terminal dependencies stay out of the library.
* Good, because one stability promise on `skif` leaves the internal crates free to refactor.
* Bad, because thirteen published crates are a slow first release, a permanent publishing matrix and thirteen `Cargo.toml` files to keep consistent.

### One library crate plus a binary

* Good, because it is the least to build, publish and version.
* Bad, because oxc would be a dependency of every consumer, and feature flags inside one crate do not stop internal modules from depending on each other in ways that later make the split impossible.

### Unpublished internal crates with everything re-exported from `skif`

* Good, because only one crate would carry a public name and a stability promise.
* Bad, because crates.io does not allow a published crate to depend on an unpublished path dependency.

### Binary in the `skif` crate

* Good, because `cargo install skif` would install the binary, the most obvious command.
* Bad, because a library consumer would pull clap, a terminal UI and a prompt library to get the rules. `cargo install skif` installing nothing is the cost; the README names `skif-cli` as the install target.

## More Information

* Related decisions: [ADR 0003](0003-library-first-embedding.md), [ADR 0004](0004-configuration-model.md), [ADR 0005](0005-workspace-discovery.md), [ADR 0006](0006-specifier-and-semver-model.md), [ADR 0007](0007-rule-and-diagnostic-model.md), [ADR 0008](0008-fix-and-edit-model.md), [ADR 0009](0009-version-and-catalog-policies.md), [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0011](0011-plugin-model.md), [ADR 0012](0012-reporting.md), [ADR 0013](0013-distribution-and-naming.md), [ADR 0015](0015-attribution-and-vendoring.md)
* See the [ADR index](index.md).
