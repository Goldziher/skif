---
status: accepted
date: 2026-11-17
decision-makers: ["Na'aman Hirschfeld"]
---

# Distribution, Release and Supported Versions

## Context and Problem Statement

skif's users are JavaScript developers, Rust developers who embed the library, and CI pipelines, and each group
installs tools differently: as a dev dependency from npm, with `cargo install` or `cargo binstall`, with Homebrew or
Scoop, as a GitHub Action, or from a release archive. A release that reaches some channels and not others, or that names
a different version on each, breaks installs and makes a bug report impossible to place. skif also runs on Node.js and
Bun, is built with Rust and analyses workspaces of several package managers and linkers; without a rule, every old
toolchain or untested layout becomes an implicit promise.

Which channels does skif ship through, how are versions kept and released, and which versions does it support?

## Decision Drivers

* The same command, `skif`, and one version on every channel
* JavaScript users install skif like any dev dependency: offline at first run, no install scripts, Windows included
* The GitHub release is never visible with missing archives, and a failed release is finished by re-running it
* No personal credential in the release path; registries are published to by trusted publishing
* A supported version or layout is one CI installs and lints

## Considered Options

* One name on every channel, a napi-rs addon on npm under `@polylint`, synchronized versions, one idempotent release
  workflow, and support tied to upstream end of life with a tested matrix
* A Node launcher that spawns a binary from per-platform npm packages, because the platform packages and launcher are
  hand-maintained and library callers pay a process start
* A download-on-install npm package, because it runs an install script and needs the network at install
* Separate release workflows per channel, because channels drift apart in version and timing and trusted publishing
  bindings multiply

## Decision Outcome

Chosen option: "One name on every channel, a napi-rs addon on npm under `@polylint`, synchronized versions, one
idempotent release workflow, and support tied to upstream end of life with a tested matrix", because it keeps installs
offline and script-free, generates the platform packages instead of maintaining them, gives every channel the same
version from one build, and ties the support statement to facts someone else maintains.

### Channels

* The repository is `polylint/skif`. The command, the crates and the configuration are named `skif`. There is no PyPI
  package.
* crates.io: every published crate ([ADR 0002](0002-crates-library-api-and-upstream-code.md)), with the binary in
  `skif-cli` (`cargo install skif-cli`).
* cargo-binstall: metadata on `skif-cli` that points at the release archives. The Linux gnu triples are redirected to
  the static musl archives, because binstall cannot see the host's glibc; the glibc archives stay on the release page.
* GitHub release archives for macOS, Linux (gnu and musl) and Windows on x64 and arm64, each with `LICENSE` and the
  attributions, plus `checksums.txt` and build attestations.
* npm: `@polylint/skif`, described below.
* The composite GitHub Action at the repository root, `polylint/skif@v0.M`.
* Homebrew (`polylint/tap`) and Scoop (the `polylint` bucket), shared by every polylint tool. The formula and manifest
  are templates in `scripts/publish/`, rendered and pushed by the xberg-io publishing actions with a token of the
  organization GitHub App scoped to the one repository it writes. Bottles are built for the platforms that have a
  runner and merged only when all are built; below the glibc floor the formula installs the musl archive.
* Git hooks ([ADR 0013](0013-editors-agents-and-git-hooks.md)).
* `SKIF_GLIBC_FLOOR` in `scripts/lib/libc.sh` is the one definition of the glibc the gnu builds need; the formula and
  the build matrix are checked against it.

### The npm package

* The umbrella `@polylint/skif` is a napi-rs package: a Node-API addon built from the Rust library and loaded
  in-process, exposing the `skif` bin and a programmatic API. It is ESM only, has no install scripts and works offline.
  A scope makes the package name the product name, so `npx @polylint/skif` and `bunx @polylint/skif` run this tool.
* napi-rs generates the platform packages `@polylint/skif-<platform>` for macOS, Linux (gnu and musl) and Windows on
  x64 and arm64, with their `os`, `cpu` and `libc` fields, and writes them into the umbrella's `optionalDependencies`,
  so the list cannot drift from `napi.targets`. The Windows addons link the MSVC runtime statically, and the arm64 one
  is load-tested on an arm64 runner.
* Every package is published from `publish.yaml` by npm trusted publishing with provenance.
  `scripts/check-npm-packages-exist.sh` stops a release unless every package exists on the registry, since a trusted
  publisher can only be configured on an existing package.

### The Action's cache

* The Action caches skif's store ([ADR 0007](0007-js-analysis-core-on-oxc.md)) with the inputs `cache` (default
  `true`), `cache-key` (a suffix for matrix jobs), `cache-max-size` (default `256MiB`) and `cache-save` (default `true`
  on the default branch, `false` elsewhere), and the output `cache-hit`.
* It sets `SKIF_CACHE_DIR` to `${{ runner.temp }}/skif-cache` and restores it with `actions/cache/restore` under the
  key `skif-<os>-<version>-<job><cache-key>-<hash of the lockfiles, skif.toml and tsconfig*.json>-<sha>`, with restore
  keys that drop the sha, then the hash. After skif runs, `actions/cache/save` saves it under
  `always() && cache-save`.
* Pull requests restore only, so a pull request never writes a cache the default branch reads: the store's envelope
  checks integrity, not authenticity. The documentation gives the manual `actions/cache` snippet and GitLab's
  `cache:paths` for other setups.

### Build profiles

* The CLI binaries use `[profile.release]`, which unwinds, so a panic that reaches the entry point exits `2` with the
  `error` document instead of aborting.
* The addon uses `[profile.node]`, which inherits it and unwinds too, so a panic becomes a JavaScript exception instead
  of ending the host process.

### Versions and tags

