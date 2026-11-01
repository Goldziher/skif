# 0014 — Git Hook Integration: pre-commit Launcher, poly Catalog, Lefthook Docs

* Status: Accepted
* Date: 2026-11-01

## Context

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
JavaScript developers (ADR 0013), so requiring Rust is not acceptable.

## Decision

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
surface rewritten by the bump script (ADR 0013).

### Lefthook

Lefthook needs nothing from the producer. The README documents a `lefthook.yml` snippet that runs
`skif` on `pre-commit` with a `glob` over the manifest files; there is no shipped config.

## Consequences

Positive:

* pre-commit users need neither Rust nor Node; the launcher brings the binary.
* An installed `skif` (from npm, Homebrew or Cargo) is used directly and versions stay under the
  user's control; the download is only a fallback.
* poly users get the same hooks through the catalog they already use, pinned per release.

Negative / risks:

* The launcher is a shell script, so the pre-commit hooks need a POSIX shell; on Windows that is Git
  Bash, which Git for Windows ships.
* The download fallback needs network on first use and fails in isolated environments; `SKIF_BINARY`
  is the escape hatch.
* The launcher's pinned version, `poly-hooks.toml` and the release must agree; the version check in
  CI covers all three.

## Alternatives considered

* **`language: rust`** (adrlint's choice): rejected — requires a Rust toolchain and a multi-minute
  compile for a JavaScript audience.
* **`language: node` installing the npm package:** rejected — pre-commit's Node support builds its own
  environment and ignores the project's package manager and lockfile; the launcher already prefers an
  installed `skif` on `PATH`.
* **Shipping a Lefthook config file:** rejected — Lefthook has no producer-side discovery, so a file in
  this repository would be copied, not consumed.
