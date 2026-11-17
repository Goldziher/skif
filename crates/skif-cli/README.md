# skif-cli

The `skif` command-line tool: a dependency and package-manager linter for JavaScript and TypeScript monorepos
(npm, pnpm, yarn and bun), with autofixes.

## Install

```bash
cargo install skif-cli
cargo binstall skif-cli
```

The binary is named `skif`. The `skif` crate is the embeddable library and has no binary, so `cargo install skif` fails. Other
channels (npm, Homebrew, Scoop, GitHub release archives and the GitHub Action) are listed in the
[repository README](https://github.com/polylint/skif#installation).

## Usage

```bash
skif                 # lint the workspace or package containing the current directory
skif -r              # lint every workspace and standalone package at or below the current directory
skif --fix           # apply safe fixes, lint again, then run the package manager's install
skif --format json   # one JSON document on stdout
skif rules           # list every rule
skif config --show    # print the configuration files and the effective configuration
skif init            # write a starter skif.toml
skif cache stats     # show what the cache of this project holds; `skif cache clean` removes it
```

Parse results are cached between runs by default and the cache never changes the report: `--no-cache` turns it off,
and `--cache-location` and `--cache-max-size` place and cap it (see the
[CLI reference](https://polylint.github.io/skif/reference/cli/#flags)).

Exit codes: `0` clean, `1` problems remain, `2` skif could not run. The full flag and exit-code contract is in
the repository's [ADR 0012](https://github.com/polylint/skif/blob/main/adrs/0012-cli-contract-and-reporting.md).

Licensed under MIT.
