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
also runs `scripts/check-*.sh` (version surfaces, exact oxc pins, workspace-inherited dependencies, vendored
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
6. Add or extend an end-to-end scenario under `scripts/e2e` when the rule changes what a package manager
   layout produces, and a bench in `tools/bench` when it changes the hot path.
7. Document the rule in the changelog.

## Architecture decisions

Substantive design changes get an ADR in [`adrs/`](adrs/index.md), linted with [adrlint](https://github.com/Goldziher/adrlint)
(`mise run adr:lint`). Accepted ADRs are locked by `adrlint.lock` and are never edited: when a decision evolves
or is replaced, write a new ADR that supersedes or amends it, add it to `adrs/index.md` and run
`adrlint lock`.

## Releasing

0.1.0 is the first release and contains the whole tool ([ADR 0017](adrs/0017-release-policy-a-single-complete-0-1-0.md)).

1. Bump every version surface at once with `scripts/release-bump.sh <version>`, then check them with
   `scripts/check-versions.sh`.
2. Move the `[Unreleased]` entries of `CHANGELOG.md` under the new version.
3. Tag the commit `v<version>` and push the tag. The `Publish Packages` workflow builds the release
   archives, publishes the crates, the npm packages, the Homebrew formula and the Scoop manifest, and moves
   the floating `v0` tag (the major version's tag, used as `Goldziher/skif@v0`) to the release.
   npm trusted publishing can only be configured for a package that already exists, so before the first release
   publish a placeholder of `skif` and of each of the six platform packages (`skif-darwin-arm64`,
   `skif-darwin-x64`, `skif-linux-arm64-gnu`, `skif-linux-arm64-musl`, `skif-linux-x64-gnu`,
   `skif-linux-x64-musl`) once by hand, then add the trusted publisher (this repository, `publish.yaml`) to each.
4. Before the first release, create the one-time registry state the workflow cannot create itself:
   - crates.io trusted publishing needs the crate to exist: reserve each of the 14 crates (`skif-core`,
     `skif-specifier`, `skif-config`, `skif-edit`, `skif-workspace`, `skif-lint`, `skif-report`, `skif-js-syntax`,
     `skif-js-plugins`, `skif-migrate`, `skif-rules`, `skif-js`, `skif`, `skif-cli`) with a placeholder publish
     ([ADR 0017](adrs/0017-release-policy-a-single-complete-0-1-0.md) allows it), then add the trusted publisher
     (this repository, `publish.yaml`) to each.
   - `Goldziher/homebrew-tap` needs a `Formula/` directory and `Goldziher/scoop-bucket` a `bucket/` directory.
   - The `HOMEBREW_TOKEN` secret must be able to push to both repositories.
5. The release must be tagged on a commit that is on `main` and has a green `ci.yml` run; the workflow stops
   otherwise (`scripts/check-release-gate.sh`).
6. Rehearse with the workflow's `dry-run` input: it builds every target and verifies the assets and the crate
   packages without publishing. The same dry run starts on every pull request that touches the release inputs
   (`Cargo.toml`, `Cargo.lock`, `crates/skif-node`, `crates/skif-cli`, `npm-package`, `action.yml` or a release
   script) and every Monday; do not tag while the last one is red.
7. The GitHub release stays a draft until crates.io is published and the npm package has run under Node 22, 24 and
   26 and Bun; only then is it made public, and `cargo binstall` is verified against the public release. A failed
   run therefore never leaves a public release without its registry packages; re-run it to finish.
8. `scripts/release-bump.sh` also writes the version into `action.yml` (`BAKED_VERSION`), which is what makes a
   SHA-pinned `Goldziher/skif@<sha>` install that release instead of the latest one. The release archives carry a
   build-provenance attestation (`gh attestation verify skif-<target>.tar.gz --repo Goldziher/skif`).
   packages without publishing.
9. In the release change, remove the pre-release notices: the "Pre-release" callout at the top of `README.md`, the
   "has not been released yet" wording in its Installation section, and the `Aside` marked `pre-release-notice` in
   `website/src/content/docs/start/introduction.mdx` and `installation.mdx`
   (`grep -rn "pre-release-notice" website/src` lists them).

## AI governance

Run `mise install`, then `mise run governance:hooks:install`. The governance tasks run the latest
ai-rulez through `npx -y ai-rulez@latest`, which needs the Node that mise installs.

Edit `.ai-rulez/` sources, then run `mise run governance:generate` and `npx -y ai-rulez@latest lock`.
`mise run governance:check` verifies the generated outputs are fresh.
