<!-- markdownlint-disable MD033 MD041 -->
<div align="center">

<h1 align="center"><img src="docs/media/skif-banner.svg" alt="skif: dependency linter for JavaScript and TypeScript monorepos" width="820"></h1>

**One fast binary for the dependency checks your monorepo spreads across three tools.**

skif lints manifests, enforces version and catalog policies, and finds unused dependencies and dead code,
with one config file and one report. It never runs your project's code.

npm · pnpm · yarn · bun · **49 rules** · **206 plugins** · format-preserving fixes

[![crates.io](https://img.shields.io/crates/v/skif?style=flat-square)](https://crates.io/crates/skif)
[![npm](https://img.shields.io/npm/v/skif-cli?style=flat-square)](https://www.npmjs.com/package/skif-cli)
[![CI](https://img.shields.io/github/actions/workflow/status/Goldziher/skif/ci.yml?style=flat-square)](https://github.com/Goldziher/skif/actions/workflows/ci.yml)
[![Docs](https://img.shields.io/badge/docs-goldziher.github.io%2Fskif-f07a1a?style=flat-square)](https://goldziher.github.io/skif/)
[![License: MIT](https://img.shields.io/badge/license-MIT-green?style=flat-square)](LICENSE)

[Docs](https://goldziher.github.io/skif/) · [Quick start](#quick-start) · [Compare](#how-it-compares) · [Benchmarks](#benchmarks) · [Install](#installation) · [Rules](docs/rules.md)

</div>

---

## What you get

| | |
| --- | --- |
| **One tool** | Manifest hygiene, version policy and dead-code analysis in one run, one `skif.toml` and one report. |
| **Fast** | Timed against knip, sherif and syncpack on pinned public repositories and generated workspaces. [See the numbers.](#benchmarks) |
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
all three. The JS analysis is close to knip but not identical, and [the differences are published](#agreement-with-knip).

## Benchmarks

Every number in this section is generated: the speed tables from a run of the `bench` workflow on a pinned
GitHub-hosted runner, imported with `mise run bench:import`, and the agreement table by `mise run parity:publish` from
the committed parity baseline. Each block says which skif commit, tool versions, runner image and machine it comes
from, and links the workflow run. Absolute times depend on the machine; read the ratios.

### Speed

Full skif runs against knip, and `skif --no-js` against sherif and syncpack, on the
[pinned public repositories](tools/bench/corpus.toml) with dependencies installed and on generated workspaces.

<!-- bench:summary:start -->
Medians of 15 runs after 3 warmup runs (hyperfine 1.20.0), skif 0.3.0 at `150634614dea` built with rustc 1.99.1 (58cf8f9c3 2026-10-13) against knip 6.44.1, sherif 1.13.0, syncpack 15.3.3, measured on 2026-11-12 on a GitHub-hosted runner, image ubuntu24 20261101.360.1, Ubuntu 24.04.5 LTS, Linux 7.0.0-1017-azure, x86_64, AMD EPYC 9V45 96-Core Processor, 4 logical CPUs (2 physical cores), 15.6 GiB memory, hypervisor Microsoft, load average 3.76 / 2.39 / 1.02 before and 1.72 / 1.95 / 1.12 after the run ([workflow run, attempt 1](https://github.com/Goldziher/skif/actions/runs/43136971156/attempts/1)).

**Dead-code analysis against knip** (full skif run)

| Repository | skif | knip | knip / skif |
| --- | ---: | ---: | ---: |
| create-t3-turbo | 23 ms | fails: its Expo Metro config needs metro-cache, which pnpm's strict install does not provide | - |
| vue-core | 43 ms | fails: its SFC playground Vite config reads a build-time global that only the repository's own build defines | - |
| tanstack-query | 280 ms | 2.5 s | 9x |
| babel | 603 ms | 3.7 s | 6x |
| npm-cli | 64 ms | 481 ms | 8x |

**Manifest and policy checks** (`skif --no-js`)

| Target | skif --no-js | sherif | syncpack |
| --- | ---: | ---: | ---: |
| create-t3-turbo | 3.0 ms | 22 ms | 26 ms |
| vue-core | 3.1 ms | 23 ms | 28 ms |
| tanstack-query | 8.9 ms | 35 ms | 86 ms |
| babel | 12 ms | 25 ms | 251 ms |
| npm-cli | 4.6 ms | 22 ms | 56 ms |
| Generated, 50 packages | 3.6 ms | 22 ms | 30 ms |
| Generated, 500 packages | 18 ms | 38 ms | 109 ms |
| Generated, 2000 packages | 81 ms | 94 ms | 495 ms |
<!-- bench:summary:end -->

### Accuracy on seeded defects

A generated 500-package workspace with defects injected on purpose. `Actual` is counted from the files, so the right
answer is known. Counted by hand on 2026-11-06 (skif 0.1.0), not re-counted for this release; the method is in
[`tools/bench`](tools/bench/README.md#seeded-defects):

| Defect | Actual | skif | sherif | syncpack |
| --- | ---: | ---: | ---: | ---: |
| Dependencies with mismatched versions | 200 | 200 | 200 | 200 |
| Unsorted dependency maps | 25 | 25 | 25 | n/a |
| Type packages in `dependencies` | 24 | 24 | 24 | n/a |
| Empty dependency objects | 31 | 31 | 31 | n/a |

skif and sherif name the same 200 dependencies. skif and syncpack report the same 4,568 mismatched instances, with
nothing unique to either. `n/a` means the tool does not check for that defect.

### Precision on seeded dead code

A generated 5-package workspace with seeded unused files, exports, types and dependencies, next to live code that is
reached only through patterns that caused false findings on real repositories (namespace reads, `typeof import()`,
`import.meta.glob`, tsconfig `paths`, `#` subpath imports, manual mocks, fixtures and more). The answer key lists all
33 seeded findings. skif measured on 2026-11-08 and knip 6.44.1 on 2026-11-07, not re-measured for this release;
reproduce with `mise run bench:deadcode -- --knip`:

| Tool | Precision | Recall |
| --- | ---: | ---: |
| skif | 100% | 100% |
| knip | 77% | 100% |

skif reports no false finding; knip reports exports read only through `typeof import("./m")["name"]` and files in
`fixtures` directories. Per issue type and pattern:
[`tools/bench`](tools/bench/README.md#dead-code-precision-and-recall).

### Agreement with knip

Real repositories have no answer key for dead code, so this measures agreement, not correctness. A low share means the
tools differ, and either one could be right. Findings in files skif deliberately leaves out (fixtures, tests, non-code
files), inside files the other tool reports unused, in packages skif does not judge and in packages where knip could
not load a config are counted apart, and a repository whose knip run is not a valid comparison is left out of the sums.
Reading the disagreements found false findings on both sides
([ADR 0049](adrs/0049-js-corrections-from-adjudicating-knip-disagreements.md)).

<!-- parity:agreement:start -->
Findings knip@6.44.1 and skif share on 16 pinned repositories (angular, babel, cal-com, create-t3-turbo, jest, mastra, n8n, next-js, npm-cli, opencode, rules-js, rushstack, tanstack-query, vercel-examples, vscode, vue-core; 16 of them installed), from `tools/bench/baselines/parity.json`, recorded at skif `1c497a74f5fa`.

| Finding | Shared | Of skif's findings | Of knip's findings |
| --- | ---: | ---: | ---: |
| Unused files | 11,041 | 100% | 96% |
| Unused exports | 6,918 | 99% | 91% |
| Unused types | 3,328 | 99% | 89% |
| Unused dev dependencies | 1,236 | 99% | 98% |
| Unlisted dependencies | 819 | 97% | 60% |
| Unused dependencies | 630 | 98% | 95% |
| Unused namespace exports | 424 | 100% | 76% |
| Unused namespace types | 312 | 100% | 93% |
| Duplicate exports | 158 | 96% | 83% |
| Unused enum members | 129 | 100% | 24% |
| Unused namespace members | 105 | 99% | 94% |
| Unlisted binaries | 85 | 90% | 36% |
| Unresolved imports | 26 | 100% | 38% |
| Unused optional peer dependencies | 3 | 60% | 60% |
<!-- parity:agreement:end -->

Method, caveats and every run: [`tools/bench`](tools/bench/README.md).

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
