---
status: accepted
date: 2026-11-02
decision-makers: ["Na'aman Hirschfeld"]
---

# npm Distribution via napi-rs

## Context and Problem Statement

[ADR 0013](0013-distribution-and-naming.md) ships skif on npm as an umbrella package with a small Node launcher that spawns a prebuilt
binary from one of eight per-platform packages (including `win32-x64` and `win32-arm64`). That model costs a child
process per invocation, keeps a hand-maintained set of platform manifests in `npm-package/platforms/`, and has no
in-process entry point for Node tools that want to call skif as a library. The sibling projects that publish
native code to npm build a Node-API addon with napi-rs, which generates the platform packages and the umbrella's
`optionalDependencies` from the build matrix.

## Decision Drivers

* JavaScript users install skif like any dev dependency, with no install script and no download at first run
* The package must be usable from Node and from Bun, and from ES modules without a CommonJS shim
* Platform packages and the umbrella's `optionalDependencies` must come from one build definition, not from hand-edited manifests
* The release matrix should not carry targets whose users have other install channels

## Considered Options

* A napi-rs addon with generated platform packages, six targets, Node 24+ and Bun, ESM only
* Keep the launcher and eight hand-maintained platform packages
* A downloader that fetches the binary on first run

## Decision Outcome

Chosen option: "A napi-rs addon with generated platform packages, six targets, Node 24+ and Bun, ESM only", because it keeps the offline, no-install-script model of [ADR 0013](0013-distribution-and-naming.md) and replaces its hand-maintained parts with generated ones.

**The npm package `skif` is a napi-rs package.** It exposes the CLI entry point (`bin`) and a programmatic API
through a Node-API addon built from the Rust library ([ADR 0003](0003-library-first-embedding.md)). The addon is loaded in-process; the
launcher that spawns a binary is retired.

**Runtime support.** Node 24 or newer and Bun. The package is ESM only (`"type": "module"`); there is no CommonJS
entry point. `engines` declares the Node floor, and Bun is exercised in the verification job.

**Six platform packages, no Windows.** The npm targets are `aarch64-apple-darwin`, `x86_64-apple-darwin`,
`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-musl` and
`aarch64-unknown-linux-musl`, published as `skif-darwin-arm64`, `skif-darwin-x64`, `skif-linux-x64-gnu`,
`skif-linux-arm64-gnu`, `skif-linux-x64-musl` and `skif-linux-arm64-musl`. There are no Windows npm packages.
Windows users install from the GitHub release archive, Scoop or cargo.

**Generated, not hand-written.** napi-rs generates the platform packages and writes them, with matching `os`,
`cpu` and `libc` fields, into the umbrella's `optionalDependencies`. The version of every generated package is the
workspace version ([ADR 0013](0013-distribution-and-naming.md), [ADR 0017](0017-release-policy-a-single-complete-0-1-0.md)); `scripts/release-bump.sh` and `scripts/check-versions.sh` cover the
umbrella manifest and the generated set.

**Unchanged for now.** The GitHub release still attaches the eight `skif-<target>` archives (Windows included) and
`checksums.txt`; cargo-binstall, Homebrew and Scoop consume those archives as before, and the Action downloads
them. The git hooks' npm channel ([ADR 0014](0014-git-hook-integration.md)) keeps running `pnpm dlx skif@<version>`.

**Release profile.** The CLI binaries keep `panic = "abort"`. The addon is built with an unwinding profile so a
panic becomes a JavaScript exception instead of ending the host process, and its allocator choice is independent of the CLI's.

### Consequences

* Good, because platform manifests and the `optionalDependencies` list cannot drift from the build matrix.
* Good, because Node and Bun callers can use skif in-process without spawning a binary.
* Good, because installs stay offline and script-free, as in [ADR 0013](0013-distribution-and-naming.md).
* Bad, because Windows has no npm package; Windows users who want `pnpm add -D skif` cannot have it.
* Bad, because the addon needs its own build profile and per-target build jobs, and a Node-API ABI floor (Node 24+) excludes older Node versions.
* Bad, because the npm tree and the archive set differ (six versus eight targets), so the release verification has to check both lists.

### Confirmation

* The `publish.yaml` dry run builds the six addon targets with napi-rs and fails when the generated package names differ from the six listed above.
* `scripts/check-versions.sh` fails when the umbrella version, any generated platform package version or any `optionalDependencies` pin disagrees with the workspace version.
* The `verify_npm` job installs the published umbrella on a clean runner and runs `skif --version` under Node 24 and under Bun; it must print the released version.
* `npm-package/package.json` declares `"type": "module"`, an `engines.node` floor of 24 and no `install` or `postinstall` script.

## Pros and Cons of the Options

### A napi-rs addon with generated platform packages, six targets, Node 24+ and Bun, ESM only

* Good, because platform packages are generated, the API is available in-process and installs need no script.
* Bad, because there is no Windows npm package and the addon needs a separate unwinding build profile.

### Keep the launcher and eight hand-maintained platform packages

* Good, because it exists and covers Windows.
* Bad, because the platform manifests are maintained by hand, every call spawns a process and there is no library entry point.

### A downloader that fetches the binary on first run

* Good, because one small package would serve every platform.
* Bad, because it needs network access at first run, which [ADR 0013](0013-distribution-and-naming.md) rules out.

## More Information

* Amends: [ADR 0013](0013-distribution-and-naming.md) (the npm channel, the npm target count and the platform package names)
* Related decisions: [ADR 0003](0003-library-first-embedding.md), [ADR 0014](0014-git-hook-integration.md), [ADR 0017](0017-release-policy-a-single-complete-0-1-0.md)
* See the [ADR index](index.md).
