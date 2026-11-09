# skif-js

JavaScript and TypeScript analysis on oxc: the module graph, entry and project file discovery, and the 18 `js` rules (`unused-files`, `unused-dependencies`, `unlisted-dependencies`, `unresolved-imports`, `unused-exports` and the rest of [`docs/rules.md`](https://github.com/Goldziher/skif/blob/main/docs/rules.md)). It is part of [skif](https://github.com/Goldziher/skif), a dependency and package-manager linter for JavaScript
and TypeScript with first-class monorepo support.

It follows knip's issue types and its knowledge of tools, without claiming the same findings: a weekly harness compares both on a pinned corpus and publishes where they differ. Sources are analysed statically and never executed. oxc is pinned exactly.
Most users want the `skif` command (`cargo install skif-cli`, `npm install -D skif-cli`) or the `skif` facade crate;
this crate is an implementation detail of them and follows their version.
