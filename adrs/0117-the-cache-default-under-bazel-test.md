---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0066"]
---

# The `--cache` Default Under bazel test

## Context and Problem Statement

[ADR 0066](0066-a-module-cache-and-unsaved-buffers-in-the-language-server.md) has `skif --cache` without
`--cache-location` keep its store in `node_modules/.cache/skif/` below the directory the report's paths are relative
to. Under `bazel test` that directory is the test's runfiles tree, inside Bazel's output tree: with skif wrapped in a
`native_test` and the sources as `data`, `bazel test //:skif_test --test_arg=--cache` wrote
`bazel-bin/skif_test_bin.runfiles/_main/node_modules/.cache/skif/modules.bin` (GitHub issue #177). A test must not
write there; Bazel gives each test a private scratch directory in `TEST_TMPDIR` instead.

Where should `--cache` keep its store under `bazel test` when no location is given?

## Decision Drivers

* Nothing is written into Bazel's output tree
* `--cache-location` keeps its meaning everywhere
* Outside `bazel test` nothing changes
* The environment is read only by the CLI crate ([ADR 0003](0003-library-first-embedding.md))

## Considered Options

* Default to `$TEST_TMPDIR/skif/` when `TEST_TMPDIR` is set
* Turn the persistent cache off when `TEST_TMPDIR` is set
* Document passing `--cache-location`

## Decision Outcome

Chosen option: "Default to `$TEST_TMPDIR/skif/` when `TEST_TMPDIR` is set", because it writes only where Bazel
allows a test to write and keeps `--cache` working within one test run, with no flag to remember.

When `--cache` is given without `--cache-location` and `TEST_TMPDIR` is set and not empty, the store is
`modules.bin` in `$TEST_TMPDIR/skif/`; a relative value is taken against the working directory, as a relative
`--cache-location` is. An explicit `--cache-location` wins. Without `TEST_TMPDIR` the default of ADR 0066 holds.
`skif-cli` reads the variable (`env::bazel_test_tmpdir`); the library never does.

### Consequences

* Good, because `bazel test` with `--cache` leaves the output tree as the build made it.
* Good, because watch mode, which shares the `--cache` store, follows the same default.
* Neutral, because `TEST_TMPDIR` is cleared between test runs, so the store saves parsing only within one run.
* Bad, because another tool that sets `TEST_TMPDIR` outside Bazel moves the default store too.

### Confirmation

* `crates/skif-cli/src/env.rs`: `an_empty_test_tmpdir_is_unset`.
* `crates/skif-cli/tests/cache.rs`: `under_bazel_test_the_default_store_is_in_the_test_tmpdir`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Default to `$TEST_TMPDIR/skif/` when `TEST_TMPDIR` is set

* Good, because the cache stays usable and the output tree is untouched.
* Bad, because the default now depends on an environment variable.

### Turn the persistent cache off when `TEST_TMPDIR` is set

* Good, because nothing is written at all.
* Bad, because `--cache` would silently mean nothing, and a run with `--cache-location` would differ in kind.

### Document passing `--cache-location`

* Good, because nothing changes.
* Bad, because the default keeps writing into the output tree for everyone who misses the note.

## More Information

* Amends: [ADR 0066](0066-a-module-cache-and-unsaved-buffers-in-the-language-server.md) (the "`--cache` and
  `--cache-location`" section)
* Issue: [#177](https://github.com/Goldziher/skif/issues/177)
* Bazel test environment: <https://bazel.build/reference/test-encyclopedia#initial-conditions>
* See the [ADR index](index.md).
