---
status: accepted
date: 2026-11-09
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0018", "0020", "0043"]
---

# Windows npm Packages

## Context and Problem Statement

[ADR 0018](0018-npm-distribution-via-napi-rs.md) publishes the napi-rs addon as six platform packages and no
Windows package. [ADR 0020](0020-js-rule-set-severities-addon-profile-and-windows-support.md) restates that only the
npm packages omit Windows, and [ADR 0043](0043-floating-tags-for-0x-releases-and-musl-binstall.md) records that
none is planned and that `npx skif` fails on Windows at import. The release already builds and smoke-tests
`x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc` binaries, and CI runs the test suite on Windows, so the gap
is only the addon packaging. For most JavaScript users npm is the install channel, and `npx skif-cli`,
`pnpm dlx skif-cli` and `bunx -p skif-cli skif` are the first thing a Windows user tries (issue
[#93](https://github.com/Goldziher/skif/issues/93)).

Should the npm package cover Windows?

## Decision Drivers

* JavaScript users on Windows install skif the way they install any other dev dependency
* The npm channel stays offline and script-free ([ADR 0013](0013-distribution-and-naming.md), [ADR 0018](0018-npm-distribution-via-napi-rs.md))
* A target is published only after it has been loaded on its own architecture under every supported runtime ([ADR 0027](0027-supported-versions-follow-upstream-end-of-life-policies.md))

## Considered Options

* Publish Windows addon packages for x64 and arm64
* Keep the six packages and document the other channels
* Publish an x64 Windows package only

## Decision Outcome

Chosen option: "Publish Windows addon packages for x64 and arm64", because the targets the release already ships
as binaries become installable from npm with no new install mechanism.

* **Eight platform packages.** `npm-package/package.json` lists `x86_64-pc-windows-msvc` and
  `aarch64-pc-windows-msvc` in `napi.targets` next to the six targets of [ADR 0018](0018-npm-distribution-via-napi-rs.md). napi-rs
  generates `skif-win32-x64-msvc` and `skif-win32-arm64-msvc` with `os: ["win32"]` and the matching `cpu`, and adds
  them to the umbrella's `optionalDependencies`. The umbrella, its bin and the generated loader are unchanged; the
  package managers create the Windows launchers for the bin as for any other package.
* **Build.** Both addons build on the x64 Windows runner, as the release binaries do; the aarch64 addon is
  cross-built. The MSVC runtime is linked statically, so the addon loads without the VC++ redistributable.
* **Load before publish.** The `publish.yaml` dry run loads each Windows addon on its own architecture (an x64 and an
  ARM64 runner) under Node 22, 24 and 26 and the newest Bun 1.x, and runs the `skif` bin through it. Publishing
  waits for those jobs.
* **Verify after publish.** `verify_npm` runs `npm exec skif-cli` and `bunx -p skif-cli skif` on both Windows
  architectures as well as on Linux, under the same runtimes.
* The release archives, `cargo binstall`, Scoop and the Action stay as they are; they remain the Windows channels for
  users who do not install from npm.

### Consequences

* Good, because `npx skif-cli`, `pnpm dlx skif-cli`, `bunx -p skif-cli skif` and `pnpm add -D skif-cli` work on
  Windows x64 and arm64.
* Good, because the npm set and the archive set now cover the same eight targets.
* Bad, because the release runs two more addon builds and sixteen more load and verification jobs, eight of them on
  ARM64 Windows runners.
* Bad, because two more npm packages need a placeholder publish and a trusted publisher before the first release
  that ships them.

### Confirmation

* `scripts/check-npm-tarballs.sh` fails unless the packed set is the umbrella plus the eight platform packages,
  including `skif-win32-x64-msvc` and `skif-win32-arm64-msvc`, each carrying its `.node` addon, and the umbrella lists
  all eight as `optionalDependencies`; `scripts/test-release-scripts.sh` covers a missing Windows package and a
  Windows package without its addon.
* `load_addon_windows` in `publish.yaml` must pass on `windows-latest` and `windows-11-arm` under every runtime before
  `publish_npm` runs.
* `verify_npm` must print the released version on both Windows architectures under every runtime before the release
  is finalized.

## Pros and Cons of the Options

### Publish Windows addon packages for x64 and arm64

* Good, because the npm channel works on every platform the release supports.
* Bad, because the ARM64 addon is cross-built and needs an ARM64 runner to be tested at all.

### Keep the six packages and document the other channels

* Good, because the release matrix stays as it is.
* Bad, because the most common install path for JavaScript users keeps failing on Windows at import.

### Publish an x64 Windows package only

* Good, because it needs no ARM64 runner.
* Bad, because Windows on Arm users, whom [ADR 0013](0013-distribution-and-naming.md) counts among the users, get an import error: a
  native arm64 Node or Bun looks for the arm64 package.

## More Information

* Amends: [ADR 0018](0018-npm-distribution-via-napi-rs.md) (six platform packages, no Windows npm package),
  [ADR 0020](0020-js-rule-set-severities-addon-profile-and-windows-support.md) (only the npm packages omit Windows),
  [ADR 0043](0043-floating-tags-for-0x-releases-and-musl-binstall.md) (no Windows addon is planned)
* Related decisions: [ADR 0013](0013-distribution-and-naming.md),
  [ADR 0027](0027-supported-versions-follow-upstream-end-of-life-policies.md),
  [ADR 0047](0047-npm-package-name-skif-cli.md)
* See the [ADR index](index.md).
