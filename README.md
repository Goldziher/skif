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

skif is not published yet. The first release will provide:

```bash
cargo install skif-cli
cargo binstall skif-cli
npm install --save-dev skif
```

Until then, build from source with `cargo build --release`.

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
