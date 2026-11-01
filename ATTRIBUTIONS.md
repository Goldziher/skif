# Attributions

This document acknowledges sources of forked or copied code used in the skif project. Ordinary
crates.io dependencies are tracked in `Cargo.toml` / `Cargo.lock` and governed by `deny.toml`; this
file covers code that was **copied or derived** into the repository from other projects.

## sherif

A zero-config linter for TypeScript and JavaScript monorepos. skif began as a fork of sherif and is
being rebuilt as a clean, restructured codebase; the workspace rules are being ported from it.

- **Source**: <https://github.com/QuiiBz/sherif>
- **Upstream commit**: `1eeac7104af1a6143d7f5d17d18d11635222ca8d`
- **License**: MIT
- **Authors**: Tom Lienard and the sherif contributors
- **Copyright**: Copyright (c) 2023 Tom Lienard
- **Location**: `src/` (rule logic, package and workspace collection; being ported into `crates/`)
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
