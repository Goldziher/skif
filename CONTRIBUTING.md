# Contributing to skif

Thanks for helping out. skif is a Rust dependency linter for JavaScript and TypeScript monorepos. Its
diagnostics gate other people's CI, so a false positive or an unstable rule ID is a regression.

Read the records in [`adrs/`](adrs/index.md) before your first change. They are short and they explain most of
the design.

## Setup

```bash
git clone https://github.com/Goldziher/skif.git
cd skif
mise install
mise run setup
```

`mise run setup` installs the pinned tools, fetches the crates and installs the git hooks.

## The gate

```bash
mise run check
```

It runs the format check, the lint (clippy with warnings denied, then poly), the tests, `schema:check`,
`consistency`, the rustdoc build with warnings denied and the facade feature matrix: the Rust part of what CI
enforces. CI also runs `mise run deny`, the MSRV build, `cargo-semver-checks`, shellcheck and the script test suites,
the pre-commit hook, npm addon, end-to-end (`mise run e2e <package-manager>`), Action and toolchain jobs, the ADR
lint (`mise run adr:lint`) and the documentation site build (`mise run docs:build`); run those you touch. Hooks run
the same checks on commit; do not commit with `--no-verify`. Commits are conventional (`feat:`, `fix:`,
`perf:`, `docs:`, `refactor:`, `test:`, `build:`, `ci:`, `chore:`).

`mise run deny` checks advisories, licenses, bans and sources; it needs `cargo-deny` installed locally.

`mise run parity` and `mise run diff:syncpack` compare skif's findings with the pinned knip and syncpack on the pinned
corpus (`tools/bench/corpus.toml`) and print the per-repository differences; `mise run corpus:check` checks both against
the baselines committed in `tools/bench/baselines`, as the weekly `corpus` workflow does. A change that moves
the findings of a rule on real code refreshes the baselines with `--write-baseline` in the same pull request
(see `tools/bench/README.md`, [ADR 0026](adrs/0026-parity-and-diff-tasks-on-a-pinned-corpus.md)). `mise run bench`
exits non-zero when a speed target of ADR 0001 fails.

`mise run schema:check` regenerates the JSON Schema of `skif.toml` and fails when `schema/skif.schema.json`
differs; refresh it with `cargo run -p skif-cli -- schema > schema/skif.schema.json`. The `consistency` CI job
also runs `scripts/check-*.sh` (version surfaces, oxc versions, workspace-inherited dependencies, vendored
file headers, rule ids against the code, plugin lists against the plugin table, and the LICENSE and ATTRIBUTIONS files each published crate must ship).

## Adding a rule

1. Give the rule a stable kebab-case ID. IDs are the public contract; add it to `rule-ids.txt`.
2. Implement it in `skif-rules` (manifest, policy, catalog) or `skif-js` (JS), with a fixture (under the crate's
   `tests/fixtures` or the top-level `fixtures/`) and a snapshot test for the diagnostics. Never edit `.snap` files by hand.
3. Fixers must keep the original indentation and newline style of the edited file.
4. Regenerate the reference with `SKIF_UPDATE_DOCS=1 cargo test -p skif --features js-analysis --test docs`
   and commit `docs/rules.md`; `scripts/check-rule-ids.sh` compares it with `rule-ids.txt`.
5. For a JS plugin, add its row to `skif-js-plugins`, list it in `crates/skif-js-plugins/README.md` and in
   `website/src/content/docs/reference/configuration.mdx` (`scripts/check-plugin-list.sh` compares both with the
   code), add a fixture project
   that exercises it ([ADR 0011](adrs/0011-plugin-model.md)), and compare the behaviour with the pinned knip commit recorded in `ATTRIBUTIONS.md`.
