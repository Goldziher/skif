# skif-js

JavaScript and TypeScript analysis on oxc: the module graph, entry and project file discovery, and the 14 `js` rules (`unused-files`, `unused-dependencies`, `unlisted-dependencies`, `unresolved-imports`, `unused-exports` and the rest of [`docs/rules.md`](https://github.com/Goldziher/skif/blob/main/docs/rules.md)). It is part of [skif](https://github.com/Goldziher/skif), a dependency and package-manager linter for JavaScript
and TypeScript with first-class monorepo support.

It re-implements the behaviour of knip; sources are analysed statically and never executed. oxc is pinned exactly.
Most users want the `skif` command (`cargo install skif-cli`, `npm install -D skif`) or the `skif` facade crate;
this crate is an implementation detail of them and follows their version.
