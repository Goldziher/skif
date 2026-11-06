---
status: accepted
date: 2026-11-06
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0013", "0018"]
---

# npm Package Name skif-cli

## Context and Problem Statement

[ADR 0013](0013-distribution-and-naming.md) ships one name, `skif`, on every channel, and [ADR 0018](0018-npm-distribution-via-napi-rs.md) names the npm umbrella package `skif`. The name was free, but npm refuses to publish it: `403 Package name too similar to existing packages swig,sift`. Availability on the registry does not predict this; the check runs only at publish.

## Decision Drivers

* The first release needs an npm package that the registry accepts
* The project, crates, command and configuration names should not change for one registry
* The platform packages `skif-darwin-arm64` and its five siblings are already reserved

## Considered Options

* Umbrella `skif-cli`, keeping the project name and the reserved platform packages
* Scoped umbrella `@goldziher/skif`, with scoped platform packages
* Rename the project to a name npm accepts
* Ask npm support for an exception

## Decision Outcome

Chosen option: "Umbrella `skif-cli`, keeping the project name and the reserved platform packages", because it matches the `skif-cli` crate, keeps `npx` short and changes one registry name only. Renaming was tried first: `orde` and `taso` were also refused, because short names are usually close to a popular package.

**The umbrella npm package is `skif-cli`.** Its bin stays `skif`, so an installed project still runs `skif`. Library users import from `"skif-cli"`.

**The platform packages keep their names.** `napi.packageName` in `npm-package/package.json` stays `skif`, which is what the generated loader and `optionalDependencies` use.

**Bun needs the package named explicitly.** The documented form is `bunx -p skif-cli skif`, the form `bunx` documents for a binary whose name differs from its package.

The git hooks' npm channel runs `pnpm dlx skif-cli@<version>`. Everything else in ADR 0013 and ADR 0018 stands.

### Consequences

* Good, because every name but one stays `skif`, and `skif-cli` is the same name on crates.io and npm.
* Bad, because the npm package name and the command differ, so `npx skif` is not this tool.
* Bad, because the platform package names no longer derive from the umbrella's name; `napi.packageName` carries that link.

### Confirmation

* `scripts/check-npm-tarballs.sh` requires `skif-cli-<version>.tgz` and platform tarballs named `skif-<platform>-<version>.tgz`.
* `verify_npm` in `.github/workflows/publish.yaml` runs the published `skif-cli` under each Node line and Bun before the GitHub release is promoted.
