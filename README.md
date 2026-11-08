<!-- markdownlint-disable MD033 MD041 -->
<div align="center">

<h1 align="center"><img src="docs/media/skif-banner.svg" alt="skif: dependency linter for JavaScript and TypeScript monorepos" width="820"></h1>

**One fast binary for the dependency checks your monorepo spreads across three tools.**

skif lints manifests, enforces version and catalog policies, and finds unused dependencies and dead code,
with one config file and one report. It never runs your project's code.

npm · pnpm · yarn · bun · **42 rules** · **205 plugins** · format-preserving fixes

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
| **Fast** | 8 to 61 times faster than knip on real repositories. Manifest checks beat sherif on every real repository and syncpack on every target, and match sherif at 2,000 generated packages. [See the numbers.](#benchmarks) |
| **Safe** | Analysis is static: skif reads your code and configs without running them. Fixes keep key order, indentation and comments. |
| **Easy to adopt** | `skif migrate` translates your sherif, syncpack and knip configuration in one step. |
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

| | skif | sherif | syncpack | knip |
| --- | --- | --- | --- | --- |
| Manifest and workspace rules | Yes | Yes | No | No |
| Version and semver-range policy | Yes | Version mismatches | Yes | No |
| Catalog policy | Yes | No | Yes | Catalog entries |
| Registry updates (`update --check`) | Yes, opt-in | No | Yes | No |
| Unused dependencies, files and exports | Yes | No | No | Yes |
| Runs project code to read configs | No | No | No | Yes |

skif is younger than all three. The JS analysis is close to knip but not identical, and
[the differences are published](#agreement-with-knip).

## Benchmarks

Medians of 10 runs on a 4-vCPU Linux VM (Intel Xeon at 2.10 GHz) with no other work running, on
[pinned public repositories](tools/bench/corpus.toml) with dependencies installed, measured on 2026-11-08 at commit
`a923580` (after 0.1.1). Absolute times depend on the machine; the ratios are the result. Reproduce with
`mise run bench:publish`.

**Dead-code analysis against knip** (full skif run)

| Repository | skif | knip | Faster by |
| --- | ---: | ---: | ---: |
| create-t3-turbo | 35 ms | 2.1 s | 61x |
| vue-core | 72 ms | 1.9 s | 26x |
| tanstack-query | 415 ms | 5.4 s | 13x |
| babel | 1.0 s | 8.5 s | 8x |
| npm-cli | 124 ms | 1.1 s | 8x |

knip completes its analysis on all five. On create-t3-turbo and vue-core it then exits with an error because some of
the repositories' tool configurations do not load; on npm-cli it runs with `--no-gitignore`, since npm's `.gitignore`
otherwise hides every workspace from it.

**Manifest and policy checks** (`skif --no-js`)

| Target | skif | sherif | syncpack |
| --- | ---: | ---: | ---: |
| create-t3-turbo | 4.7 ms | 39 ms | 44 ms |
| vue-core | 4.7 ms | 38 ms | 50 ms |
| tanstack-query | 13 ms | 47 ms | 124 ms |
| babel | 10 ms | 44 ms | 337 ms |
| npm-cli | 5.9 ms | 44 ms | 88 ms |
| Generated, 50 packages | 6.4 ms | 39 ms | 53 ms |
| Generated, 500 packages | 34 ms | 68 ms | 159 ms |
| Generated, 2000 packages | 137 ms | 135 ms | 665 ms |

At 2,000 generated packages skif and sherif are level within the noise (124 ms against 131 ms in a separate 30-run
comparison), while skif reports about 24,700 diagnostics there and sherif 638.

### Accuracy on seeded defects

A generated 500-package workspace with defects injected on purpose. `Actual` is counted from the files, so the right
answer is known:

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
33 seeded findings:

| Tool | Precision | Recall |
| --- | ---: | ---: |
| skif | 100% | 100% |
| knip | 77% | 100% |

skif reports no false finding; knip reports exports read only through `typeof import("./m")["name"]` and files in
`fixtures` directories. skif was measured at the current source on 2026-11-08, knip 6.44.1 on 2026-11-07. Per issue
type and pattern: [`tools/bench`](tools/bench/README.md#dead-code-precision-and-recall).

### Agreement with knip

Real repositories have no answer key for dead code, so this measures agreement, not correctness: findings skif and
knip share on 14 pinned repositories. A low score means the tools differ, and either one could be right. Reading the disagreements found
false findings on both sides, and skif's are fixed in the current source
([ADR 0049](adrs/0049-js-corrections-from-adjudicating-knip-disagreements.md)).

| Finding | Shared | Of skif's findings | Of knip's findings |
| --- | ---: | ---: | ---: |
| Unused files | 27,193 | 92% | 73% |
| Unused exports | 9,997 | 96% | 86% |
| Unused types | 5,011 | 90% | 87% |
| Unlisted dependencies | 959 | 91% | 34% |
| Unused dev dependencies | 917 | 86% | 76% |
| Unused dependencies | 467 | 84% | 66% |
| Unresolved imports | 69 | 34% | 56% |
| Unlisted binaries | 32 | 89% | 21% |

Method, caveats and every run: [`tools/bench`](tools/bench/README.md).

## Installation

<details open>
<summary><strong>npm, pnpm, yarn or bun</strong></summary>

The package is `skif-cli`; the command it installs is `skif`.

```bash
pnpm add -D skif-cli     # or npm install -D, yarn add -D, bun add -D
pnpm dlx skif-cli        # or npx skif-cli, yarn dlx skif-cli, bunx -p skif-cli skif
```

Needs Node.js 22, 24 or 26, or Bun 1.x, on macOS or Linux. There are no npm packages for Windows; use Scoop or Cargo.

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
- uses: Goldziher/skif@v0.1
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
    rev: v0.1.0 # use the latest release tag
    hooks:
      - id: skif       # lints manifests, skipping JS analysis
      - id: skif-fix   # optional: also applies safe fixes
```

Needs neither Rust nor Node. See the [hooks guide](https://goldziher.github.io/skif/guides/integrations/) for Poly and
Lefthook.

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