6. Add or extend an end-to-end scenario under `fixtures/e2e` when the rule changes what a package manager
   layout produces (see [End-to-end tests](#end-to-end-tests)), and a bench in `tools/bench` when it changes the
   hot path.
7. Document the rule in the changelog.

## End-to-end tests

`mise run e2e <package-manager>` builds skif and runs `scripts/e2e/run.sh <package-manager> target/debug/skif`,
where the manager is `npm`, `pnpm`, `yarn` (Yarn 2+), `yarn-classic` or `bun`. Every scenario under `fixtures/e2e`
that applies to the manager is generated, installed by the real manager under each of its linkers (pnpm isolated and
hoisted; Yarn 2+ Plug'n'Play, `node-modules` and `pnpm`; Bun default, hoisted and isolated), linted, recorded with
`skif baseline`, fixed and linted again. It needs `jq` and the network.

The version under test is the one `<pm> --version` prints outside a project; set `E2E_PM_VERSION` to test another.
The runner writes that version into every root's `packageManager`, so corepack's `pnpm` and `yarn` shims select it:
`E2E_PM_VERSION=4.10.0 scripts/e2e/run.sh yarn target/debug/skif` runs Yarn 4.10.0, and
`E2E_PM_VERSION=1.22.22 scripts/e2e/run.sh yarn-classic target/debug/skif` runs Yarn Classic. For npm and Bun, put
the release first on `PATH` (`npm install --global --prefix /tmp/npm12 npm@12`).
`scripts/e2e/resolve-pm-version.sh <package-manager> <newest|previous> [major]` prints the release CI tests; with
`major`, only releases of that major count, and CI passes `1` for `bun` and `yarn-classic` so a later major does not
enter the matrix unannounced.
`E2E_SCENARIOS` and `E2E_LINKERS` (space separated) limit a run.

A scenario is `fixtures/e2e/<name>/scenario.json` with `project/`, the files every manager shares (the root
`package.json` declares `workspaces`; the runner writes `pnpm-workspace.yaml`, the catalog and the linker setting),
and `overlays/<pm>/` for files one manager needs, where `<name>.merge.json` is deep-merged into `<name>.json`.
`scenario.json` names the managers and minimum versions it applies to, the roots to install, the expected exit code
and findings (`rule:package[:dependency][ in file]`) before and after the fix, an optional script that `<pm> run`
runs in one package and whether it succeeds under the layout, and overrides per manager and per `<pm>/<linker>` with
a `reason`. An override that records a skif bug instead of the correct answer names it in `known_issue`
(`#<issue> <title>`), which the runner prints as `KNOWN ISSUE`. `bash scripts/test-e2e-scripts.sh` generates every scenario for every manager and
linker without installing.

## Architecture decisions

Substantive design changes get an ADR in [`adrs/`](adrs/index.md), linted with [adrlint](https://github.com/Goldziher/adrlint)
(`mise run adr:lint`). Accepted ADRs are locked by `adrlint.lock` and are never edited: when a decision evolves
or is replaced, write a new ADR that supersedes or amends it, add it to `adrs/index.md` and run
`adrlint lock`.

## Releasing

Every release follows this checklist. The `Publish Packages` workflow (`.github/workflows/publish.yaml`) does the
publishing; the steps around it are by hand.

1. **Registry state.** npm and crates.io trusted publishing can only be configured on a package that exists, so a
   package the release publishes for the first time needs a placeholder publish by hand and then the trusted
   publisher (this repository, workflow `publish.yaml`):
   - npm: `skif-cli` and the platform package of every `napi.targets` entry in `npm-package/package.json`
     (`skif-darwin-arm64`, `skif-darwin-x64`, `skif-linux-arm64-gnu`, `skif-linux-arm64-musl`, `skif-linux-x64-gnu`,
     `skif-linux-x64-musl`, `skif-win32-arm64-msvc`, `skif-win32-x64-msvc`). A real run stops before it creates the
     release when one is missing (`scripts/check-npm-packages-exist.sh`), so check with
     `bash scripts/check-npm-packages-exist.sh` before tagging.
   - crates.io: every crate in `scripts/publish-crates.sh`. `skif-node` and `skif-bench` are `publish = false`.
   - `Goldziher/homebrew-tap` needs a `Formula/` directory and `Goldziher/scoop-bucket` a `bucket/` directory; the
     `HOMEBREW_TOKEN` secret must be able to push to both, and needs the `workflows` permission on this repository to
     move the floating tag.
2. **Bump.** `scripts/release-bump.sh <version>` rewrites every version surface (the workspace version and `=` pins,
   `npm-package/package.json`, `poly-hooks.toml`, the pre-commit launcher, `action.yml` `BAKED_VERSION`, the agent
   plugin manifests, the documentation and agent skill pins and `Cargo.lock`); `scripts/check-versions.sh` checks
   them. The JSON snapshots spell the
   version `[VERSION]`, so the bump changes none of them.
3. **Changelog.** Merge the `[Unreleased]` entries into one Added / Changed / Fixed set, with any breaking change
   listed first, and move them under `## [<version>] - <date>`. The CI `Public API (cargo-semver-checks)` job summary
   lists the crates whose public API needs a semver bump; quote it for the breaking changes. A real run refuses a tag
   whose `CHANGELOG.md` has no `## [<version>]` section.
4. **Rehearse.** Run the workflow with the `dry-run` input on the release commit. It builds every target, loads the
   npm addons on every platform, installs the packed npm tarballs on both Windows runners (npm and Bun) and packages
   the crates, without publishing. The same dry run starts on every pull request that touches the release inputs
   (`Cargo.toml`, `Cargo.lock`, `crates/skif-node`, `crates/skif-cli`, `npm-package`, `action.yml` or a release
   script) and every Monday; do not tag while the last one is red.
5. **Tag.** Tag a commit that is on `main` and has a green `ci.yml` run as `v<version>` and push the tag; the workflow
   stops otherwise (`scripts/check-release-gate.sh`). It builds the release archives, publishes the crates and the
   npm packages, the Homebrew formula and the Scoop manifest, and moves the floating tag (`v0.M` for a 0.x release,
   `vN` from 1.0.0; used as `Goldziher/skif@v0.3`) to the release.
6. **Watch the run.** crates.io is published only after every npm addon has loaded and installed, and the GitHub
   release stays a draft until crates.io is published and the published npm package has run under Node 22, 24 and
   26 and Bun; only then is it made public and `cargo binstall` verified against it. A failed run never leaves a
   public release without its registry packages; fix the cause and re-run it, which skips what is already published.

`action.yml` `BAKED_VERSION` is what makes a SHA-pinned `Goldziher/skif@<sha>` install that release instead of the
latest one. The release archives carry a build-provenance attestation
(`gh attestation verify skif-<target>.tar.gz --repo Goldziher/skif`).

## AI governance

Run `mise install`, then `mise run governance:hooks:install`. The governance tasks run the latest
ai-rulez through `npx -y ai-rulez@latest`, which needs the Node that mise installs.

Edit `.ai-rulez/` sources, then run `mise run governance:generate` and `npx -y ai-rulez@latest lock`.
`mise run governance:check` verifies the generated outputs are fresh.