* One version on every surface: the crates, the internal `=` pins, the npm umbrella and platform packages,
  `poly-hooks.toml`, the pre-commit launcher and the agent plugin manifests. `scripts/release-bump.sh` rewrites them
  together, and `scripts/check-versions.sh` fails when any two disagree.
* Floating tags: from 1.0 a release moves `vN`. A 0.x minor may break, so a 0.x release moves `v0.M` and `v0` never
  moves. Documentation pins the Action to the latest `v0.M`, checked by `scripts/check-doc-pins.sh`.
* A release ships when it is complete ([ADR 0001](0001-mission-scope-and-non-goals.md)). A name-reservation upload is
  not a release, and documentation never promises work for a later release.

### The release workflow

`.github/workflows/publish.yaml` is one workflow and is never renamed, because crates.io and npm trusted publishing are
bound to its file name. A tag is released only when it is on `main`, `CHANGELOG.md` has its section and its commit has a
green CI run (`scripts/check-release-gate.sh`). It creates a draft release; builds and smoke-tests every archive and
addon; uploads the archives and checksums and attests their build provenance; publishes the crates leaves-first and the npm
platform packages before the umbrella, so nothing ever depends on a missing package; verifies the published npm package
on every supported Node line and the newest Bun; promotes the draft once every expected asset is present and verifies
`cargo binstall` against it; and only then moves the floating tag and writes the tap and the bucket, which a prerelease
leaves alone.

* Every step probes for what already exists and skips it, so a re-run completes a failed release. A dry-run input
  builds everything and skips every irreversible step.
* The Action and the pre-commit launcher verify a release's attestation against the repository it was downloaded from:
  `polylint/skif`, unless the Action's `repository` input or `SKIF_REPOSITORY` names a mirror. A `polylint/skif`
  release up to v0.6.0 is also accepted when `Goldziher/skif`, the repository before the move, attested it
  (`scripts/lib/attestation.sh`).

### Supported versions

* skif supports a version of a runtime, language or tool only while it is not past its upstream end of life. Where
  upstream publishes no policy, skif supports the latest two majors. A version is dropped in the first release after it
  leaves support, with a `Changed` entry in `CHANGELOG.md`.
* Support means CI tests it. An older version may work but is not tested or blocked: `engines` fields are advisory and
  skif never refuses to run, and a bug that reproduces only on an unsupported version is not fixed.
* Node.js support is its non-EoL even-numbered lines; Bun its 1.x line; Rust the workspace `rust-version` and stable;
  npm, pnpm and Yarn 2+ their latest two majors; Yarn Classic its newest 1.x until upstream ends it. The versions
  themselves live in the installation guide. TypeScript syntax support is what the pinned oxc parser understands.

### The end-to-end matrix

* One job per package manager and version slot, resolved from the registry by `scripts/e2e/resolve-pm-version.sh`. The
  newest slot of every manager runs on Linux, macOS and Windows; the previous majors run on Linux.
* Each job runs every scenario under every linker its manager ships, and each install is checked for the layout its
  linker produces, so a scenario cannot pass on the wrong one.
* A scenario under `fixtures/e2e/<name>` holds the files every manager shares, overlays per manager and an expectation
  with overrides per manager and linker. An override states its reason; one that records a skif bug names the issue.
* `scripts/check-support-matrix.sh` builds the expected support tables from the CI matrix, the linker list and the npm
  runtimes, and fails when the README or the website guides differ.

### Consequences

* Good, because the command is `skif` everywhere, the npm name is the product name, and JavaScript users on every
  platform pin it in their lockfile with no network access at first run.
* Good, because Node and Bun callers can use skif in-process without spawning a binary.
* Good, because the GitHub release is never visible with missing archives, and the channels people follow move only
  after verification.
* Good, because no personal token takes part in a release, and skif's formula and manifest follow the same templates as
  the other polylint tools.
* Good, because the support matrix follows upstream, shrinks on a schedule and cannot drift from the documentation.
* Good, because Action runs start from a warm cache, and a pull request cannot write the cache the default branch
  reads.
* Bad, because a bad crate version cannot be unpublished, only yanked, and a single failing target blocks the release
  by design.
* Bad, because the release depends on the xberg-io actions and on the organization App's installation and permissions.
* Bad, because the version appears in many files, and the matrix is a large set of CI jobs to keep green.

### Confirmation

* `scripts/check-versions.sh` and `scripts/test-release-scripts.sh` require every version surface to follow a bump;
  `scripts/test-release-scripts.sh` also renders the formula and manifest templates and checks their hashes, URLs and
  glibc guard.
* `scripts/check-npm-tarballs.sh` requires exactly the umbrella and the platform packages `napi.targets` generates;
  `scripts/test-npm-published.sh` covers `scripts/check-npm-packages-exist.sh`.
* `scripts/release-assets.sh`, `scripts/check-archive.sh`, `scripts/release-floating-tag.sh`,
  `scripts/check-doc-pins.sh`, `scripts/check-glibc-floor.sh`, `scripts/check-libc-selection.sh` and
  `scripts/check-release-gate.sh` guard the assets, tags, pins, libc selection and release gate.
* `scripts/test-install-scripts.sh` checks the attestation identities, and `scripts/npm-smoke.sh` requires a panic in
  the addon to raise a JavaScript exception after which the addon still runs.
* The `load_addon_windows`, `verify_npm` and `verify_binstall` jobs of `.github/workflows/publish.yaml` run the built
  and published packages on clean runners.
* The `e2e` job of `.github/workflows/ci.yml`, `scripts/test-e2e-scripts.sh` and `scripts/check-support-matrix.sh`
  cover the matrix and its documentation; `npm-package/package.json` declares the advisory `engines`.
