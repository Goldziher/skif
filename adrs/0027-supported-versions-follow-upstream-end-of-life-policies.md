---
status: accepted
date: 2026-11-04
decision-makers: ["Na'aman Hirschfeld"]
---

# Supported Versions Follow Upstream End-of-Life Policies

## Context and Problem Statement

skif runs on Node.js and Bun (the npm addon), is built with Rust, and analyses workspaces of four package managers. [ADR 0018](0018-npm-distribution-via-napi-rs.md) fixed the Node floor at 24 without a rule for how the floor moves, and no record says which package manager versions skif is tested against. Without a rule, the floor is either raised arbitrarily or never, and every old toolchain becomes an implicit promise.

## Decision Drivers

* A supported version is one that is tested; a promise without a test is not a promise
* Upstream knows best when it stops fixing a version
* Older versions that happen to work should not be blocked
* The rule must also cover upstreams that publish no end-of-life policy

## Considered Options

* Support only versions that upstream still supports; where upstream has no policy, the latest two majors
* Support every version back to a fixed date
* Support only the latest version of everything

## Decision Outcome

Chosen option: "Support only versions that upstream still supports; where upstream has no policy, the latest two majors", because it ties the test matrix to facts someone else maintains and keeps it small.

**Rule.** skif supports a version of a runtime, language or tool it targets only while that version is not past its upstream end-of-life. Where the upstream publishes no end-of-life policy, skif supports the latest two major versions. A version is dropped in the first release after it leaves support, and the drop is recorded under `Changed` in `CHANGELOG.md`. Support means CI tests it. An older version may work, but there is no guarantee: it is not tested, it is not blocked (`engines` fields are advisory and skif does not refuse to run), and a bug that reproduces only on it is not fixed.

**Applied today (2026-11-04).**

| Target | Upstream policy | Supported by skif |
| --- | --- | --- |
| Node.js | [release schedule](https://github.com/nodejs/Release/blob/main/schedule.json): 20 ended 2026-04-30, 22 ends 2027-04-30, 24 ends 2028-04-30, 26 ends 2029-04-30 (25 ended 2026-06-01) | 22, 24 and 26; `engines.node` is `>=22`; odd-numbered lines are never supported |
| Bun | no end-of-life policy ([bun.sh](https://bun.sh/docs)); one major, 1.x | all of 1.x, tested on the latest 1.x release; the second major joins once 2.x exists; the package declares `>=1` |
| Rust | the [xberg-io toolchains](../toolchains.toml) baseline | the MSRV is `rust-version` in `Cargo.toml` (1.99); CI builds on it and on stable; raising it is allowed in any release and is a `Changed` entry |
| npm | no end-of-life policy ([npm docs](https://docs.npmjs.com/policies/) and the [CLI security policy](https://github.com/npm/cli/blob/latest/SECURITY.md) state none) | the latest two majors |
| pnpm | no formal end-of-life policy; its [compatibility table](https://pnpm.io/installation#compatibility) lists 10, 11 and 12 | the latest two majors |
| Yarn | Yarn Classic (1.x) has been in maintenance mode since January 2020 and "will eventually reach end-of-life" ([classic.yarnpkg.com](https://classic.yarnpkg.com/en/docs/install)); no policy for 2+ | the latest two majors of Yarn 2+; Yarn Classic is legacy input: its `workspaces` forms and `yarn.lock` detection stay parsed, but it is not tested and bugs only it shows are not fixed |
| TypeScript and oxc | [ADR 0021](0021-oxc-lockstep-with-poly.md) | oxc lockstep with poly; analysed projects may use any TypeScript the pinned parser understands |

The Node floor in [ADR 0018](0018-npm-distribution-via-napi-rs.md) moves from 24 to 22. The napi addon targets a Node-API version available on 22, and the `verify_npm` job runs on Node 22, 24 and 26 and on the latest Bun.

### Consequences

* Good, because the test matrix follows upstream and shrinks on a schedule.
* Good, because users on a slightly older toolchain are not locked out.
* Bad, because the table needs a refresh at each release; a stale row is a documentation bug.
* Bad, because "latest two majors" for a tool with no policy is skif's judgement, not the upstream's.

### Confirmation

* `npm-package/package.json` declares `engines.node` as `>=22`.
* The `verify_npm` job runs the published package under Node 22, 24 and 26 and under the latest Bun.
* `README.md` and `website/src/content/docs/start/installation.mdx` carry a "Supported versions" section that matches the table.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Support only versions that upstream still supports; where upstream has no policy, the latest two majors

* Good, because the matrix is small and justified by facts.
* Bad, because a user pinned to an end-of-life version gets no fixes.

### Support every version back to a fixed date

* Good, because it is simple to state.
* Bad, because it commits skif to testing toolchains nobody maintains.

### Support only the latest version of everything

* Good, because it is the smallest matrix.
* Bad, because ordinary upgrade lag would make skif unusable for teams on a still-supported Node LTS.

## More Information

* Amends: [ADR 0018](0018-npm-distribution-via-napi-rs.md)
* Related decisions: [ADR 0002](0002-crate-layout-and-publishing.md), [ADR 0005](0005-workspace-discovery.md), [ADR 0013](0013-distribution-and-naming.md), [ADR 0021](0021-oxc-lockstep-with-poly.md)
* See the [ADR index](index.md).
