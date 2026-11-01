# 0002 — Crate Layout and Publishing: Fourteen Crates, One Stable Facade

* Status: Accepted
* Date: 2026-11-01

## Context

The inherited code is one binary crate. Its rules, workspace collection, JSON handling, printer
and installer share a module tree, so nothing can be reused without the CLI, and a change to
terminal output recompiles the rule logic.

skif has to be embeddable (ADR 0003), has a heavy and fast-moving dependency in oxc that most
consumers will not want (ADR 0010), vendors upstream code that must stay isolated and attributed
(ADR 0006, ADR 0015), and publishes to crates.io as well as six other channels (ADR 0013). Those
four pressures each argue for a crate boundary at a specific place.

## Decision

The repository is a **Cargo workspace of fourteen crates** under `crates/`, resolver 3, with a
virtual root manifest that owns `[workspace.package]`, `[workspace.dependencies]`,
`[workspace.lints]` and the release profile.

| Crate | Responsibility |
| --- | --- |
| `skif-core` | Plain data: `Diagnostic`, `Span`, `Fix`, `FixOp`, `TextEdit`, `FileChange`, `RuleMeta`, `Report`. No IO. |
| `skif-specifier` | Vendored syncpack specifier parser (ADR 0006). |
| `skif-config` | Typed `Config`, file discovery, per-rule options, CLI overrides, JSON Schema (ADR 0004). |
| `skif-edit` | `FixOp` → `TextEdit` with format-preserving JSON and YAML editing (ADR 0008). |
| `skif-workspace` | Package-manager detection, workspace discovery, catalogs, the `FileSystem` trait (ADR 0005). |
| `skif-lint` | `Rule`, `RuleProvider`, `Registry`, `LintContext`, `Emitter`, parallel run (ADR 0007). |
| `skif-rules` | `manifest/` rules and `policy/` rules (ADR 0009). |
| `skif-js-syntax` | The oxc boundary: source file → `ModuleInfo` (ADR 0010). |
| `skif-js-plugins` | `Plugin` trait, declarative plugin table, AST config resolvers (ADR 0011). |
| `skif-js` | Module graph and the JS rules as a `RuleProvider` (ADR 0010). |
| `skif-report` | human, json, sarif and github renderers (ADR 0012). |
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
| `skif` | everything above, the optional ones behind features (ADR 0003) |
| `skif-cli` | `skif` with every feature enabled |
| `skif-testing` | `skif-core`, `skif-workspace` (dev-dependency only) |

Rules return fixes as `FixOp` values defined in `skif-core`; only the facade's `fix` feature pulls
in `skif-edit` to turn them into text edits (ADR 0008), so no rule crate depends on an editor.

Rules for the graph:

* **`skif-core` depends on nothing in the workspace** and on nothing that does IO. Every other
  crate can name its types without pulling in a parser.
* **oxc is reachable only through `skif-js-syntax`, `skif-js-plugins` and `skif-js`.** No other
  crate lists an `oxc_*` dependency; the `js-analysis` feature of the facade is the single switch
  that brings them in (ADR 0003, ADR 0010).
* **Terminal concerns live only in `skif-cli`**: clap, colour, prompts, file writes, exit codes.

Every internal dependency is declared once in `[workspace.dependencies]` with **both `path` and
`version`** (`skif-core = { path = "crates/skif-core", version = "=0.1.0" }`), so a local build uses
the path and the published crate resolves the exact sibling version. All crates share one version
(ADR 0013).

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

## Consequences

Positive:

* A host that wants only manifest and policy rules compiles no parser for JavaScript; the oxc
  dependency tree is opt-in.
* Upstream code is physically separate: `skif-specifier` holds only vendored code and its
  attribution headers, which keeps license review to one directory.
* One stable crate gives a clear semver surface; internal refactors across thirteen crates do not
  need major version bumps.
* The facade's feature flags map onto crate boundaries, so a disabled feature removes whole crates
  from the build rather than `cfg`-gating code inside them.

Negative / risks:

* Thirteen published crates is a slow first release and a permanent publishing matrix; the
  idempotent, ordered publish job is a requirement, not a convenience.
* Publishing internals invites people to depend on them despite the documentation. The exact
  `=` version pins between siblings keep a mismatched set from resolving, which limits the damage.
* More crates means more `Cargo.toml` files to keep consistent; workspace inheritance of package
  metadata, lints and dependencies is mandatory for every crate.

## Alternatives considered

* **One library crate plus a binary:** rejected — oxc would be a dependency of every consumer, and
  feature flags inside one crate do not stop internal modules from depending on each other in
  ways that later make the split impossible.
* **Unpublished internal crates with everything re-exported from `skif`:** rejected — crates.io
  does not allow a published crate to depend on an unpublished path dependency.
* **Binary in the `skif` crate:** rejected — a library consumer would pull clap, a terminal UI and
  a prompt library to get the rules. `cargo install skif` installing nothing is the cost; the
  README names `skif-cli` as the install target.
