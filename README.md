<!-- markdownlint-disable MD033 MD041 -->
<div align="center">

# skif

**A dependency and package-manager linter, auditor and enforcer for JavaScript and TypeScript, with first-class monorepo support.**

Written in Rust. A single-binary CLI that is also an embeddable library, for npm, pnpm, yarn and bun workspaces.

[![crates.io](https://img.shields.io/crates/v/skif?style=flat-square)](https://crates.io/crates/skif)
[![npm](https://img.shields.io/npm/v/skif?style=flat-square)](https://www.npmjs.com/package/skif)
[![CI](https://img.shields.io/github/actions/workflow/status/Goldziher/skif/ci.yml?style=flat-square)](https://github.com/Goldziher/skif/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-green?style=flat-square)](LICENSE)

</div>

---

> **Pre-release.** skif has not been published. Nothing is released until every phase of the plan is
> done; 0.1.0 is the first, complete release. Sections marked **planned** describe work that is
> designed (see the [ADRs](adrs/index.md)) but not yet built.

skif combines three kinds of checks that are usually spread over separate tools:

- **Monorepo and manifest hygiene**: root and member `package.json` rules, workspace pattern checks,
  sorted and well-placed dependency fields (the [sherif](https://github.com/QuiiBz/sherif) lineage).
- **Version, semver-range and catalog policies**: version groups, range policies, workspace protocol
  and catalog enforcement across packages (the [syncpack](https://github.com/JamieMason/syncpack)
  lineage). **Planned.**
- **Unused, unlisted and unresolved dependencies and dead code**: static analysis of JavaScript and
  TypeScript sources on [oxc](https://oxc.rs) (the [knip](https://github.com/webpro-nl/knip) lineage).
  **Planned.**

It works across npm, pnpm, yarn and bun workspaces. skif never executes project code: analysis is
static, and fixes are format-preserving edits that keep key order, indentation and comments.

## Status

| Capability | State |
| --- | --- |
| Manifest and workspace rules (table below), autofixes | Available |
| CLI: human and JSON output, exit codes, `--fix`, rule and ignore overrides, `rules`, `schema`, `init` | Available |
| Embeddable library facade (`skif` crate) | Available |
| `skif.toml` configuration: rule levels, ignores, workspace settings | Available |
| Version groups, semver groups, catalog and workspace-protocol policies | Planned ([ADR 0009](adrs/0009-version-and-catalog-policies.md)) |
| Unused / unlisted / unresolved dependency and dead-code analysis | Planned ([ADR 0010](adrs/0010-js-analysis-on-oxc.md), [ADR 0011](adrs/0011-plugin-model.md)) |
| SARIF and GitHub annotation output | Planned ([ADR 0012](adrs/0012-reporting.md)) |

## Rules

Run `skif rules` (or `skif rules --format json`) to list the registry. Every rule has a stable
kebab-case id, a category and a default level. Fixes are `safe` (applied by `--fix`), `unsafe`
(applied only by `--fix-unsafe`) or `sometimes` (fixable for some findings).

| Rule | Category | Default | Fix |
| --- | --- | --- | --- |
| `empty-dependencies` | manifest | error | safe |
| `incompatible-peer-dependencies` | manifest | warn | none |
| `local-path-dependency` | manifest | error | sometimes |
| `member-toolchain-fields` | manifest | warn | unsafe |
| `missing-package-manifest` | workspace | warn | unsafe |
| `root-package-dependencies` | manifest | warn | unsafe |
| `root-package-manager-field` | manifest | error | none |
| `root-package-private-field` | manifest | error | safe |
| `types-in-dependencies` | manifest | error | safe |
| `unmatched-workspace-patterns` | workspace | warn | safe |
| `unsorted-dependencies` | manifest | error | safe |

Policy and analysis rules will join this registry as those phases land.

## Quick start

```bash
skif                      # lint the workspace containing the current directory
skif path/to/pkg          # the workspace root is the nearest ancestor that declares a workspace
skif --fix                # apply safe fixes, lint again, then run the package manager's install
skif --fix-unsafe         # also apply unsafe fixes
skif --format json        # one JSON document on stdout
skif --rule unsorted-dependencies=off --fail-on-warnings
skif init                 # write a starter skif.toml
skif rules                # list every rule
skif schema               # print the JSON Schema of skif.toml
```

Exit codes: `0` clean (warnings within `--max-warnings`), `1` problems remain, `2` skif could not run
(bad configuration, bad flags, refused `--fix`, failed install). `--fix` is refused when `CI` is
truthy. In `--format json` mode failures are also JSON. Other flags: `--output FILE`, `--select
highest|lowest`, `--no-install`, `--max-warnings N`, `--ignore-rule`, `--ignore-package`,
`--ignore-dependency`, `--ignore-rule-for-package RULE:PACKAGE`, `--no-js`, `--production`, `--color`.
The full contract is [ADR 0016](adrs/0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md).

## Installation

skif has not been released yet. Every channel below is available from the first release; until then,
build from source with `cargo build --release`.

```bash
# Rust toolchain
cargo install skif-cli
cargo binstall skif-cli

# npm (Node 24+ and Bun; macOS and Linux only, no Windows npm packages); the package is `skif`
pnpm add -D skif        # or: npm install -D skif / yarn add -D skif / bun add -D skif
pnpm dlx skif           # run without installing; also: npx skif / yarn dlx skif / bunx skif

# Homebrew (macOS and Linux)
brew install goldziher/tap/skif

# Scoop (Windows)
scoop bucket add goldziher https://github.com/Goldziher/scoop-bucket
scoop install skif
```

`cargo install skif` installs the library crate, not the binary; the command-line tool is `skif-cli`.

Channels follow [ADR 0013](adrs/0013-distribution-and-naming.md). The npm package moves to napi-rs and targets Node 24+ and Bun, with no Windows npm packages; Windows users get the GitHub release archive, Scoop or cargo.

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

skif reads one TOML file, `skif.toml` or `.skif.toml`, at the workspace root; having both is an
error. There is no `package.json` key and no compatibility reader for sherif, syncpack or knip
configuration. `skif init` writes a starter file and `skif schema` prints its JSON Schema.

```toml
[workspace]
ignore-packages = ["examples/*"]

[rules]
unsorted-dependencies = "warn"
empty-dependencies = "off"

[[ignore]]
rules = ["types-in-dependencies"]
packages = ["legacy-app"]
```

Top-level sections: `[workspace]`, `[lint]`, `[rules]`, `[[ignore]]`, `[policy]`,
`[[version-groups]]`, `[[semver-groups]]`, `[catalog]`, `[workspace-protocol]`,
`[[dependency-families]]` and `[js]`. The policy and `[js]` sections are parsed and validated today
but only take effect when the policy and analysis phases land. See
[ADR 0004](adrs/0004-configuration-model.md).

## Output

`--format human` (default) prints diagnostics grouped by file with an optional colour and a summary.
`--format json` prints one versioned document (`version`, `summary`, `diagnostics`, each with rule,
severity, message, location with line and column span, related locations and fix titles); the schema
is documented in [`crates/skif-report`](crates/skif-report/README.md).

## Library usage

The `skif` crate is a facade that never prints, prompts, exits, reads the environment or writes
files. Everything is read through a `FileSystem` (`OsFs` on disk, `MemoryFs` for editor buffers and
tests), diagnostics are relative to the workspace root, and fixes come back as a `FixPlan` for the
caller to show, merge or write. Poly is the first host.

```rust
use std::path::Path;
use skif::{ConfigOverrides, Linter, MemoryFs, Workspace, find_workspace_root, load_config};

let fs = MemoryFs::new()
    .with_file("/repo/package.json", r#"{"name": "root", "private": true, "workspaces": ["packages/*"]}"#)
    .with_file("/repo/packages/a/package.json", r#"{"name": "a", "version": "1.0.0"}"#);

let root = find_workspace_root(Path::new("/repo/packages/a"), &fs)?;
let config = load_config(&root, &fs, &ConfigOverrides::default())?;
let workspace = Workspace::discover_with_fs(&root, &config, &fs)?;
let report = Linter::new(&config)?.run(&workspace)?;
# Ok::<(), skif::Error>(())
```

Cargo features: `manifest` and `policy` (default), `js-analysis` (reserved), `fix`, `yaml-fix`,
`report`, `schema`. See [ADR 0003](adrs/0003-library-first-embedding.md) and
[ADR 0002](adrs/0002-crate-layout-and-publishing.md) for the crate layout.

## How it compares

skif builds on three projects and does not claim to replace them yet. The comparison is about scope.

| Tool | Scope | Relation to skif |
| --- | --- | --- |
| [sherif](https://github.com/QuiiBz/sherif) | Zero-config monorepo manifest rules | skif began as a fork; the manifest rules are ported with stable ids and `skif.toml` configuration. |
| [syncpack](https://github.com/JamieMason/syncpack) | Version consistency, semver ranges, catalogs | Same problem space; skif's policy phase is **planned** and reuses syncpack's specifier model (vendored in `skif-specifier`). |
| [knip](https://github.com/webpro-nl/knip) | Unused files, exports and dependencies | Same problem space; skif's analysis phase is **planned**, on oxc, with static config extraction instead of running tools. |
| [manypkg](https://github.com/Thinkmill/manypkg) | Workspace consistency checks and fixes | Overlapping manifest checks; skif adds a rule registry, severities and a library API. |

skif aims to offer all three in one tool with one configuration file and one report format. Today only
the manifest layer exists. There are no performance claims: a benchmark harness lives in
[`tools/bench`](tools/bench/README.md), and numbers will be published once it has results.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) and [SECURITY.md](SECURITY.md). Design decisions are recorded
as architecture decision records in [`adrs/`](adrs/index.md); read them before proposing a change in
direction.

## License and attribution

MIT, see [LICENSE](LICENSE). skif started as a fork of sherif and vendors parts of syncpack; code
derived from other projects is credited in [ATTRIBUTIONS.md](ATTRIBUTIONS.md). The knip project
informs the design of the analysis phase.
