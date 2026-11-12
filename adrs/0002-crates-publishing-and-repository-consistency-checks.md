---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
---

# Crates, Publishing and Repository Consistency Checks

## Context and Problem Statement

skif has to be embeddable without its CLI ([ADR 0003](0003-library-api-and-embedding.md)). It depends on oxc, which is
heavy, moves fast and is unwanted by most hosts that only need manifest and policy rules. It vendors upstream code that
must stay isolated and attributed ([ADR 0007](0007-specifier-and-semver-model.md),
[ADR 0033](0033-attribution-and-vendoring.md)). It publishes to crates.io next to the other channels of
[ADR 0027](0027-distribution-and-release.md).

Each of those pressures argues for a crate boundary at a specific place, and a multi-crate workspace then needs
invariants (one version, inherited dependencies, layering, notices, frozen rule ids) that nobody keeps by hand. How is
the code split into crates, how are they published, and what holds the repository invariants?

## Decision Drivers

* Embedding without the CLI, and manifest and policy rules without oxc
* Vendored code isolated in one place
* Every library crate publishable to crates.io
* One semver promise, so internal refactors stay cheap
* Repository invariants checked by machines, in seconds

## Considered Options

* A layered Cargo workspace with one semver-stable facade, published leaves first, and scripted consistency checks
* One library crate plus a binary
* Unpublished internal crates re-exported from `skif`
* The binary in the `skif` crate

## Decision Outcome

Chosen option: "A layered Cargo workspace with one semver-stable facade, published leaves first, and scripted
consistency checks", because every boundary maps to one of the pressures, one stability promise on `skif` leaves the
other crates free to change, and shell checks over the manifests keep the invariants in CI.

### Workspace and crates

* The repository is a Cargo workspace under `crates/`, resolver 3, with a virtual root manifest that owns
  `[workspace.package]`, `[workspace.dependencies]`, `[workspace.lints]` and the profiles.
* The crates and what they own:
  * `skif-core`: plain data (`Diagnostic`, `Span`, `Fix`, `FixPlan`, `RuleMeta`, `Report`) and the `FileSystem` trait;
  * `skif-specifier`: the vendored syncpack specifier parser;
  * `skif-config`: the typed configuration, its loading, layering and schema;
  * `skif-edit`: fix operations to text edits, format-preserving for JSON and YAML;
  * `skif-workspace`: package-manager detection, root and workspace discovery, packages and dependency instances;
  * `skif-lint`: `Rule`, `RuleProvider`, `Registry`, `LintContext`, `Emitter` and the parallel run;
  * `skif-rules`: the manifest, workspace, policy and catalog rules;
  * `skif-js-syntax`: the oxc boundary, source file to skif-owned module facts;
  * `skif-js-plugins`: the plugin model and the plugin table;
  * `skif-js`: the module graph and the JS rules as a `RuleProvider`;
  * `skif-report`: the renderers;
  * `skif-migrate`: the one-shot configuration translator;
  * `skif`: the library facade;
  * `skif-cli`: the `skif` binary;
  * `skif-node`: the napi addon, `publish = false`, reaching users only through npm.
* `tools/bench` (`skif-bench`) is `publish = false`.
* There is no shared test-helper crate. Fixtures and helpers sit next to the tests that use them.

### Layering

* The crate graph is acyclic and layered.
* `skif-core` depends on no workspace crate and on no parser, so every crate can name its types and read files through
  `FileSystem` without pulling in a parser.
* Only `skif-js-syntax`, `skif-js-plugins` and `skif-js` depend on an oxc crate directly. Every other crate that needs
  JavaScript syntax (`skif-migrate`) goes through them, and `skif` without default features contains no oxc crate.
* Terminal concerns (clap, colour, prompts, file writes, exit codes) live only in `skif-cli`. `skif-node` calls into
  `skif-cli` to run the same command line in process.
* Rules return fix operations defined in `skif-core`; only the facade's `fix` feature brings in `skif-edit`.

### Dependencies

* Every dependency, external and internal, is declared once in `[workspace.dependencies]`, and every member uses it as
  `workspace = true`, so a feature set applies to every crate that uses the dependency.
* An internal dependency carries both `path` and an exact `=` version, so a local build uses the path and a published
  crate resolves exactly the sibling it was built with.
* The oxc crates use caret ranges, and every oxc crate except `oxc_resolver` names the same release; `oxc_resolver` has
  its own release line ([ADR 0013](0013-js-analysis-core-oxc-module-graph-and-resolution.md)).
* All crates share one version ([ADR 0027](0027-distribution-and-release.md)).

### Publishing and stability

* Every crate under `crates/` except `skif-node` is published to crates.io, leaves first in topological order. The
  publish script probes crates.io before each crate, skips a version that already exists and backs off on HTTP 429, so a
  re-run completes a partial release.
* `skif` is the only crate with a stability promise. The other crates are published so that `skif` and `skif-cli` can
  depend on them from crates.io; their documentation says they are not an API.
