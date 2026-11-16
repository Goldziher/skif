---
status: accepted
date: 2026-11-16
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0027", "0028"]
---

# Publish the npm Packages Under the polylint Scope

## Context and Problem Statement

[ADR 0027](0027-distribution-and-release.md) publishes skif to npm as the umbrella `skif-cli` with platform packages
named `skif-<platform>`, because npm refused `skif` as too close to existing packages. Two costs followed. The package
name and the command differ, so `npx skif` is not this tool and Bun needs `bunx -p skif-cli skif`. And npm blocked
`skif-win32-x64-msvc` and `skif-win32-arm64-msvc` the same way, so Windows has no npm package, `npx skif-cli` on
Windows fails at import, and Windows users are sent to archives, `cargo binstall` and Scoop.

The `@polylint` organization on npm exists, and polylint already publishes its packages there. Where should skif's npm
packages live?

## Decision Drivers

* `npx` and `bunx` run this tool when given its package name
* Windows gets the same in-process addon as every other platform
* npm's name-similarity check does not apply to the names
* The `skif` command, the napi binary name and the crate name stay as they are
* Trusted publishing (OIDC) remains the only way CI publishes

## Considered Options

* Keep `skif-cli` and `skif-<platform>` (Windows stays off npm)
* A personal scope, `@goldziher/skif`
* The `@polylint` organization, `@polylint/skif`
* A launcher package that spawns the CLI executable on Windows

## Decision Outcome

Chosen option: "The `@polylint` organization, `@polylint/skif`", because a scope sidesteps the similarity check, makes
the package name the product name, and lets the Windows packages exist under an organization that owns the product and
can share ownership.

* The umbrella is `@polylint/skif`. Platform packages are `@polylint/skif-<napi platform>`, derived from
  `napi.packageName = "@polylint/skif"`. The `skif` bin, the napi `binaryName` (so `skif.<platform>.node`) and the
  crate `skif-cli` do not change.
* `npx @polylint/skif` and `bunx @polylint/skif` run this tool. The MCP server of
  [ADR 0028](0028-editors-agents-and-git-hooks.md) is declared as `npx -y @polylint/skif@<version> mcp`, and the
  poly hook's `npm` channel is `pnpm dlx @polylint/skif@X.Y.Z`; both remain release surfaces.
* Windows returns to npm. `napi.targets` gains `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`, with the MSVC
  runtime linked statically. The arm64 addon is built and load-tested on an arm64 Windows runner, and the release
  verifies the npm install on Windows. The umbrella and the eight platform packages are nine packages.
* Bootstrap: trusted publishing can only be configured on a package that exists, and the trusted publisher is bound to
  `publish.yaml`. Release 0.6.0 is therefore published once under the new names by the maintainer, from the artifacts
  of the v0.6.0 release run plus Windows addons built from the v0.6.0 tag. Trusted publishers are then configured on
  all nine packages, and CI publishes from the next release. Platform packages publish before the umbrella.
* `skif-cli` and `skif-<platform>` are deprecated on npm with a pointer to `@polylint/skif`. No further versions are
  published under them.

This supersedes the parts of ADR 0027 that name the npm packages and keep Windows off npm (the umbrella name, the bun
form, the platform package names and the Windows paragraph of "The npm package", and the matching Consequences), and
amends the npm surfaces of ADR 0028. The rest of both records holds. The launcher package that ADR 0027 rejected stays
rejected.

### Consequences

* Good, because the npm name is the product name and `npx @polylint/skif` is this tool.
* Good, because Windows users install the in-process addon from npm like everyone else.
* Good, because the packages sit under the organization that owns the product.
* Bad, because users of `skif-cli` must reinstall under the new name; the old packages stop receiving releases.
* Bad, because 0.6.0 needs one manual publish of nine packages before CI can take over, and the Windows addons for it
  are built outside the release run.
* Bad, because the Windows addon adds an arm64 runner to the release and its verification.
* Neutral, because the crate, the binary, the archives, Homebrew and Scoop are unchanged.

### Confirmation

* `scripts/check-npm-packages-exist.sh` stops a release unless all nine packages exist on the registry.
* `scripts/check-npm-tarballs.sh` accepts exactly the umbrella and the eight platform tarballs.
* The `verify_npm` jobs install the published packages on Linux, macOS and Windows, including arm64 Windows, and run
  `scripts/npm-smoke.sh`.

## Pros and Cons of the Options

### Keep `skif-cli` and `skif-<platform>`

* Good, because nothing changes for current users.
* Bad, because `npx skif` stays a different tool and Windows stays off npm.

### A personal scope, `@goldziher/skif`

* Good, because it avoids the similarity check.
* Bad, because the scope names a person, not the product, and polylint already left it for `@polylint`.

### The `@polylint` organization, `@polylint/skif`

* Good, because it avoids the similarity check, names the product and shares ownership.
* Bad, because existing users migrate and the first publish is manual.

### A launcher package that spawns the CLI executable on Windows

* Good, because it needs no new platform packages.
* Bad, because it breaks the in-process API on Windows, as ADR 0027 found.
