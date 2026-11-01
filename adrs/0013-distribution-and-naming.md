---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# Distribution and Naming: skif, Eight Targets, One Version Everywhere

## Context and Problem Statement

The tool needs a name that is not sherif's: skif is a different tool with a different
configuration, rule set and scope ([ADR 0001](0001-mission-and-scope.md)), and shipping it as a sherif release would promise a
compatibility it does not have. The repository has already been renamed to `Goldziher/skif`.

The name `skif` was checked on 2026-11-01 and is unregistered on both npm and crates.io, so every
channel can use the bare name and no channel needs the decorated fallbacks voom and adrlint had to
accept.

skif's users are JavaScript developers first. Most of them have Node and a package manager, not a
Rust toolchain; CI runners are Linux on x64 and increasingly arm64, often in Alpine containers; a
meaningful minority work on Windows, including Windows on Arm. The sibling projects (poly, adrlint,
voom) have already worked out a release pipeline for this audience, including npm's handling of
install scripts (voom ADR 0010's amendment) and the per-platform package model poly uses.

## Decision Drivers

* The tool must not promise sherif compatibility it does not have
* Users are JavaScript developers first, on Linux (glibc and musl), macOS and Windows, x64 and arm64
* No network access at first run and no npm install scripts
* A user must never be pointed at a release whose binaries are missing, and a half-published set of registries must be recoverable

## Considered Options

* One name, skif, on every channel and eight release targets, fed by one release build with synchronized versions
* Keep the sherif name and publish as a fork
* npm downloader on first run (voom's model)
* cargo-dist or release-plz
* PyPI wrapper

## Decision Outcome

Chosen option: "One name, skif, on every channel and eight release targets, fed by one release build with synchronized versions", because the same name everywhere, per-platform npm packages with no install script, and a draft-then-publish pipeline suit JavaScript users and never expose a GitHub release with missing archives.

skif is published under **one name, `skif`**, through these channels, all fed by one release
build:

| Channel | Package | Install |
| --- | --- | --- |
| crates.io | every crate in [ADR 0002](0002-crate-layout-and-publishing.md); the binary in `skif-cli` | `cargo install skif-cli` |
| cargo-binstall | metadata on `skif-cli` pointing at the release archives | `cargo binstall skif-cli` |
| GitHub releases | `skif-<target>.tar.gz` / `.zip` plus `checksums.txt` | download |
| npm | umbrella `skif` with per-platform packages as `optionalDependencies` | `pnpm add -D skif` |
| GitHub Action | composite action at the repository root | `uses: Goldziher/skif@v1` |
| Homebrew | `Goldziher/homebrew-tap` → `Formula/skif.rb` | `brew install goldziher/tap/skif` |
| Scoop | `Goldziher/scoop-bucket` → `skif.json` | `scoop install skif` |
| Git hooks | poly catalog, pre-commit, Lefthook | [ADR 0014](0014-git-hook-integration.md) |

There is **no PyPI package**: skif lints JavaScript workspaces, and a Python wrapper would serve no
user who does not already have Node.

**Eight release targets:** `aarch64-apple-darwin`, `x86_64-apple-darwin`,
`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-musl`,
`aarch64-unknown-linux-musl`, `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`. musl builds are
static and serve Alpine-based CI images; the npm launcher picks musl or glibc at runtime.

**npm follows poly's model**: the umbrella `skif` package has a small Node launcher as its `bin` and
lists one package per platform (`skif-darwin-arm64`, `skif-linux-x64-gnu`, `skif-linux-x64-musl`, `skif-win32-x64`, …)
in `optionalDependencies`, each declaring `os`, `cpu` and, for Linux, `libc`. The package manager
installs only the matching one, no install script runs, and nothing is downloaded at first run. The
launcher fails with an explicit message naming the missing platform package if none was installed.

**Release mechanics** (`publish.yaml`, one hand-written workflow, as in the siblings): resolve
metadata → create a **draft** GitHub release → build the eight targets → upload archives and
checksums → publish the release only when all are present → publish crates in dependency order with
an existence probe and 429 backoff ([ADR 0002](0002-crate-layout-and-publishing.md)) → publish npm platform packages, then the umbrella →
verify `cargo binstall skif-cli` against the new release → move the floating `v1` tag for the
Action.

**What "never partial" covers.** The draft guarantees that the *GitHub release* is not visible until
all eight archives and `checksums.txt` exist. It does not make the whole release atomic: crates.io
and npm cannot be rolled back, so a run that fails after publishing some crates leaves a partially
published version on those registries. The pipeline limits the damage and makes it recoverable
rather than preventing it: every step probes for existing artifacts and skips them, so re-running the
workflow completes the release; crates go leaves first, so a crate never depends on an unpublished
sibling; npm publishes the platform packages before the umbrella, so the umbrella never lists a
missing package; and the floating `v1` tag and the Homebrew and Scoop manifests move last, after
verification, so the install channels people follow point at a version only once it is complete. crates.io and npm use OIDC trusted publishing bound to the workflow file name, so
`publish.yaml` is never renamed. Homebrew and Scoop manifests are regenerated by scripts from the
checksums file.

**Versions are synchronized across every surface**: all `Cargo.toml` crate versions (one workspace
version), the internal `=` pins, `npm-package/package.json` and every platform package, the
umbrella's `optionalDependencies`, and the pinned versions in `poly-hooks.toml` and the pre-commit
launcher ([ADR 0014](0014-git-hook-integration.md)). `scripts/release-bump.sh` rewrites them together, and a CI job fails when any
two disagree.

### Consequences

* Good, because the same name on every registry: `skif` in `package.json`, `skif` on `$PATH`, `skif` in docs.
* Good, because JavaScript users install skif like any dev dependency and pin it in their lockfile, with no
  network access at first run and no install scripts.
* Good, because the GitHub release is never visible with missing archives, and every publishing step is idempotent, so a
  run that fails partway is finished by re-running it.
* Bad, because the registries are not atomic: between the first and last publish step a version can exist on crates.io
  without the npm packages, and a bad crate version cannot be unpublished, only yanked.
* Bad, because eight targets and nine npm packages make a large release matrix; a single failing target blocks
  the release by design.
* Bad, because `cargo install skif` does not install the binary (the `skif` crate is the library, [ADR 0002](0002-crate-layout-and-publishing.md));
  the README and the `skif` crate's documentation point at `skif-cli`.
* Bad, because the version appears in many files. The bump script and the CI agreement check are the only things
  standing between that and a mismatched release.

### Confirmation

* A `publish.yaml` dry run (the workflow's `dry-run` input, also triggered by a pull request that touches the workflow) builds the eight targets, creates the draft, and skips every irreversible step; it must pass before the workflow changes merge.
* The workflow's draft-to-published step has a `needs:` on all eight build jobs and a check that the release asset list equals the expected eight archives plus `checksums.txt`; the same check runs in the dry run.
* Each publish step (crates, npm platform packages, umbrella, Homebrew, Scoop) probes for the artifact before publishing, and a test in `scripts/` runs the probe logic against recorded registry responses for "present", "absent" and HTTP 429.
* After publishing, the workflow runs `cargo binstall skif-cli@<version>` and `npx skif@<version> --version` on a clean runner, and fails the release if either does not print the released version.
* `scripts/release-bump.sh` rewrites every version together, and the `consistency` job in `.github/workflows/ci.yml` fails when any two of the crate versions, `=` pins, `npm-package/package.json`, platform packages, `poly-hooks.toml` and the pre-commit launcher disagree.

## Pros and Cons of the Options

### One name, skif, on every channel and eight release targets, fed by one release build with synchronized versions

* Good, because the same name appears on every registry, JavaScript users install skif like any dev dependency with no network access at first run and no install scripts, and the GitHub release stays hidden until it is complete.
* Bad, because eight targets and nine npm packages make a large release matrix in which one failing target blocks the release, the version appears in many files, and registry publishing is not atomic.

### Keep the sherif name and publish as a fork

* Good, because existing sherif users would find the tool under a name they know.
* Bad, because it implies compatibility ([ADR 0001](0001-mission-and-scope.md)), and `sherif` on npm and crates.io belongs to the upstream author.

### npm downloader on first run (voom's model)

* Good, because the npm package would be tiny and one package would serve every platform.
* Bad, because per-platform packages work offline once installed and need no install script or first-run network access, and poly already proves the publishing matrix. (rejected for skif)

### cargo-dist or release-plz

* Good, because both automate much of the release by convention.
* Bad, because of the reasons voom ADR 0010 records, and the sibling workflow already encodes the registry-specific fixes.

### PyPI wrapper

* Good, because Python users could `pip install` it.
* Bad, because skif lints JavaScript workspaces, so no user lacks Node but needs skif.

## More Information

* Related decisions: [ADR 0001](0001-mission-and-scope.md), [ADR 0002](0002-crate-layout-and-publishing.md), [ADR 0014](0014-git-hook-integration.md)
* See the [ADR index](index.md).
