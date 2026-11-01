---
status: accepted
date: 2026-11-01
decision-makers: ["Na'aman Hirschfeld"]
---

# Git Hook Integration: pre-commit Launcher, poly Catalog, Lefthook Docs

## Context and Problem Statement

A dependency linter belongs in a pre-commit hook: the developer has just edited a `package.json`,
and a mismatched version or an unsorted dependency map is cheapest to fix before it is committed.
Three hook managers matter for skif's users:

* **pre-commit** (and its Rust reimplementation prek) discovers hooks from `.pre-commit-hooks.yaml`
  in the producing repository.
* **poly** consumes producer catalogs (`poly-hooks.toml`) declared in `[[hooks.sources]]`.
* **Lefthook** is configured entirely on the consumer side and needs no producer manifest.

The siblings show two ways to give pre-commit a binary. adrlint uses `language: rust`, which makes
pre-commit `cargo install` the crate from the pinned revision — simple, but every consumer needs a
Rust toolchain and pays a full compile. ai-rulez uses `language: script` with a launcher,
`scripts/pre-commit/run-ai-rulez.sh`, that takes an explicit `AI_RULEZ_BINARY`, else an `ai-rulez`
on `PATH`, else downloads the pinned release into `~/.cache/ai-rulez/pre-commit`. skif's users are
JavaScript developers ([ADR 0013](0013-distribution-and-naming.md)), so requiring Rust is not acceptable.

## Decision Drivers

* A dependency linter belongs in a pre-commit hook
* skif's users are JavaScript developers, so requiring a Rust toolchain is not acceptable
* pre-commit, poly and Lefthook discover hooks in three different ways

## Considered Options

* A script-language pre-commit launcher, a poly hook catalog and documented Lefthook configuration
* `language: rust` (adrlint's choice)
* `language: node` installing the npm package
* Shipping a Lefthook config file

## Decision Outcome

Chosen option: "A script-language pre-commit launcher, a poly hook catalog and documented Lefthook configuration", because the launcher brings the binary without Rust or Node, prefers an installed `skif`, and the poly catalog pins the same hooks per release.

### pre-commit / prek

`.pre-commit-hooks.yaml` declares hooks with **`language: script`** and
`entry: scripts/pre-commit/run-skif.sh`, modelled on ai-rulez's launcher. The launcher resolves the
binary in this order:

1. `SKIF_BINARY`, if set — and fails if it is not executable, rather than falling back to a
   different binary;
2. `skif` on `PATH` (Homebrew, Scoop, `cargo install`, a global npm install);
3. the pinned release for the current platform, downloaded once into `~/.cache/skif/pre-commit/<version>`
   (`%LOCALAPPDATA%` on Windows), verified against the release's `checksums.txt`, and unpacked into a
   temporary directory then renamed so concurrent hooks cannot corrupt it.

Hooks:

| id | Runs | Triggers |
| --- | --- | --- |
| `skif` | `skif` (lint) | `package.json`, `pnpm-workspace.yaml`, `skif.toml`, `.skif.toml` |
| `skif-fix` | `skif --fix` | same files; opt-in, rewrites manifests |

Both use `pass_filenames: false`: skif lints a workspace, not a file list, and a change to one
manifest can create a diagnostic in another. JS analysis is not part of the default hook; it is a
CI job, because it reads every source file.

### poly

`poly-hooks.toml` (`version = 1`) exposes the same hooks with `workspace = true` and two channels per
hook, following adrlint's and voom's catalogs:

| Channel | `check` | `run` |
| --- | --- | --- |
| `system` | `command -v skif` | `skif` |
| `npm` | `command -v pnpm` | `pnpm dlx skif@X.Y.Z` |

The npm channel is **pinned to the exact release version**, so `poly-hooks.toml` is a release
surface rewritten by the bump script ([ADR 0013](0013-distribution-and-naming.md)).

### Lefthook

Lefthook needs nothing from the producer. The README documents a `lefthook.yml` snippet that runs
`skif` on `pre-commit` with a `glob` over the manifest files; there is no shipped config.

### Consequences

* Good, because pre-commit users need neither Rust nor Node; the launcher brings the binary.
* Good, because an installed `skif` (from npm, Homebrew or Cargo) is used directly and versions stay under the
  user's control; the download is only a fallback.
* Good, because poly users get the same hooks through the catalog they already use, pinned per release.
* Bad, because the launcher is a shell script, so the pre-commit hooks need a POSIX shell; on Windows that is Git
  Bash, which Git for Windows ships.
* Bad, because the download fallback needs network on first use and fails in isolated environments; `SKIF_BINARY`
  is the escape hatch.
* Bad, because the launcher's pinned version, `poly-hooks.toml` and the release must agree; the version check in
  CI covers all three.

### Confirmation

The version check in CI covers the launcher's pinned version, `poly-hooks.toml` and the release.

## Pros and Cons of the Options

### A script-language pre-commit launcher, a poly hook catalog and documented Lefthook configuration

* Good, because it satisfies the decision drivers above; the benefits are listed under Consequences.

### `language: rust` (adrlint's choice)

* Bad, because it requires a Rust toolchain and a multi-minute compile for a JavaScript audience.

### `language: node` installing the npm package

* Bad, because pre-commit's Node support builds its own environment and ignores the project's package manager and lockfile; the launcher already prefers an installed `skif` on `PATH`.

### Shipping a Lefthook config file

* Bad, because Lefthook has no producer-side discovery, so a file in this repository would be copied, not consumed.

## More Information

* Related decisions: [ADR 0013](0013-distribution-and-naming.md)
* See the [ADR index](index.md).