* The binary is installed with `cargo install skif-cli` (or `cargo binstall skif-cli`), and the executable is `skif`.
* `cargo-semver-checks` runs on every published crate against its latest crates.io release, because the facade
  re-exports the other crates and the tool does not follow re-exports across crates. The job is advisory while skif is
  0.x; its per-crate result goes to the job summary for the release pull request to quote in the changelog.
* Every published crate ships copies of the root `LICENSE` and `ATTRIBUTIONS.md` and lists them in `include`, so the
  notices of vendored and ported code travel with each package.
* Crates that hold types shared between workers or kept by hosts as long-lived values assert `Send + Sync` at compile
  time.

### Generated and checked artefacts

* `schema/skif.schema.json` is committed, generated by `skif schema` and checked by `scripts/check-schema.sh`, which
  runs as `mise run schema:check` and in the CI `build` job.
* CLI tests run the built binary with `assert_cmd` and snapshot its output with `insta`.

### Consistency checks

`mise run consistency` and the CI `consistency` job run the repository guards; each is a shell script under `scripts/`
with its own tests in `scripts/test-check-scripts.sh`:

* `check-versions.sh`: one version across crates, internal pins, npm packages, hook pins and the pre-commit launcher;
* `check-pins.sh`: the oxc requirements are caret ranges on one release, and every member dependency is inherited;
* `check-headers.sh`: the vendored files carry their attribution headers and `ATTRIBUTIONS.md` keeps its sections
  ([ADR 0033](0033-attribution-and-vendoring.md));
* `check-package-contents.sh`: each crate's `LICENSE` and `ATTRIBUTIONS.md` equal the root copies and are packaged;
* `check-rule-ids.sh`: rule ids are never dropped ([ADR
  0008](0008-rule-and-diagnostic-model-rule-ids-and-severities.md));
* `check-dependency-tree.sh`: the layering above;
* the documentation guards owned by other records: `check-doc-pins.sh`
  ([ADR 0027](0027-distribution-and-release.md)), `check-plugin-list.sh`
  ([ADR 0015](0015-plugin-model-and-plugin-set.md)) and `check-support-matrix.sh`
  ([ADR 0030](0030-supported-versions-and-the-end-to-end-matrix.md)).

The `deny` job bans duplicate versions (with documented skips), unknown registries and unknown git sources, and also
runs on a schedule. The public-API rule of the facade is checked by a test, not by a lint
([ADR 0003](0003-library-api-and-embedding.md)), and the policy behaviour is held by the conformance fixtures under
`fixtures/policy`, not by comparison with a syncpack checkout.

### Consequences

* Good, because a host that wants only manifest and policy rules compiles no JavaScript parser.
* Good, because vendored code is physically separate, which keeps licence review to one directory.
* Good, because one stable crate is a clear semver surface, and internal crates change freely behind it.
* Good, because the facade's features map onto crate boundaries, so a disabled feature removes whole crates.
* Good, because the invariants are scripts that run in seconds and fail CI with a file and a message.
* Bad, because many published crates make a slow first release and a permanent publishing matrix; the ordered,
  idempotent publish script is a requirement.
* Bad, because published internals invite direct use despite the documentation; the exact `=` pins keep a mismatched set
  from resolving.
* Bad, because the shell checks read TOML textually and need maintenance when the manifests change shape.

### Confirmation

* `scripts/check-dependency-tree.sh`, `scripts/check-pins.sh`, `scripts/check-versions.sh`,
  `scripts/check-package-contents.sh` and `scripts/check-headers.sh` pass in the CI `consistency` job, and
  `scripts/test-check-scripts.sh` proves each of them fails on a broken input.
* `scripts/test-release-scripts.sh` covers the publish order, the crates.io probe (200, 404, 429) and the semver-check
  summary with stub `cargo` and `curl`.
* The `build` job runs `cargo package --workspace --locked`, which fails for a path dependency without a `version`, and
  `scripts/check-schema.sh` against the freshly built binary.
* The `deny` job runs `cargo-deny` on every push and on a schedule.

## Pros and Cons of the Options

### A layered Cargo workspace with one semver-stable facade, published leaves first, and scripted consistency checks

* Good, because every boundary maps to a real pressure and the terminal dependencies stay out of the library.
* Good, because one stability promise leaves the internal crates free to refactor.
* Bad, because there are many manifests to keep consistent, which the scripts make mandatory rather than optional.

### One library crate plus a binary

* Good, because it is the least to build, publish and version.
* Bad, because oxc would be a dependency of every consumer, and features inside one crate do not stop modules from
  depending on each other in ways that later make a split impossible.

### Unpublished internal crates re-exported from `skif`

* Good, because only one crate would carry a public name.
* Bad, because crates.io does not accept a published crate whose path dependency is unpublished.

### The binary in the `skif` crate

* Good, because `cargo install skif` would install the binary.
* Bad, because a library consumer would pull clap, a terminal UI and a prompt library to get the rules.
