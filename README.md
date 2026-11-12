<!-- markdownlint-disable MD033 MD041 -->
<div align="center">

<h1 align="center"><img src="docs/media/skif-banner.svg" alt="skif: dependency linter for JavaScript and TypeScript monorepos" width="820"></h1>

**One fast binary for the dependency checks your monorepo spreads across three tools.**

skif lints manifests, enforces version and catalog policies, and finds unused dependencies and dead code,
with one config file and one report. It never runs your project's code.

npm · pnpm · yarn · bun · **53 rules** · **206 plugins** · format-preserving fixes

[![crates.io](https://img.shields.io/crates/v/skif?style=flat-square)](https://crates.io/crates/skif)
[![npm](https://img.shields.io/npm/v/skif-cli?style=flat-square)](https://www.npmjs.com/package/skif-cli)
[![CI](https://img.shields.io/github/actions/workflow/status/Goldziher/skif/ci.yml?style=flat-square)](https://github.com/Goldziher/skif/actions/workflows/ci.yml)
[![Docs](https://img.shields.io/badge/docs-goldziher.github.io%2Fskif-f07a1a?style=flat-square)](https://goldziher.github.io/skif/)
[![License: MIT](https://img.shields.io/badge/license-MIT-green?style=flat-square)](LICENSE)

[Docs](https://goldziher.github.io/skif/) · [Quick start](#quick-start) · [Compare](#how-it-compares) · [Benchmarks](https://goldziher.github.io/skif/guides/benchmarks/) · [Install](#installation) · [Rules](docs/rules.md)

</div>

---

## What you get

| | |
| --- | --- |
| **One tool** | Manifest hygiene, version policy and dead-code analysis in one run, one `skif.toml` and one report. |
| **Fast** | Timed against knip, sherif and syncpack on pinned public repositories and generated workspaces. [See the numbers.](#performance) |
| **Safe** | Analysis is static: skif reads your code and configs without running them. Fixes keep key order, indentation and comments. |
| **Easy to adopt** | `skif migrate` translates your sherif, syncpack and knip configuration in one step, and `skif baseline` records the findings you cannot fix yet so CI only accepts fewer. |
| **Built for CI** | Human, JSON, SARIF, GitHub annotation, GitLab Code Quality, compact, Markdown and CODEOWNERS output, stable rule ids and a GitHub Action. |
| **Embeddable** | The [`skif`](crates/skif/README.md) crate is a library that does no I/O of its own. |

## Quick start

```bash
npx skif-cli              # lint the workspace or package you are in
npx skif-cli --fix        # apply safe fixes
npx skif-cli migrate      # turn sherif, syncpack and knip config into a skif.toml
npx skif-cli update --check  # list newer published versions; the only command that uses the network
```

With skif installed, the command is `skif`. See the [CLI reference](https://goldziher.github.io/skif/reference/cli/).

## How it compares

skif builds on [sherif](https://github.com/QuiiBz/sherif), [syncpack](https://github.com/JamieMason/syncpack) and
[knip](https://github.com/webpro-nl/knip), which remain excellent, focused tools. It started as a fork of sherif and
follows knip's issue types. The difference is that one tool covers all three jobs.

The tables compare skif with sherif 1.13.0, syncpack 15.3.3 and knip 6.45.1, checked against their source code at
those tags. The [comparison guide](https://goldziher.github.io/skif/guides/comparison/#capabilities) has the full
matrix, explains each "Partial" and lists the sources.

| Capability | skif | sherif | syncpack | knip |
| --- | --- | --- | --- | --- |
| Manifest and workspace rules | Yes | Yes | Partial: sorts dependency maps | No |
| Version mismatches | Yes | Yes | Yes | No |
| Version and semver-range groups, banned dependencies | Yes | No | Yes | No |
| Related dependencies on one version (`react`, `react-dom`) | Yes | Partial: a fixed list | Yes | No |
| Catalog policy | Yes: pnpm, Bun, Yarn 4.10+ | No | Partial: pnpm, Bun | No |
| Unused catalog entries, unresolved `catalog:` references | Yes | No | Partial: unresolved references | Yes |
| `workspace:` protocol | Yes | No | Partial: a pinned version group | No |
| Toolchain pins (`packageManager`, `engines`, `.nvmrc`, Dockerfiles, workflows) | Yes | Partial: `packageManager` present | Partial: `package.json` fields | No |
| Lockfile checks (stray lockfiles, bans over the lockfile, stale patches) | Partial: `pnpm-lock.yaml`, `bun.lock` | No | No | No |
| Registry updates | Yes: `skif update`, `--check` | No | Yes: `syncpack update`, `--check` | No |
| Unused and unlisted dependencies, unlisted binaries, unresolved imports | Yes | No | No | Yes |
| Unused files, exports, types, enum and namespace members; duplicate exports | Yes | No | No | Yes |
| Circular imports | Yes, off by default | No | No | Yes, off by default |
| Unused `package.json` subpath imports and exports | No | No | No | Yes |
| Vue, Svelte, Astro, MDX, Marko | Yes | No | No | Yes |
| Custom compilers for other file types | No | No | No | Yes |
| Built-in plugins | 206 | No | No | 207 |
| Runs no project code while linting | Yes | Yes | Partial: imports its own JS config | No |
| Autofix | Yes | Yes | Yes | Yes |
| Safe and unsafe fix levels | Yes | No | No | Partial: file deletion opt-in |
| Baseline that can only shrink | Yes | No | No | No |
| Translates the other tools' configuration | Yes: `skif migrate` | No | No | No |
| Configuration | `skif.toml`, layered | `package.json` key | `.syncpackrc` (JSON, YAML, JS, TS) | `knip.json`, `knip.ts` |
| JSON, SARIF, GitHub, Code Climate output | Yes | No | Partial: JSON from some commands | Yes |
| Watch mode, language server | Yes | No | No | Yes |
| Editor extension | No | No | No | Yes |
| MCP server | Yes | No | No | Yes: runs knip, serves its docs |
| GitHub Action | Yes | Yes | No | No |
| Git hooks (pre-commit, prek, Poly) | Yes | No | No | No |
| Library API | Yes: Rust crate, npm package | No | No | Partial: undocumented |

| Support | skif | sherif | syncpack | knip |
| --- | --- | --- | --- | --- |
| Package managers | npm, pnpm, Yarn 2+, Yarn Classic, Bun | npm, pnpm, Yarn, Bun | npm, pnpm, Yarn, Bun, Lerna | npm, pnpm, Yarn, Bun |
| Linkers tested end to end in CI | npm (hoisted); pnpm (isolated, `nodeLinker: hoisted`); Yarn 2+ (Plug'n'Play, `nodeLinker: node-modules`, `nodeLinker: pnpm`); Yarn Classic (hoisted); Bun (default, `linker = "hoisted"`, `linker = "isolated"`) | None | None | No linker matrix |
| Runtime | Native binary, or through npm on Node.js 22, 24, 26 and Bun 1.x | Native binary | Native binary | Node.js 20.19+ or 22.12+, or Bun |
| Platforms | macOS, Linux (glibc, musl), Windows; x64, arm64 (Windows not yet on npm) | macOS, Linux (glibc, musl), Windows; x64, arm64 | macOS, Linux (glibc, musl), Windows; x64, arm64 | Where `oxc-parser` has a native binding |
| Distribution | npm, crates.io, cargo-binstall, release archives, Homebrew, Scoop | npm, release archives, Homebrew | npm; crates.io up to 14.0.0 | npm |

Versions per package manager and operating system are under
[supported versions](https://goldziher.github.io/skif/start/installation/#supported-versions). skif is younger than
all three. The JS analysis is close to knip but not identical, and
[the differences are published](https://goldziher.github.io/skif/guides/benchmarks/#agreement-with-knip).

## Performance

Generated from the latest imported run of the `bench` workflow, with the dependencies of the repositories installed:

<!-- bench:claims:start -->
- **6.0–9.0x faster than knip** (median 7.5x) for dead-code analysis, on 3 pinned public repositories where knip completes; it fails on create-t3-turbo and vue-core.
- **1.1–7.5x faster than sherif** (median 4.2x) for manifest and workspace checks (`skif --no-js`), on 5 pinned public repositories and 3 generated workspaces of 50, 500 and 2000 packages.
- **5.9–20.6x faster than syncpack** (median 8.9x) for version-policy checks (`skif --no-js`), on 5 pinned public repositories and 3 generated workspaces of 50, 500 and 2000 packages.

skif 0.3.0 against knip 6.44.1, sherif 1.13.0, syncpack 15.3.3: medians of 15 runs on a GitHub-hosted runner, measured on 2026-11-12.
<!-- bench:claims:end -->

The [benchmarks page](https://goldziher.github.io/skif/guides/benchmarks/) has every table, the workflow run the
numbers come from, the methodology, and skif's accuracy on seeded defects and its agreement with knip.

## Installation

<details open>
<summary><strong>npm, pnpm, yarn or bun</strong></summary>

The package is `skif-cli`; the command it installs is `skif`.

```bash
pnpm add -D skif-cli     # or npm install -D, yarn add -D, bun add -D
pnpm dlx skif-cli        # or npx skif-cli, yarn dlx skif-cli, bunx -p skif-cli skif
```

Needs Node.js 22, 24 or 26, or Bun 1.x, on macOS or Linux (x64 or arm64). Tested end to end with the latest two
majors of npm, pnpm and Yarn 2+, the newest Yarn Classic 1.x and the newest Bun 1.x. Windows is not on npm in 0.3.0
([ADR 0114](adrs/0114-windows-npm-packages-deferred.md)): on Windows, use the release archives, cargo-binstall or
Scoop.

</details>

<details>
<summary><strong>Homebrew</strong></summary>

```bash
brew install goldziher/tap/skif
```

</details>

<details>
<summary><strong>Scoop</strong></summary>

```powershell
scoop bucket add goldziher https://github.com/Goldziher/scoop-bucket
scoop install skif
```

</details>

<details>
<summary><strong>Cargo</strong></summary>

```bash
cargo install skif-cli    # or cargo binstall skif-cli
```

The `skif` crate is the library; `skif-cli` is the command-line tool.

</details>

<details>
<summary><strong>GitHub Action</strong></summary>

```yaml
- uses: actions/checkout@v7
- uses: Goldziher/skif@v0.3
  with:
    fail-on-warnings: true
```

See the [Action guide](https://goldziher.github.io/skif/guides/integrations/) for inputs and version pinning.

</details>

<details>
<summary><strong>pre-commit or prek</strong></summary>

```yaml
repos:
  - repo: https://github.com/Goldziher/skif
    rev: v0.3.0 # use the latest release tag
    hooks:
      - id: skif       # lints manifests, skipping JS analysis
      - id: skif-fix   # optional: also applies safe fixes
```

Needs neither Rust nor Node. See the [hooks guide](https://goldziher.github.io/skif/guides/integrations/) for Poly and
Lefthook.

</details>

<details>
<summary><strong>Agent skill for coding agents</strong></summary>

The [`skif` skill](skills/skif/SKILL.md) teaches an agent to lint, triage and fix with skif. It needs skif itself
(any channel above, or `npx skif-cli`). Installed as a plugin, it also registers skif's MCP server, run with
`npx -y skif-cli@<release> mcp`, so the plugin needs Node.js but no separate install.

```bash
npx skills add Goldziher/skif                                 # any agent that reads Agent Skills
codex plugin marketplace add Goldziher/skif && codex plugin add skif@skif
droid plugin marketplace add https://github.com/Goldziher/skif && droid plugin install skif@skif
copilot plugin marketplace add Goldziher/skif && copilot plugin install skif@skif
```

In Claude Code, run `/plugin marketplace add Goldziher/skif`, then `/plugin install skif@skif` (or, from 2.1.275 on,
`/plugin install skif --marketplace Goldziher/skif`). See the
[agents guide](https://goldziher.github.io/skif/guides/editors-and-agents/#agent-skill).

</details>

Prebuilt archives and supported versions are listed in the
[installation guide](https://goldziher.github.io/skif/start/installation/).

## Configuration

skif works with no configuration. To adjust it, add a `skif.toml`:

```toml
[rules]
unsorted-dependencies = "warn"

[[ignore]]
rules = ["version-mismatch"]
dependencies = ["wrangler"]
reason = "workers still on the v2 runtime"
```

`skif init` writes a starter file. Files layer from your project up to the git root, so an organisation file applies to
every project below it. See the [configuration reference](https://goldziher.github.io/skif/reference/configuration/).

## Library

```bash
cargo add skif
```

The `skif` crate never prints, exits or writes files, so a host can embed the rules. Poly is the first host. See the
[library guide](https://goldziher.github.io/skif/guides/library/).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md). Design decisions are recorded as
[architecture decision records](adrs/index.md).

## License

MIT, see [LICENSE](LICENSE). skif started as a fork of sherif, vendors parts of syncpack and follows knip's issue types
and plugin knowledge; derived code is credited in [ATTRIBUTIONS.md](ATTRIBUTIONS.md).
