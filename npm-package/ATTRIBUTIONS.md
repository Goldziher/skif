# Attributions

This document acknowledges sources of forked or copied code used in the skif project. Ordinary
crates.io dependencies are tracked in `Cargo.toml` / `Cargo.lock` and governed by `deny.toml`; this
file covers code that was **copied or derived** into the repository from other projects.

## sherif

A zero-config linter for TypeScript and JavaScript monorepos. skif began as a fork of sherif and was
rebuilt as a clean, restructured codebase; the workspace and manifest rules are ported from it.

- **Source**: <https://github.com/QuiiBz/sherif>
- **Upstream commit**: `1eeac7104af1a6143d7f5d17d18d11635222ca8d`
- **License**: MIT
- **Authors**: Tom Lienard and the sherif contributors
- **Copyright**: Copyright (c) 2023 Tom Lienard
- **Location**: `crates/skif-rules` and `crates/skif-workspace` (rule logic, package and workspace collection)
- **Purpose**: Monorepo workspace rules (dependency hygiene, root and member `package.json`
  checks, workspace discovery for npm, yarn and pnpm) and their autofixes.

### Modifications

- Renamed from sherif to skif; the crate, binary, repository, and distribution surface are new.
- Restructured from a single binary crate into a library-first workspace of `skif-*` crates.
- Rules are rewritten and renamed to stable rule IDs, with structured diagnostics instead of
  pre-rendered messages.
- Configuration comes from `skif.toml` / `.skif.toml` and CLI flags; the `sherif` key in
  `package.json` and the `sherif.config.jsonc` / `.sherifrc.jsonc` files are not supported.
- The upstream GitHub Action, npm packaging, and website assets were removed.

### License Compatibility

MIT is permissive and compatible with skif's MIT license. The upstream license text follows.

```text
MIT License

Copyright (c) 2023 Tom Lienard

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## syncpack specifier

The npm dependency specifier parser and comparator from syncpack, a consistent-versions tool for
JavaScript monorepos.

- **Source**: <https://github.com/JamieMason/syncpack>
- **Upstream path**: `crates/syncpack-specifier`
- **Upstream commit**: `958d30689ac24b60623258630242330bd6d0264b`
- **License**: MIT
- **Copyright**: Copyright (c) 2017-2023 Jamie Mason
- **Location**: `crates/skif-specifier`
- **Purpose**: Parsing, classification, ordering, and semver comparison of npm dependency
  specifiers (exact, ranges, workspace protocol, aliases, git, tags, and so on).

### Modifications

1. Every vendored source file carries a header comment naming its upstream path and commit.
2. Thread safety: `Rc` is replaced with `Arc` and `OnceCell` with `OnceLock`, and the
   `thread_local!` caches are removed. `Specifier::new` and `Specifier::create` parse without
   caching; the caller-owned `SpecifierCache` (`RwLock<HashMap>`, `get_or_parse`) provides
   interning. Static assertions check that `Specifier` and `SpecifierCache` are `Send + Sync`.
3. `node-semver` 2.2.0 is replaced with `nodejs-semver` 7.0.0 (Apache-2.0). `Range` and `Version`
   parsing, `satisfies`, and `allows_any` map one to one; the only adaptation is that version
   components are read through accessor methods (`major()`, `minor()`, `patch()`,
   `pre_release()`) instead of public fields.
4. The upstream tests and test data are kept. `eq_hash_test::specifier_eq_by_raw_string` now
   obtains pointer-equal specifiers through `SpecifierCache` because `Specifier::new` no longer
   interns. The upstream `#[ignore]`d prerelease test stays ignored, with a reason attached.
5. Lint fixes to the skif workspace standard (clippy pedantic, no `unwrap`/`expect` in non-test
   code where avoidable); two crate-level lint allowances and a per-function `expect_used`
   allowance are documented in place. The code is formatted with the workspace `rustfmt.toml`,
   and the benchmark crate name is updated.
6. Hardening: `Major`, `Minor`, `RangeMajor`, `RangeMinor` and `Latest` parse their padded
   versions and ranges eagerly (no `OnceLock`, no `expect`); numbers that `nodejs-semver` rejects
   (for example `99999999999999999999`, above 2^53 - 1) now yield `Specifier::Unsupported`
   instead of panicking later. `Ord` breaks semantic ties on the raw string so that
   `cmp == Equal` exactly when `==` does (`1.2.3` and `=1.2.3` no longer compare equal).
   `is_complex_range` accepts unspaced `||` unions, prerelease tags and x-range operands
   (`1.x || 2.x`), and only a bare `*`, `x` or `X` is `Latest` (`*foo` is `Unsupported`, `xyz` is
   a `Tag`). Property tests cover panic freedom and the `Ord`/`Eq` law.
7. The version-group state machine in `crates/skif-rules/src/policy/visit.rs` is a Rust re-implementation of
   the visitors in syncpack's `src/version_group/*.rs` (preferred-semver, pinned, banned, ignored, same-range,
   same-minor, snapped-to) at the same commit, on skif's own instance model; no syncpack code is copied. It differs
   in having no strict mode, leaving `catalog:` references to the catalog rules, and treating specifiers that
   cannot carry a semver range as having no preferred range.

### License Compatibility

MIT is permissive and compatible with skif's MIT license. The upstream license text follows.

```text
MIT License

Copyright (c) 2017-2023 Jamie Mason

Permission is hereby granted, free of charge, to any person obtaining a copy of
this software and associated documentation files (the "Software"), to deal in
the Software without restriction, including without limitation the rights to
use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software is furnished to do so,
subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR
COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
```

## knip

A tool that finds unused files, exports and dependencies in JavaScript and TypeScript projects. skif's JS
analysis follows its semantics and its plugin set; the behaviour is re-implemented in Rust on oxc, and no knip
source file is copied, apart from the data noted below.

- **Source**: <https://github.com/webpro-nl/knip>
- **Upstream commit**: `da07b6e9887f9a1eb08545da29755b7f9d309bca`
- **License**: ISC
- **Authors**: Lars Kappert and the knip contributors
- **Copyright**: Copyright (c) 2022-2026 Lars Kappert
- **Location**: `crates/skif-js` (issue types, module graph and usage rules) and `crates/skif-js-plugins` (the
  plugin table, entry patterns and configuration readers)
- **Purpose**: Unused and unlisted dependency, unused export, unresolved import and dead file analysis.

### Modifications

- The behaviour of knip's issue types, plugins and production mode is re-implemented in Rust; no TypeScript
  source is translated line by line.
- Analysis is static: configuration files are read with oxc, never executed, and tools are not run to ask for
  their entries.
- The list of binaries that are never reported as unlisted is adapted from knip's `constants.ts`.
- The plugin table is declarative data with optional resolvers, keyed by knip's plugin names. `aws-cdk` and
  `browser-extension` are skif's own: knip has no plugin of either name at the pinned commit.
- Rule ids, severities and configuration (`skif.toml`) are skif's own.

### License Compatibility

ISC is permissive and compatible with skif's MIT license. The upstream license text follows.

```text
ISC License

Copyright (c) 2022-2026 Lars Kappert

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
```
