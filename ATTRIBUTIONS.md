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
