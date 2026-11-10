<!-- markdownlint-disable MD033 MD041 -->
<div align="center">

<h1 align="center"><img src="docs/media/skif-banner.svg" alt="skif: dependency linter for JavaScript and TypeScript monorepos" width="820"></h1>

**One fast binary for the dependency checks your monorepo spreads across three tools.**

skif lints manifests, enforces version and catalog policies, and finds unused dependencies and dead code,
with one config file and one report. It never runs your project's code.

npm · pnpm · yarn · bun · **46 rules** · **205 plugins** · format-preserving fixes

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
| **Fast** | 8 to 61 times faster than knip on real repositories. Manifest checks beat sherif and syncpack on every target, by 1.3 times or more at 2,000 generated packages. [See the numbers.](#benchmarks) |
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

Medians of 15 runs on a 4-vCPU Linux VM (Intel Xeon at 2.10 GHz) with no other work running, on
[pinned public repositories](tools/bench/corpus.toml) with dependencies installed, measured on 2026-11-08 at commit
`bbb0971` (0.1.1 with the manifest-check work of [#29](https://github.com/Goldziher/skif/issues/29)). Absolute times depend on the machine; the ratios are the result. Reproduce with
`mise run bench:publish`.

**Dead-code analysis against knip** (full skif run)

| Repository | skif | knip | Faster by |
| --- | ---: | ---: | ---: |
| create-t3-turbo | 32 ms | 2.0 s | 61x |
| vue-core | 62 ms | 1.7 s | 27x |
| tanstack-query | 375 ms | 4.5 s | 12x |
| babel | 884 ms | 7.2 s | 8x |
| npm-cli | 99 ms | 968 ms | 10x |

knip completes its analysis on all five. On create-t3-turbo and vue-core it then exits with an error because some of
the repositories' tool configurations do not load; on npm-cli it runs with `--no-gitignore`, since npm's `.gitignore`
otherwise hides every workspace from it.

**Manifest and policy checks** (`skif --no-js`)

| Target | skif | sherif | syncpack |
| --- | ---: | ---: | ---: |
| create-t3-turbo | 4.6 ms | 36 ms | 41 ms |
| vue-core | 5.5 ms | 36 ms | 43 ms |
| tanstack-query | 11 ms | 42 ms | 106 ms |
| babel | 9.4 ms | 37 ms | 280 ms |
| npm-cli | 5.3 ms | 38 ms | 77 ms |
| Generated, 50 packages | 6.3 ms | 39 ms | 46 ms |
| Generated, 500 packages | 27 ms | 59 ms | 156 ms |
| Generated, 2000 packages | 102 ms | 130 ms | 655 ms |

At 2,000 generated packages skif is 1.3 times faster than sherif (101 ms against 125 to 128 ms in two separate 30-run
comparisons), while skif reports about 24,700 diagnostics there and sherif 638.

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
knip share on 9 pinned repositories (create-t3-turbo, vue-core, tanstack-query, jest, babel, cal-com, mastra, vscode
and npm-cli; the other compared repositories await a baseline refresh). A low score means the tools differ, and either
one could be right. Findings in files skif deliberately leaves out (fixtures, tests, non-code files), in packages it
does not judge and in packages where knip could not load a config are counted apart. Reading the disagreements found
false findings on both sides, and skif's are fixed in the current source
([ADR 0049](adrs/0049-js-corrections-from-adjudicating-knip-disagreements.md)).

| Finding | Shared | Of skif's findings | Of knip's findings |
| --- | ---: | ---: | ---: |
| Unused exports | 4,691 | 98% | 94% |
| Unused files | 2,263 | 98% | 75% |
| Unused types | 2,247 | 96% | 93% |
| Unlisted dependencies | 973 | 98% | 36% |
| Unused dev dependencies | 687 | 87% | 97% |
| Unused dependencies | 229 | 60% | 78% |
| Unresolved imports | 54 | 76% | 61% |
| Unlisted binaries | 26 | 96% | 27% |

Method, caveats and every run: [`tools/bench`](tools/bench/README.md).

## Installation

<details open>
<summary><strong>npm, pnpm, yarn or bun</strong></summary>

The package is `skif-cli`; the command it installs is `skif`.

```bash
pnpm add -D skif-cli     # or npm install -D, yarn add -D, bun add -D
pnpm dlx skif-cli        # or npx skif-cli, yarn dlx skif-cli, bunx -p skif-cli skif
```

Needs Node.js 22, 24 or 26, or Bun 1.x, on macOS, Linux or Windows (x64 or arm64).

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
- uses: Goldziher/skif@v0.2
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
    rev: v0.2.0 # use the latest release tag
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
