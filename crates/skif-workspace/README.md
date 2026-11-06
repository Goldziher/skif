# skif-workspace

Workspace discovery for npm, pnpm, yarn and bun: package manager detection, workspace globs, packages and manifests, catalogs and dependency instances, read through the `FileSystem` trait. It is part of [skif](https://github.com/Goldziher/skif), a dependency and package-manager linter for JavaScript
and TypeScript with first-class monorepo support.

Facts are gathered here; decisions belong to the rules.
Most users want the `skif` command (`cargo install skif-cli`, `npm install -D skif-cli`) or the `skif` facade crate;
this crate is an implementation detail of them and follows their version.
