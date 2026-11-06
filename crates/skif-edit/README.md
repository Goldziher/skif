# skif-edit

Format-preserving edits for autofixes: structural JSON operations on `package.json` that keep key order, indentation and newline style, and (with the `yaml` feature) YAML edits for `pnpm-workspace.yaml`. It is part of [skif](https://github.com/Goldziher/skif), a dependency and package-manager linter for JavaScript
and TypeScript with first-class monorepo support.

The crate turns `skif_core::FixOp`s into byte-level text edits and never touches the disk.
Most users want the `skif` command (`cargo install skif-cli`, `npm install -D skif-cli`) or the `skif` facade crate;
this crate is an implementation detail of them and follows their version.
