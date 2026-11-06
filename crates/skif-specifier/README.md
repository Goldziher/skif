# skif-specifier

npm dependency specifier parsing, classification, ordering and semver comparison, vendored from syncpack and made thread-safe. It is part of [skif](https://github.com/Goldziher/skif), a dependency and package-manager linter for JavaScript
and TypeScript with first-class monorepo support.

Every source file carries an attribution header naming its upstream path and commit; see `ATTRIBUTIONS.md`.
Most users want the `skif` command (`cargo install skif-cli`, `npm install -D skif-cli`) or the `skif` facade crate;
this crate is an implementation detail of them and follows their version.
