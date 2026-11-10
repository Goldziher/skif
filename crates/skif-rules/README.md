# skif-rules

The built-in manifest, workspace, policy and catalog rules: sherif-derived manifest checks, syncpack-style version and semver groups, workspace-protocol and catalog rules. It is part of [skif](https://github.com/Goldziher/skif), a dependency and package-manager linter for JavaScript
and TypeScript with first-class monorepo support.

`skif_rules::providers()` returns the rule providers, the rules over version files, Dockerfiles, CI workflows and
shell scripts included; rules that read files read them through the run's `FileSystem`. Rule ids are a stable public
contract.
Most users want the `skif` command (`cargo install skif-cli`, `npm install -D skif-cli`) or the `skif` facade crate;
this crate is an implementation detail of them and follows their version.
