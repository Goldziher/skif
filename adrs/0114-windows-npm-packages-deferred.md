---
status: accepted
date: 2026-11-11
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0070"]
---

# Windows npm Packages Deferred Until Their Names Exist

## Context and Problem Statement

[ADR 0070](0070-windows-npm-packages.md) adds `skif-win32-x64-msvc` and `skif-win32-arm64-msvc` to the npm
platform packages. npm trusted publishing can only be configured on a package that already exists, so each new
platform package needs a placeholder published by hand before the first release that ships it
(`scripts/check-npm-packages-exist.sh` stops a release whose packages do not all exist). npm's spam filter rejected
the placeholder publishes for both Windows names, and a support request with npm is open. Until npm releases the
names, neither package can be created, no trusted publisher can be configured for it, and a release that lists the
Windows targets fails before it starts.

Should 0.3.0 wait for the Windows npm packages?

## Decision Drivers

* A release publishes nothing unless every package it lists can be published ([ADR 0070](0070-windows-npm-packages.md), "Bad" consequences)
* Windows users already have channels that need no npm package: the release archives, `cargo binstall` and Scoop ([ADR 0013](0013-distribution-and-naming.md))
* The fixes and performance work of 0.3.0 should not wait on a registry support request with no date

## Considered Options

* Ship 0.3.0 without the Windows npm packages
* Hold 0.3.0 until npm releases the names
* Publish the Windows addons under other names

## Decision Outcome

Chosen option: "Ship 0.3.0 without the Windows npm packages", because the other channels already serve Windows and
the rest of the release does not depend on npm's answer.

* **Six platform packages.** `npm-package/package.json` lists the six targets of
  [ADR 0018](0018-npm-distribution-via-napi-rs.md) in `napi.targets`; `x86_64-pc-windows-msvc` and
  `aarch64-pc-windows-msvc` are removed, so napi-rs generates no Windows package and the umbrella's
  `optionalDependencies` list six packages.
* **Release jobs.** `publish.yaml` builds no Windows addon, and the Windows addon load job, the Windows tarball
  install job and the Windows runners of `verify_npm` are removed with the script that only they ran
  (`scripts/npm-install-smoke.sh`). `publish_crates` and `publish_npm` wait for the Linux addon loads only.
* **Windows channels.** The Windows release archives for x64 and arm64, their smoke test on an ARM64 runner,
  `cargo binstall` and the Scoop manifest are unchanged. On Windows, `npx skif-cli`, `pnpm dlx skif-cli` and
  `bunx -p skif-cli skif` fail at import, as they did before [ADR 0070](0070-windows-npm-packages.md); the
  documentation sends Windows users to the archives, `cargo binstall` and Scoop.
* **Restoring the packages.** The Windows packages return in the first release after both names exist on npm and
  each has the repository's trusted publisher configured. That release re-adds the two targets to `napi.targets`
  and restores the Windows build, load, install and verification jobs, `scripts/npm-install-smoke.sh` and the
  Windows cases of the release script tests as [ADR 0070](0070-windows-npm-packages.md) describes them, with a new
  record that supersedes this one.

### Consequences

* Good, because 0.3.0 ships on its own schedule, and its npm packages are the ones `check-npm-packages-exist.sh`
  finds on the registry.
* Good, because the release drops two addon builds and the Windows load, install and verification jobs while the
  packages are absent.
* Bad, because the most common install path for JavaScript users on Windows fails at import until the names exist.
* Bad, because the npm set (six targets) and the archive set (eight targets) differ again.

### Confirmation

* `scripts/check-npm-tarballs.sh` fails unless the packed set is the umbrella plus the six platform packages, and
  the umbrella lists exactly those six as `optionalDependencies`; `scripts/test-release-scripts.sh` covers a set
  that contains a Windows package.
* `scripts/test-npm-published.sh` requires `check-npm-packages-exist.sh` to probe the umbrella and six platform
  packages for `npm-package/package.json`.
* `bash scripts/check-npm-packages-exist.sh` passes before a release is tagged.

## Pros and Cons of the Options

### Ship 0.3.0 without the Windows npm packages

* Good, because nothing else in the release waits on the registry.
* Bad, because Windows npm users wait for a later release.

### Hold 0.3.0 until npm releases the names

* Good, because the npm set covers Windows from 0.3.0 on.
* Bad, because every platform waits for fixes for as long as the support request takes, with no date to plan on.

### Publish the Windows addons under other names

* Good, because Windows npm users get the package in 0.3.0.
* Bad, because napi-rs derives every platform package name from `napi.packageName`, so the Windows packages would
  need their own naming scheme, and the names would change again once the intended ones exist.

## More Information

* Amends: [ADR 0070](0070-windows-npm-packages.md) (eight platform packages, the Windows load, install and
  verification jobs)
* Related decisions: [ADR 0013](0013-distribution-and-naming.md),
  [ADR 0018](0018-npm-distribution-via-napi-rs.md),
  [ADR 0043](0043-floating-tags-for-0x-releases-and-musl-binstall.md),
  [ADR 0047](0047-npm-package-name-skif-cli.md)
* npm trusted publishing: <https://docs.npmjs.com/trusted-publishers>
* See the [ADR index](index.md).
