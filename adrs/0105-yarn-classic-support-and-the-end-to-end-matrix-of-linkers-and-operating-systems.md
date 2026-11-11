---
status: accepted
date: 2026-11-11
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0027"]
---

# Yarn Classic Support and the End-to-End Matrix of Linkers and Operating Systems

## Context and Problem Statement

[ADR 0027](0027-supported-versions-follow-upstream-end-of-life-policies.md) defines support as "CI tests it" and lists
Yarn Classic (1.x) as legacy input: its `workspaces` forms and `yarn.lock` are parsed, its install command is chosen
without Berry's flags and its hoisting decides whether member binaries are provided
([ADR 0097](0097-commands-in-other-directories-and-binaries-of-workspace-members.md)), yet no test installs a Classic
workspace. Classic is still the version many repositories run, and upstream has not ended it.

The end-to-end job installed each fixture once per manager, on Linux only, with the manager's default layout. skif's
view depends on the layout: it reads installed packages from `node_modules` when there is one, from the lockfile under
Plug'n'Play, and decides member binaries from the linker. A linker or an operating system that no fixture installs is
a promise without a test. The fixtures also pinned `packageManager` to fixed versions, so pnpm and corepack ran those
versions rather than the previous majors the matrix named.

## Decision Drivers

* A supported version or layout is one that CI installs and lints
* The matrix must stay bounded: one job per manager, version slot and operating system
* Scenarios are shared across managers; only the manager-specific syntax differs
* A difference between linkers in skif's findings is asserted where it occurs, not hidden

## Considered Options

* Test Yarn Classic and every linker the managers ship, the newest slot on Linux, macOS and Windows
* Keep Yarn Classic untested and the matrix on default layouts and Linux

## Decision Outcome

Chosen option: "Test Yarn Classic and every linker the managers ship, the newest slot on Linux, macOS and Windows",
because the code paths for Classic, the non-default linkers and the other operating systems exist and are otherwise
unverified.

**Yarn Classic.** The row of ADR 0027 changes: skif supports the newest Yarn Classic 1.x release and the latest two
majors of Yarn 2+. Classic has one major and upstream maintains it, so the "latest two majors" rule does not apply to it;
its newest 1.x is tested, and a bug only Classic shows is fixed. It drops out of support when upstream announces its
end of life, under the rule of ADR 0027.

**Matrix.** The end-to-end job runs once per manager and slot: npm, pnpm and Yarn 2+ on the newest and the previous
major, Yarn Classic and Bun on their newest 1.x (`scripts/e2e/resolve-pm-version.sh` reads the registry). The newest
slot of every manager runs on Linux, macOS and Windows; the previous majors run on Linux. Each job runs every scenario
under every linker of its manager:

| Manager | Linkers |
| --- | --- |
| npm | hoisted |
| pnpm | isolated, `nodeLinker: hoisted` |
| Yarn 2+ | Plug'n'Play (no `.yarnrc.yml`), `nodeLinker: node-modules`, `nodeLinker: pnpm` |
| Yarn Classic | hoisted |
| Bun | no `bunfig.toml` (isolated for a new workspace, through `bun.lock`'s `configVersion`), `linker = "hoisted"`, `linker = "isolated"` |

npm's `install-strategy=linked` is not in the matrix: skif makes no claim about it.

**Scenarios.** A scenario under `fixtures/e2e/<name>` holds the files shared by every manager, overlays for the
managers that need different files, and an expectation with overrides per manager and per linker. The runner writes
the manager's own forms: `pnpm-workspace.yaml`, the catalog (pnpm, Bun, and Yarn from 4.10), the linker setting, and
`packageManager` set to the version under test, which every root must report back. Each install is checked for the
layout its linker produces, so a scenario cannot pass on the wrong one. A difference in the findings between linkers is
an override that states its reason; where the install provides a member's binary that skif still reports, the override
records that limitation of ADR 0097.

### Consequences

* Good, because Classic, every shipped linker, macOS and Windows are installed and linted on every change.
* Good, because the previous-major slots test the version they name.
* Good, because a scenario is written once and generated per manager.
* Bad, because the job count grows from seven to eighteen, plus one build per operating system.
* Bad, because a finding Classic reports differently from npm becomes a bug to fix rather than a known gap.

### Confirmation

* `.github/workflows/ci.yml`: the `e2e` matrix lists the five managers, the two slots and the three operating systems,
  with the exclusions above.
* `scripts/test-e2e-scripts.sh` checks the Classic slot of `resolve-pm-version.sh`, the generated projects of every
  manager and linker, and that every scenario's expectation evaluates.
* `README.md` and `website/src/content/docs/start/installation.mdx` name the newest Yarn Classic 1.x as supported.

## Pros and Cons of the Options

### Test Yarn Classic and every linker the managers ship, the newest slot on Linux, macOS and Windows

* Good, because each supported layout has a test that installs it.
* Bad, because CI uses more runner time, most of it on macOS and Windows.

### Keep Yarn Classic untested and the matrix on default layouts and Linux

* Good, because the matrix stays at seven Linux jobs.
* Bad, because the Classic and linker code paths and the Windows install path stay unverified.

## More Information

* Yarn Classic: <https://classic.yarnpkg.com/en/docs/install>
* Yarn `nodeLinker`: <https://yarnpkg.com/configuration/yarnrc#nodeLinker>
* Yarn catalogs: <https://yarnpkg.com/features/catalogs>
* pnpm `nodeLinker`: <https://pnpm.io/settings#nodelinker>
* Bun linker: <https://bun.com/docs/install/isolated>
