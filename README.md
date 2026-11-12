<!-- markdownlint-disable MD033 MD041 -->
<div align="center">

<h1 align="center"><img src="docs/media/skif-banner.svg" alt="skif: dependency linter for JavaScript and TypeScript monorepos" width="820"></h1>

**One fast binary for the dependency checks your monorepo spreads across three tools.**

npm · pnpm · yarn · bun · **53 rules** · **207 plugins** · format-preserving fixes

[![crates.io](https://img.shields.io/crates/v/skif?style=flat-square)](https://crates.io/crates/skif)
[![npm](https://img.shields.io/npm/v/skif-cli?style=flat-square)](https://www.npmjs.com/package/skif-cli)
[![CI](https://img.shields.io/github/actions/workflow/status/Goldziher/skif/ci.yml?style=flat-square)](https://github.com/Goldziher/skif/actions/workflows/ci.yml)
[![Docs](https://img.shields.io/badge/docs-goldziher.github.io%2Fskif-f07a1a?style=flat-square)](https://goldziher.github.io/skif/)
[![License: MIT](https://img.shields.io/badge/license-MIT-green?style=flat-square)](LICENSE)

[Docs](https://goldziher.github.io/skif/) · [Benchmarks](https://goldziher.github.io/skif/guides/benchmarks/) · [Comparison](https://goldziher.github.io/skif/guides/comparison/) · [Install](#installation) · [Quick start](#quick-start) · [Rules](docs/rules.md)

</div>

---

skif is a dependency linter for JavaScript and TypeScript repositories, written in Rust. It does the jobs of
[sherif](https://github.com/QuiiBz/sherif) (manifest and workspace rules), [syncpack](https://github.com/JamieMason/syncpack)
(version and catalog policies) and [knip](https://github.com/webpro-nl/knip) (unused files, exports and
dependencies) in one run, with one `skif.toml` and one report. It reads tool configurations statically and runs no
project code while linting, and its fixes keep key order, indentation and comments. `skif migrate` translates an
existing sherif, syncpack and knip setup in one step.

## Performance

<!-- bench:claims:start -->
- **6.0–9.0x faster than knip** (median 7.5x) for dead-code analysis, on 3 pinned public repositories where knip completes; it fails on create-t3-turbo and vue-core.
- **1.1–7.5x faster than sherif** (median 4.2x) for manifest and workspace checks (`skif --no-js`), on 5 pinned public repositories and 3 generated workspaces of 50, 500 and 2000 packages.
- **5.9–20.6x faster than syncpack** (median 8.9x) for version-policy checks (`skif --no-js`), on 5 pinned public repositories and 3 generated workspaces of 50, 500 and 2000 packages.

skif 0.3.0 against knip 6.44.1, sherif 1.13.0, syncpack 15.3.3: medians of 15 runs on a GitHub-hosted runner, measured on 2026-11-12.
<!-- bench:claims:end -->

The claims are generated from the latest imported run of the `bench` workflow. The
[benchmarks page](https://goldziher.github.io/skif/guides/benchmarks/) has every table, the workflow run the numbers
come from, the methodology, the accuracy on seeded defects and the agreement with knip on real repositories.

## Installation

```bash
pnpm add -D skif-cli                  # or npm install -D, yarn add -D, bun add -D; the command is `skif`
cargo binstall skif-cli               # prebuilt binary; or cargo install skif-cli
brew install goldziher/tap/skif       # Homebrew
```

```powershell
scoop bucket add goldziher https://github.com/Goldziher/scoop-bucket
scoop install skif
```

The npm package needs Node.js 22, 24 or 26, or Bun 1.x, on macOS or Linux. Windows has no npm package
([ADR 0027](adrs/0027-distribution-and-release.md)): on Windows, use cargo-binstall, Scoop or the
[release archives](https://goldziher.github.io/skif/start/installation/#prebuilt-binaries).

GitHub Action:

```yaml
- uses: actions/checkout@v7
- uses: Goldziher/skif@v0.4
  with:
    fail-on-warnings: true
```

pre-commit or prek (needs neither Rust nor Node):

```yaml
repos:
  - repo: https://github.com/Goldziher/skif
    rev: v0.4.0 # use the latest release tag
    hooks:
      - id: skif       # lints manifests, skipping JS analysis
      - id: skif-fix   # optional: also applies safe fixes
```

The [installation guide](https://goldziher.github.io/skif/start/installation/) lists every channel, the prebuilt
archives and the supported versions; the [integrations guide](https://goldziher.github.io/skif/guides/integrations/)
covers the Action inputs, Poly, Lefthook, GitLab CI and the npm API.

## Quick start

```bash
npx skif-cli                  # lint the workspace or package you are in
npx skif-cli --fix --dry-run  # show the safe fixes as a diff
npx skif-cli --fix            # apply them, then run the package manager's install
npx skif-cli migrate          # translate sherif, syncpack and knip config into a skif.toml
npx skif-cli baseline write   # record the findings you cannot fix yet; CI then accepts only fewer
```

skif works without configuration. `skif init` writes a starter `skif.toml`, and files layer from a project up to the
git root, so one organisation file applies to every project below it. See the
[quickstart](https://goldziher.github.io/skif/start/quickstart/) and the
[configuration reference](https://goldziher.github.io/skif/reference/configuration/).

## Features

- **Manifest and workspace rules**: root `private` and `packageManager`, sorted and empty dependency maps, `@types`
  in `dependencies`, unmatched workspace patterns, stray lockfiles, nested workspace roots, Lerna configuration.
- **Version policy**: version and semver-range groups, banned dependencies, dependency families, the `workspace:`
  protocol, catalogs (pnpm, Bun, Yarn 4.10+) and toolchain pins (`packageManager`, `engines`, `.nvmrc`, Dockerfiles,
  workflows, `MODULE.bazel`). `skif update` finds newer published versions; it is the only command that uses the
  network.
- **Dead-code analysis on [oxc](https://oxc.rs)**: unused files, exports, types, enum and namespace members and
  `package.json` subpaths; unused, unlisted and unresolved dependencies and binaries; duplicate exports; circular
  imports. Vue, Svelte, Astro, MDX and Marko files are read, and 207 plugins read the configuration of common tools.
  `skif explain-usage` says why a file, export or dependency counts as used.
- **Monorepo layouts**: npm, pnpm, Yarn 2+, Yarn Classic and Bun workspaces, single packages, `skif -r` over many
  roots, Rush, [Turborepo, Nx and Lerna](https://goldziher.github.io/skif/guides/turborepo-nx-lerna/), and
  [Bazel with rules_js](https://goldziher.github.io/skif/guides/bazel/).
- **Fixes**: safe and unsafe levels, `--fix-type` to fix some rules only, `--dry-run` diffs, and `skif format` for
  `package.json` layout.
- **CI output**: human, JSON, SARIF, GitHub annotations, GitLab Code Quality, compact, Markdown and CODEOWNERS
  reports, several in one run, with stable rule ids and documented exit codes.
- **Editors and agents**: watch mode, a language server, an MCP server and an
  [agent skill](https://goldziher.github.io/skif/guides/editors-and-agents/#agent-skill).
- **Library**: the [`skif`](crates/skif/README.md) crate does no I/O of its own, and the npm package exports `lint`,
  `lintAsync` and `run`. See the [library guide](https://goldziher.github.io/skif/guides/library/).

## Support

| | skif |
| --- | --- |
| Package managers | npm, pnpm, Yarn 2+, Yarn Classic, Bun |
| Linkers tested end to end in CI | npm (hoisted); pnpm (isolated, `nodeLinker: hoisted`); Yarn 2+ (Plug'n'Play, `nodeLinker: node-modules`, `nodeLinker: pnpm`); Yarn Classic (hoisted); Bun (default, `linker = "hoisted"`, `linker = "isolated"`) |
| Runtime | Native binary, or through npm on Node.js 22, 24, 26 and Bun 1.x |
| Platforms | macOS, Linux (glibc, musl), Windows; x64, arm64 (Windows not on npm) |

Versions per package manager and operating system are under
[supported versions](https://goldziher.github.io/skif/start/installation/#supported-versions). The
[comparison guide](https://goldziher.github.io/skif/guides/comparison/) compares skif with sherif, syncpack and knip
capability by capability, with sources.

## Documentation

- [Introduction](https://goldziher.github.io/skif/start/introduction/) and
  [quickstart](https://goldziher.github.io/skif/start/quickstart/)
- [Rules](https://goldziher.github.io/skif/rules/) ([`docs/rules.md`](docs/rules.md))
- [CLI reference](https://goldziher.github.io/skif/reference/cli/) and
  [configuration reference](https://goldziher.github.io/skif/reference/configuration/)
- [Migrating from sherif, syncpack and knip](https://goldziher.github.io/skif/guides/migrating/)
- [Benchmarks](https://goldziher.github.io/skif/guides/benchmarks/) and
  [comparison](https://goldziher.github.io/skif/guides/comparison/)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md). Design decisions are recorded as
[architecture decision records](adrs/index.md).

## License

MIT, see [LICENSE](LICENSE). skif started as a fork of sherif, vendors parts of syncpack and follows knip's issue types
and plugin knowledge; derived code is credited in [ATTRIBUTIONS.md](ATTRIBUTIONS.md).
