---
status: accepted
date: 2026-11-02
decision-makers: ["Na'aman Hirschfeld"]
---

# Release Policy: A Single Complete 0.1.0

## Context and Problem Statement

Several records describe a staged release: [ADR 0001](0001-mission-and-scope.md) says 0.1.0 ships manifest rules,
policies and distribution while 0.2.0 adds JS analysis and the first plugins, and [ADR 0004](0004-configuration-model.md), [ADR 0010](0010-js-analysis-on-oxc.md),
[ADR 0011](0011-plugin-model.md) and [ADR 0014](0014-git-hook-integration.md) repeat that split ("in 0.1.0", "ships in 0.2.0", "from 0.2.0 on"). The
work is now organised differently: the manifest rules, the version, semver and catalog policies and the reporters
exist, and the remaining phases (JS analysis, plugins, distribution) land on the same line of development.
Publishing a first version that omits one of the three parts of the mission would put a partial tool in front of
users and then change what `skif` does on the next release.

[ADR 0013](0013-distribution-and-naming.md) also says the Action's floating tag is `v1`, but a project at 0.x has no 1.x release to follow, and
`publish.yaml` derives the floating tag from the major version of the release it publishes.

## Decision Drivers

* A user's first experience of skif must be the whole tool the mission describes
* Version numbers must not encode a staging plan that no longer matches the work
* The floating tag documented for the Action must be one the release pipeline actually moves
* Registry names may still be reserved early, because an unclaimed name can be taken by someone else

## Considered Options

* A single complete 0.1.0 with a floating `v0` tag, and placeholder publishes only to reserve names
* Staged releases: 0.1.0 with manifest and policy rules, 0.2.0 with JS analysis
* Pre-release versions (`0.1.0-rc.N`) that grow into 0.1.0

## Decision Outcome

Chosen option: "A single complete 0.1.0 with a floating `v0` tag, and placeholder publishes only to reserve names", because it keeps the first public version equal to the mission and removes version numbers that describe a plan instead of the software.

**Nothing is released until every phase is done.** 0.1.0 is the first release and contains the manifest and
workspace rules, the version, semver and catalog policies, JS analysis, the plugin set, the reporters
(human, JSON, SARIF, GitHub annotations) and every distribution channel of [ADR 0013](0013-distribution-and-naming.md) (as amended by
[ADR 0018](0018-npm-distribution-via-napi-rs.md)). Every statement in earlier records that assigns a capability to "0.2.0", or that
describes 0.1.0 as shipping only part of the mission, is superseded by this record: those capabilities ship in 0.1.0.

**Placeholder publishes are allowed.** A name-reservation upload of an empty or minimal package to crates.io or
npm is not a release and does not change this policy. It is not announced, installed by any channel or tagged on
GitHub, and the first real version of a registry name is still 0.1.0.

**The floating tag follows the major version, so it is `v0` while the version is 0.x.** The release workflow moves
`v<major>` to each stable release, and documentation, `action.yml` and the README name the Action as
`Goldziher/skif@v0`. When 1.0.0 is released the floating tag becomes `v1`, and `v0` stays where it was.

### Consequences

* Good, because the first public version matches the mission and nothing in the changelog or documentation describes work that comes "later".
* Good, because the documented floating tag is the one the pipeline moves.
* Good, because name reservation is possible without committing to a release.
* Bad, because the first release waits for the slowest phase; there is no early feedback from users on the manifest and policy rules.
* Bad, because a floating `v0` tag can move to a release with breaking changes, as any 0.x line may; users who need stability pin a full tag such as `v0.1.0`.

### Confirmation

* `rg "0\.2\.0"` over `README.md`, `CHANGELOG.md`, `CONTRIBUTING.md`, `action.yml`, `npm-package/` and `tools/` finds no statement that assigns work to a later release.
* `README.md`, `action.yml` and this record name the Action as `Goldziher/skif@v0`, and `update_major_tag` in `.github/workflows/publish.yaml` computes the tag as `v` plus the release's major version.
* `scripts/check-versions.sh` (the `consistency` job) fails when two release surfaces disagree on the version, so a release is one version everywhere.

## Pros and Cons of the Options

### A single complete 0.1.0 with a floating `v0` tag, and placeholder publishes only to reserve names

* Good, because users meet the whole tool at once and the version number says nothing about a plan.
* Bad, because the first release is later than a staged one would be.

### Staged releases: 0.1.0 with manifest and policy rules, 0.2.0 with JS analysis

* Good, because the manifest and policy rules would reach users earlier.
* Bad, because the first version would be a partial tool, and the behaviour of a bare `skif` would change when analysis arrives.

### Pre-release versions (`0.1.0-rc.N`) that grow into 0.1.0

* Good, because early users could try the tool before it is complete.
* Bad, because every channel would carry a pre-release that the floating tag and Homebrew and Scoop deliberately skip, so early users would get a second-class install path.

## More Information

* Amends: [ADR 0001](0001-mission-and-scope.md), [ADR 0004](0004-configuration-model.md), [ADR 0010](0010-js-analysis-on-oxc.md), [ADR 0011](0011-plugin-model.md), [ADR 0013](0013-distribution-and-naming.md), [ADR 0014](0014-git-hook-integration.md)
* See the [ADR index](index.md).
