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

It runs the format check, the lint (clippy with warnings denied, then poly) and the tests. Hooks run
the same checks on commit; do not commit with `--no-verify`. Commits are conventional (`feat:`, `fix:`,
`perf:`, `docs:`, `refactor:`, `test:`, `build:`, `ci:`, `chore:`).

`mise run deny` checks advisories, licenses, bans and sources; it needs `cargo-deny` installed locally.

`mise run schema:check` regenerates the JSON Schema of `skif.toml` and fails when `schema/skif.schema.json`
differs; refresh it with `cargo run -p skif-cli -- schema > schema/skif.schema.json`. The `consistency` CI job
also runs `scripts/check-*.sh` (version surfaces, exact oxc pins, workspace-inherited dependencies, vendored
file headers, and the LICENSE and ATTRIBUTIONS files each published crate must ship).

## Adding a rule

1. Give the rule a stable kebab-case ID. IDs are the public contract.
2. Add a fixture under `fixtures/` and a snapshot test for the diagnostics. Never edit `.snap` files by hand.
3. Fixers must keep the original indentation and newline style of the edited file.
4. Document the rule.

## Architecture decisions

Substantive design changes get an ADR in [`adrs/`](adrs/index.md), linted with [adrlint](https://github.com/Goldziher/adrlint)
(`mise run adr:lint`). Accepted ADRs are locked by `adrlint.lock` and are never edited: when a decision evolves
or is replaced, write a new ADR that supersedes or amends it, add it to `adrs/index.md` and run
`adrlint lock`.

## Releasing

Nothing is released until every phase of the plan is done; 0.1.0 is the first, complete release.

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
4. Rehearse with the workflow's `dry-run` input: it builds every target and verifies the assets and the crate
   packages without publishing.

## AI governance

Run `mise install`, then `mise run governance:hooks:install`. The governance tasks run the latest
ai-rulez through `npx -y ai-rulez@latest`, which needs the Node that mise installs.

Edit `.ai-rulez/` sources, then run `mise run governance:generate` and `npx -y ai-rulez@latest lock`.
`mise run governance:check` verifies the generated outputs are fresh.
