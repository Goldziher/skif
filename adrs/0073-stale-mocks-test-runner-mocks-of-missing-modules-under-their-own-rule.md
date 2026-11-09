---
status: accepted
date: 2026-11-09
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0034"]
---

# Stale Mocks: Test-Runner Mocks of Missing Modules Under Their Own Rule

## Context and Problem Statement

`skif-js-syntax` reads `vi.mock`, `vi.doMock`, `vi.unmock`, `vi.importActual`, `jest.mock`, `jest.doMock`,
`jest.requireActual` and the other test-runner calls that name a module from the syntax tree, and records them as mock
imports (GitHub issue #16). A mock keeps a local file reachable, a listed package it names counts as used, and a
package it names that no `package.json` lists is not an unlisted dependency. The specifier of a mock is resolved like
any other import, so a relative or aliased mock of a file that does not exist is reported by `unresolved-imports`, an
error by default ([ADR 0020](0020-js-rule-set-severities-addon-profile-and-windows-support.md)):

```ts
// src/a.test.ts
import { expect, test, vi } from 'vitest';
import { a } from './a';
vi.mock('./gone', () => ({ b: 2 }));
test('a', () => expect(a).toBe(1));
```

knip does not read mock specifiers, so a project migrated from knip gets these as new errors (GitHub issue #111). They
are worth reporting: such a mock is left behind when the module it named was moved or deleted, and the test runner
does not notice, because the factory replaces the module. But they are not unresolved imports: no code loads the
module, and the test passes.

How should skif report a mock whose module does not exist?

## Decision Drivers

* A finding knip does not have must be possible to triage, baseline or turn off apart from the findings knip has
* A default must not fail a build over code that runs
* A mock and an import must resolve the same way, so that they cannot disagree about whether a file exists
* Rule ids are a public contract ([ADR 0007](0007-rule-and-diagnostic-model.md))

## Considered Options

* A `stale-mocks` rule, a warning by default, that takes the mocks out of `unresolved-imports`
* Keep the mocks in `unresolved-imports`
* Stop reporting mocks of missing modules

## Decision Outcome

Chosen option: "A `stale-mocks` rule, a warning by default, that takes the mocks out of `unresolved-imports`", because it keeps the findings and gives them the id and the severity of what they are.

**The rule.** `stale-mocks` is a `js` rule. It reports a mock import whose relative, aliased (`tsconfig` `paths`,
bundler aliases, `@/...`) or `#` specifier resolves to nothing, with the message
"`` `vi.mock('./gone')` mocks a module that does not exist ``", naming the call as written. `unresolved-imports` no
longer reports mock imports. The specifier is resolved exactly as an import is, so the query of a specifier
(`./icon.svg?react`, `?url`, `?raw`) is dropped before the file is looked up, and the exemptions of
[ADR 0035](0035-js-evaluation-parity-unreadable-configs-unresolved-specifiers-and-test-entries.md) apply to both
rules alike: a relative specifier of a file a `.gitignore` lists, or of a JSON or foreign file type without a query, is
not reported. A `{ virtual: true }` mock is still not recorded, and fixture directories and ignored files are skipped
as for `unresolved-imports`.

**Package names.** A bare specifier that resolves to nothing (`vi.mock('not-installed')`) is taken for a dependency, as
an import is, and the dependency rules judge it: a mock of a listed package uses it, and a mock of an unlisted one is
not reported (#16). `stale-mocks` does not report it either: a package name that resolves to nothing is the dependency
rules' concern, and they already decided that a mock does not make a package unlisted.

**Severity.** The default is `Warning`. ADR 0020 makes errors of the findings that break a build or an install
(`unused-dependencies`, `unlisted-dependencies`, `unresolved-imports`); a stale mock breaks neither, since the test
runs. It is dead test code, like the configuration `stale-js-config` reports, which is a warning too.
`[rules] stale-mocks = "error"` and `--fail-on-warnings` make it fail a run.

**Ignore lists.** `ignore-unresolved` and `ignore-dependencies` silence `stale-mocks` for a matching specifier, as they
silence `unresolved-imports`, so an entry written for a mock before this change keeps working. `stale-js-config`
therefore judges `ignore-unresolved` only when both rules run, and the dependency lists only when `stale-mocks` runs
too.

**Migration.** knip has no counterpart, so the rule is a `none` row of the migration's tool table: the `exact`
profile turns it off and lists it among the checks the old tools did not run; `full` leaves it on.

**The parse model.** A mock import records its call as written (`Import::call`, for example `vi.mock`), so the
message can name it. The serialized parse output changes, so the module cache schema
([ADR 0066](0066-a-module-cache-and-unsaved-buffers-in-the-language-server.md)) is raised and an older cache is
discarded.

### Consequences

* Good, because a project migrated from knip can baseline or turn off stale mocks without hiding unresolved imports.
* Good, because the findings stay: a mock of a module that was moved or deleted is still reported.
* Good, because `unresolved-imports` means what knip's `unresolved` means for test files.
* Bad, because a stale mock no longer fails a run by default, where it failed as an unresolved import before.
* Bad, because one more rule id is public and has to stay.
* Bad, because a mock of a missing `./icon.svg` is not reported while `./icon.svg?react` is, as for imports: the
  foreign-file exemption of ADR 0035 reads the extension of the specifier as written.

### Confirmation

* `skif-js-syntax` test `mock_imports_record_the_call_as_written`.
* `skif-js` tests `stale_mocks_snapshot`, `a_mock_of_a_missing_module_is_a_stale_mock_not_an_unresolved_import`,
  `ignore_unresolved_silences_a_stale_mock` and
  `stale_mocks_can_be_turned_off_without_bringing_the_mocks_back_as_unresolved_imports` on the `stale-mocks` fixture:
  `vi` and `jest` calls, relative and aliased specifiers, a `?react` specifier of an existing and of a missing file, an
  existing mock target, a virtual mock and a package.
* `skif-migrate` golden tests: the `exact` profile writes `stale-mocks = "off"`.
* `scripts/check-rule-ids.sh` passes with `stale-mocks`; `adrlint lint adrs`, `adrlint format --check adrs` and
  `adrlint lock --check` pass.

## Pros and Cons of the Options

### A `stale-mocks` rule, a warning by default, that takes the mocks out of `unresolved-imports`

* Good, because the id and the severity say what the finding is.
* Bad, because the rule list grows, and the corpus baselines move where a repository has stale mocks.

### Keep the mocks in `unresolved-imports`

* Good, because nothing changes.
* Bad, because a project migrated from knip cannot tell stale mocks from broken imports, or silence one without the
  other.

### Stop reporting mocks of missing modules

* Good, because it matches knip.
* Bad, because dead mocks, which the test runner does not notice, would go unreported.

## More Information

* Amends: [ADR 0034](0034-js-configuration-parity-with-knip.md) (`ignore-unresolved` also silences `stale-mocks`)
* Related decisions: [ADR 0007](0007-rule-and-diagnostic-model.md),
  [ADR 0020](0020-js-rule-set-severities-addon-profile-and-windows-support.md),
  [ADR 0035](0035-js-evaluation-parity-unreadable-configs-unresolved-specifiers-and-test-entries.md),
  [ADR 0045](0045-migrate-tool-rule-tables-warn-for-config-hints-and-syncpack-override-reading.md),
  [ADR 0066](0066-a-module-cache-and-unsaved-buffers-in-the-language-server.md)
* See the [ADR index](index.md).
