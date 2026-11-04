# skif-cli

The `skif` command-line tool: a dependency and package-manager linter for JavaScript and TypeScript monorepos
(npm, pnpm, yarn and bun), with autofixes.

## Install

```bash
cargo install skif-cli
cargo binstall skif-cli
```

The binary is named `skif`. `cargo install skif` installs the embeddable library crate, not the tool. Other
channels (npm, Homebrew, Scoop, GitHub release archives and the GitHub Action) are listed in the
[repository README](https://github.com/Goldziher/skif#installation).

## Usage

```bash
skif                 # lint the workspace or package containing the current directory
skif -r              # lint every workspace and standalone package at or below the current directory
skif --fix           # apply safe fixes, lint again, then run the package manager's install
skif --format json   # one JSON document on stdout
skif rules           # list every rule
skif config --show    # print the configuration files and the effective configuration
skif init            # write a starter skif.toml
```

Exit codes: `0` clean, `1` problems remain, `2` skif could not run. The full flag and exit-code contract is in
the repository's [ADR 0016](https://github.com/Goldziher/skif/blob/main/adrs/0016-cli-contract-flags-exit-codes-and-machine-readable-errors.md).

Licensed under MIT.
