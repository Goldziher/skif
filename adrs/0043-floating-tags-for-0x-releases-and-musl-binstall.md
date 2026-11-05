---
status: accepted
date: 2026-11-05
decision-makers: ["Na'aman Hirschfeld"]
---

# Floating Tags for 0.x Releases, musl for cargo binstall and No Windows npm Addon

## Context and Problem Statement

[ADR 0013](0013-distribution-and-naming.md) moves a floating major tag on each release. For a 0.x release that tag is `v0`, and a breaking 0.2.0 would move the `v0` that users pinned for 0.1.x. `cargo binstall` selects an archive by target triple and cannot see the host's glibc, so a glibc older than 2.34 receives the `-gnu` archive and fails to start. The absence of a Windows npm addon, set by [ADR 0018](0018-npm-distribution-via-napi-rs.md), is still undocumented where `npx skif` fails.

## Considered Options

* Move `v0` for every 0.x release, and leave binstall on the host triple (the state before this decision)
* Float `v0.M` for 0.x, redirect the Linux gnu triples of binstall to the musl archives, document the missing Windows addon
* Publish a second, glibc-2.17 archive for binstall users

## Decision Outcome

Chosen option: "Float `v0.M` for 0.x, redirect the Linux gnu triples of binstall to the musl archives, document the missing Windows addon", because it keeps breaking 0.x minors away from pinned users and makes the default install work on every Linux without a new build artefact.

* **Floating tags.** A stable release (1.0.0 onward) moves `vN`. A 0.x release moves `v0.M` and never moves `v0`, because a 0.x minor may break. The Action resolves `vN` and `vX.Y` to the newest matching full release; the README and the guides use `@v0.1`.
* **cargo binstall.** The `skif-cli` binstall metadata redirects the `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` targets to the static musl archives, which run on any Linux. binstall has no glibc probe and no fallback strategy for a failing archive, so the redirect is the only way to make the default work everywhere; the glibc archive stays available from the release page.
* **glibc floor.** `SKIF_GLIBC_FLOOR` in `scripts/lib/libc.sh` is the single definition; the Homebrew formula takes it from there and `scripts/check-versions.sh` checks the build matrix in `publish.yaml` against it.
* **Windows npm.** There is no Windows addon and none is planned; `npx skif` on Windows fails at import. The documentation says so and points to the release archive, Scoop, Cargo and `cargo binstall`.

### Consequences

* Good, because users pinned to `@v0.1` take patches only, and `cargo binstall` works on every Linux.
* Bad, because binstall users on a new glibc get the musl build; they can download the glibc archive by hand.
* Bad, because the existing `v0` and `v1` wording in ADR 0013 no longer describes 0.x releases.
