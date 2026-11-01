<!-- markdownlint-disable MD033 MD041 -->
<div align="center">

# skif

**A fast dependency linter for JavaScript and TypeScript monorepos.**

Workspace rules, version and catalog policies, and unused or unlisted dependencies, in one
single-binary CLI that is also an embeddable Rust library.

[![crates.io](https://img.shields.io/crates/v/skif?style=flat-square)](https://crates.io/crates/skif)
[![npm](https://img.shields.io/npm/v/skif?style=flat-square)](https://www.npmjs.com/package/skif)
[![CI](https://img.shields.io/github/actions/workflow/status/Goldziher/skif/ci.yml?style=flat-square)](https://github.com/Goldziher/skif/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-green?style=flat-square)](LICENSE)

</div>

---

> **Pre-release.** skif is under active development and has not been published yet. The
> workspace rules below work today; the version policies, catalog policies, and JavaScript analysis
> are planned. Nothing here is stable until the first release.

## What you get

| Capability | What it does | Status |
|---|---|---|
| **Workspace rules** | Checks the root and member `package.json` files of npm, yarn, and pnpm workspaces: empty or unsorted dependency fields, `@types/*` in dependencies, local path dependencies, unmatched workspace patterns, incompatible peer dependencies, and more. Many rules have autofixes. | Available |
| **Version and catalog policies** | Version groups, semver range policies, and pnpm, package.json, and bun catalog enforcement across packages. | Planned |
| **Unused and unlisted dependencies, dead code** | Finds dependencies that are declared but never imported, imports that are never declared, and unused files and exports, by analysing JavaScript and TypeScript sources. | Planned |

## Installation

skif has not been released yet. Every channel below is available from the first release; until then,
build from source with `cargo build --release`.

```bash
# Rust toolchain
cargo install skif-cli
cargo binstall skif-cli

# npm (JavaScript projects); the package is `skif`, with prebuilt per-platform binaries
pnpm add -D skif        # or: npm install -D skif / yarn add -D skif / bun add -D skif
pnpm dlx skif           # run without installing; also: npx skif / yarn dlx skif / bunx skif

# Homebrew (macOS and Linux)
brew install goldziher/tap/skif

# Scoop (Windows)
scoop bucket add goldziher https://github.com/Goldziher/scoop-bucket
scoop install skif
```

`cargo install skif` installs the library crate, not the binary; the command-line tool is `skif-cli`.

### GitHub Action

```yaml
- uses: actions/checkout@v7
- uses: Goldziher/skif@v1
  with:
    fail-on-warnings: true
```

Inputs: `version` (defaults to the action's own tag), `args` (extra skif arguments), `working-directory`,
`fail-on-warnings`, and `install-only` to put `skif` on `PATH` without running it. The action downloads
the release archive for the runner and verifies it against the release's `checksums.txt`.

### Prebuilt binaries

Each release attaches one archive per target, plus `checksums.txt`:

| Target | Archive |
| --- | --- |
| macOS, Apple silicon / Intel | `skif-aarch64-apple-darwin.tar.gz`, `skif-x86_64-apple-darwin.tar.gz` |
| Linux x64 / arm64, glibc | `skif-x86_64-unknown-linux-gnu.tar.gz`, `skif-aarch64-unknown-linux-gnu.tar.gz` |
| Linux x64 / arm64, musl (static, Alpine) | `skif-x86_64-unknown-linux-musl.tar.gz`, `skif-aarch64-unknown-linux-musl.tar.gz` |
| Windows x64 / arm64 | `skif-x86_64-pc-windows-msvc.zip`, `skif-aarch64-pc-windows-msvc.zip` |

Each archive holds the `skif` binary (`skif.exe` on Windows) and the licence. Verify a download with
`sha256sum --check --ignore-missing checksums.txt`.

## Git hooks

skif belongs in a pre-commit hook: a mismatched version or an unsorted dependency map is cheapest to fix
before it is committed. Both hooks lint the whole workspace (`pass_filenames: false`) and pass `--no-js`,
because JavaScript analysis reads every source file and belongs in CI. The fix hook also passes
`--no-install`, so a commit never runs a package manager, and it is refused when `CI` is set. Hooks are
available from the first release.

<details>
<summary><strong>pre-commit / prek</strong></summary>

Add to `.pre-commit-config.yaml`:

```yaml
repos:
  - repo: https://github.com/Goldziher/skif
    rev: v0.1.0 # use the latest release tag
    hooks:
      - id: skif       # skif --no-js
      - id: skif-fix   # skif --fix --no-install --no-js (opt-in, rewrites manifests)
```

Both hooks run `scripts/pre-commit/run-skif.sh`, which needs neither Rust nor Node. It uses, in order:
`SKIF_BINARY` (an error if it is not executable), `skif` on `PATH`, then the release pinned by the hook
revision, downloaded once into `~/.cache/skif/pre-commit` and verified against `checksums.txt`.

</details>

<details>
<summary><strong>Poly</strong></summary>

Add skif as a managed source in your `poly.toml`:

```toml
[[hooks.sources]]
id = "skif"
git = "https://github.com/Goldziher/skif.git"
revision = "v0.1.0" # use the latest release tag
hooks = ["skif"]
```

`poly-hooks.toml` also provides `skif-fix`. Each hook has a `system` channel (`skif` on `PATH`) and an
`npm` channel pinned to the release version (`pnpm dlx skif@<version>`). Then:

```bash
poly hooks update
git add poly.toml poly-hooks.lock
poly hooks install
```

</details>

<details>
<summary><strong>Lefthook</strong></summary>

Lefthook is configured on the consumer side, so nothing ships from this repository. Add to `lefthook.yml`:

```yaml
pre-commit:
  commands:
    skif:
      glob: "{package.json,**/package.json,pnpm-workspace.yaml,skif.toml,.skif.toml}"
      run: skif --no-js
```

</details>

## Configuration

skif reads its configuration from `skif.toml` or `.skif.toml` in the workspace root. The
file format is planned and arrives in a later release; today only CLI flags are supported.
The intended shape:

```toml
[rules]
unsorted-dependencies = "warn"
types-in-dependencies = "error"
empty-dependencies = "off"
```

## Status

skif is a clean rebuild of a monorepo linter, restructured as a library-first workspace. Design
decisions are recorded as architecture decision records in [`adrs/`](adrs/index.md).

## License

MIT, see [LICENSE](LICENSE). Code derived from other projects, including the sherif rules this
project started from, is credited in [ATTRIBUTIONS.md](ATTRIBUTIONS.md).
