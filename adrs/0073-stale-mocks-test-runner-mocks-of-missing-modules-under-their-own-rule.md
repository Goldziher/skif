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
are worth reporting: such a call is left behind when the module it named was moved or deleted, and most of them fail
the test. `jest.mock` of a path that does not exist throws unless the mock is `{ virtual: true }`;
`jest.requireActual` and `vi.importActual` of such a path throw; `vi.mock` without a factory fails when it builds the
automock from the module. Only a call whose factory stands in for a module nothing loads, as above, passes. But they
are not unresolved imports in knip's sense, and a project migrated from knip cannot tell them apart from broken
imports, or baseline or turn off one without the other.

How should skif report a mock whose module does not exist?

## Decision Drivers

* A finding knip does not have must be possible to triage, baseline or turn off apart from the findings knip has
* A run that fails today must not pass after the change without a configuration that says so
* A mock and an import must resolve the same way, so that they cannot disagree about whether a file exists
* Rule ids are a public contract ([ADR 0007](0007-rule-and-diagnostic-model.md))

## Considered Options

* A `stale-mocks` rule, an error by default, that takes the mocks out of `unresolved-imports`
* A `stale-mocks` rule, a warning by default
* Keep the mocks in `unresolved-imports`
* Stop reporting mocks of missing modules

## Decision Outcome

Chosen option: "A `stale-mocks` rule, an error by default, that takes the mocks out of `unresolved-imports`", because it gives the findings an id of their own without weakening what a run reports.

**The rule.** `stale-mocks` is a `js` rule. It reports a mock import whose relative, aliased (`tsconfig` `paths`,
bundler aliases, `@/...`) or `#` specifier resolves to nothing, with the message
"`` `vi.mock('./gone')` names a module that does not exist ``", naming the call as written, and the help "remove the
call, or point it at the module's new path". The message says "names", not "mocks", because `jest.requireActual`,
`vi.importActual`, `vi.unmock`, `jest.dontMock`, `jest.requireMock` and `jest.createMockFromModule` load or unmock a
module rather than mock it. Vitest's typed form `vi.mock(import('./gone'), factory)` is a mock of `./gone` as well:
its `import()` belongs to the mock and is not recorded as a dynamic import. `unresolved-imports` no longer reports
mock imports. The specifier is resolved exactly as an import is, so the query of a specifier
(`./icon.svg?react`, `?url`, `?raw`) is dropped before the file is looked up, and the exemptions of
[ADR 0035](0035-js-evaluation-parity-unreadable-configs-unresolved-specifiers-and-test-entries.md) apply to both
rules alike: a relative specifier of a file a `.gitignore` lists, or of a JSON or foreign file type without a query, is
not reported. A `{ virtual: true }` mock is still not recorded, and fixture directories and ignored files are skipped
as for `unresolved-imports`.

**Package names.** A bare specifier that resolves to nothing (`vi.mock('not-installed')`) is taken for a dependency, as
an import is, and the dependency rules judge it: a mock of a listed package uses it, and a mock of an unlisted one is
not reported (#16). `stale-mocks` does not report it either: a package name that resolves to nothing is the dependency
rules' concern, and they already decided that a mock does not make a package unlisted.

**Severity.** The default is `Error`. These findings were `unresolved-imports` errors before, and most of them fail
the test they are in (see the context), which is what ADR 0020 makes errors of. A warning would let a run that fails
today pass after an upgrade without any configuration saying so. The separate id is for triage, not for a lower
severity: `[rules] stale-mocks = "warning"` or `"off"`, a baseline, or the `exact` migration profile lower or drop
them on purpose.

**Ignore lists.** `ignore-unresolved` and `ignore-dependencies` silence `stale-mocks` for a matching specifier, as they
silence `unresolved-imports`, so an entry written for a mock before this change keeps working. `stale-js-config`
judges these lists whether or not `stale-mocks` runs: when it is off, `stale-js-config` walks the mocks itself, so an
entry that only silences a mock counts as used. It judges `ignore-unresolved` when `unresolved-imports` runs, as
before.

**Migration.** knip has no counterpart, so the rule is a `none` row of the migration's tool table: the `exact`
profile turns it off and lists it among the checks the old tools did not run; `full` leaves it on.

**The parse model.** A mock import records its call (`Import::call`, a `MockCall` such as `MockCall::ViMock`), so the
message can name it. `MockCall` is a small `Copy` enum that serializes as the call is written (`"vi.mock"`), and it
is `#[non_exhaustive]`, so a runner method can be added later. `Import` itself stays exhaustive, because `skif-js`
builds imports with struct literals; the new public field is a breaking change for a library user that does the
same. The serialized parse output changes, so the module cache schema
([ADR 0066](0066-a-module-cache-and-unsaved-buffers-in-the-language-server.md)) is raised, the cache probe corpus
gains a file of mocks, and an older cache is discarded.

### Consequences

* Good, because a project migrated from knip can baseline or turn off stale mocks without hiding unresolved imports.
* Good, because the findings stay: a mock of a module that was moved or deleted is still reported.
* Good, because `unresolved-imports` means what knip's `unresolved` means for test files.
* Good, because a run that failed on a stale mock still fails by default.
* Bad, because a project migrated from knip with the `full` profile still gets these errors, now under a separate id.
* Bad, because a mock whose factory stands in for a module nothing loads, which passes, is an error too.
* Bad, because one more rule id is public and has to stay.
* Bad, because a mock of a missing `./icon.svg` is not reported while `./icon.svg?react` is, as for imports: the
  foreign-file exemption of ADR 0035 reads the extension of the specifier as written.

### Confirmation

* `skif-js-syntax` tests `mock_imports_record_the_call_as_written`,
  `a_mock_of_an_import_expression_is_a_mock_import_only`, `a_mock_call_serializes_as_written` and
  `the_cache_schema_matches_the_parse_output`.
* `skif-js` tests `stale_mocks_snapshot`, `a_mock_of_a_missing_module_is_a_stale_mock_not_an_unresolved_import`,
  `ignore_unresolved_silences_a_stale_mock`,
  `stale_mocks_can_be_turned_off_without_bringing_the_mocks_back_as_unresolved_imports` and
  `stale_js_config_judges_the_ignore_lists_with_stale_mocks_off` on the `stale-mocks` fixture: `vi` and `jest` calls,
  Vitest's `vi.mock(import(...))`, relative and aliased specifiers, a `?react` specifier of an existing and of a
  missing file, an existing mock target, a virtual mock and a package.
* `skif-migrate` golden tests: the `exact` profile writes `stale-mocks = "off"`.
* `scripts/check-rule-ids.sh` passes with `stale-mocks`; `adrlint lint adrs`, `adrlint format --check adrs` and
  `adrlint lock --check` pass.

## Pros and Cons of the Options

### A `stale-mocks` rule, an error by default, that takes the mocks out of `unresolved-imports`

* Good, because the id says what the finding is, and the severity is what it was.
* Bad, because the rule list grows, and the corpus baselines move where a repository has stale mocks.

### A `stale-mocks` rule, a warning by default

* Good, because a mock whose factory stands in for a module nothing loads does not fail the run.
* Bad, because a run that fails on a stale mock today would pass after an upgrade, though most such calls fail the
  test.

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
