# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!-- Keep a Changelog repeats Added/Changed/Fixed headings per version. -->
<!-- markdownlint-disable MD024 -->

## [Unreleased]

### Changed

- **The project is renamed to skif.** It continues as an independent Rust dependency linter for
  JavaScript and TypeScript monorepos, hosted at `github.com/Goldziher/skif`.
- **A clean rebuild is planned.** The code is being reorganised into a library-first workspace of
  `skif-*` crates with a thin `skif` command line on top. Architecture decisions are recorded in `adrs/`.

### Added

- **Catalog resolution** for pnpm, package.json and bun catalogs, so `catalog:` specifiers are resolved
  rather than treated as opaque strings.
- **`incompatible-peer-dependencies`** reports a dependency whose version cannot satisfy a peer range
  declared by another dependency.
- **`member-toolchain-fields`** reports `packageManager` and `engines` fields in workspace members that
  disagree with the root.
- **`local-path-dependencies`** reports `file:` and `link:` specifiers that point outside the workspace.
- **Recursive workspace globs** (`**`) are expanded with pnpm semantics.
- **JSON output** for machine consumers.
