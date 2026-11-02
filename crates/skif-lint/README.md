# skif-lint

The rule engine: the `Rule` and `RuleProvider` traits, the `Registry`, `LintContext`, the diagnostic emitter, central ignores and severity overrides, and a deterministic parallel runner. It is part of [skif](https://github.com/Goldziher/skif), a dependency and package-manager linter for JavaScript
and TypeScript with first-class monorepo support.

Rules report through one emitter, so configured levels and `[[ignore]]` entries apply the same way to every rule.
Most users want the `skif` command (`cargo install skif-cli`, `npm install -D skif`) or the `skif` facade crate;
this crate is an implementation detail of them and follows their version.
