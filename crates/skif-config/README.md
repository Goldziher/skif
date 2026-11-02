# skif-config

The typed model of `skif.toml` and `.skif.toml`: loading and discovery, defaults, file and CLI-override precedence, per-rule options and the JSON Schema (`schema` feature). It is part of [skif](https://github.com/Goldziher/skif), a dependency and package-manager linter for JavaScript
and TypeScript with first-class monorepo support.

Config is data in, `Config` out; nothing here reads the environment or prints.
Most users want the `skif` command (`cargo install skif-cli`, `npm install -D skif`) or the `skif` facade crate;
this crate is an implementation detail of them and follows their version.
