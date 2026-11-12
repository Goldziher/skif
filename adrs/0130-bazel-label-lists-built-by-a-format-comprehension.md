---
status: accepted
date: 2026-11-12
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0115"]
---

# Bazel Label Lists Built by a Format Comprehension

## Context and Problem Statement

[ADR 0115](0115-bazel-build-files-and-output-trees-in-the-js-analysis.md) reads Bazel `BUILD` files statically: Starlark
is never evaluated, and a string literal that is a `node_modules/<name>` label names `<name>` as a dependency of the
label's package. rules_js repositories often build the label list with a comprehension instead,
`[":node_modules/%s" % m for m in ["vite", "typescript"]]`. The template literal then read as the dependency `%s`, and
a file pattern such as `"node_modules/**"` in `glob(exclude = [...])` read as the dependency `**`. Both were reported
as unlisted, and the packages the list really names were not credited.

Should skif read the labels a comprehension builds, and which strings are no labels at all?

## Decision Drivers

* A `BUILD` file in a common rules_js shape gives the findings its literal form gives
* A string that is a pattern or a template never names a dependency
* Starlark stays unevaluated beyond shapes whose value is fixed by their text

## Considered Options

* Expand the one comprehension shape whose value is fixed by its text, and skip other templates and every `glob`
  pattern
* Skip templates and `glob` patterns only
* Evaluate Starlark

## Decision Outcome

Chosen option: "Expand the one comprehension shape whose value is fixed by its text, and skip other templates and every
`glob` pattern", because that shape is the common one, its value needs no evaluation beyond substituting literals, and
everything else stays read as ADR 0115 reads it.

* `"<template>" % v for v in [<string literals>]`, with exactly one `%s` in the template (`%%` allowed), the same loop
  variable and no `if` clause, yields one label per list item, located at the item. Any other comprehension yields
  nothing.
* A string followed by `%` or `.format` is a template and names nothing.
* The strings inside a `glob(...)` call, include and exclude patterns alike, are file patterns and name nothing.
* A label names a dependency only when its name is a valid npm package name.
* A dependency a `BUILD` file names is reported, and explained by `explain-usage --dependency`, at the file and line
  of the literal that names it.

### Consequences

* Good, because the comprehension form credits the packages it lists, and the `%s` and `**` findings are gone.
* Good, because an unlisted dependency named by a `BUILD` file points at the line that names it.
* Bad, because a list held in a variable, a filtered comprehension or a macro still names nothing.

### Confirmation

* `crates/skif-js-plugins/src/bazel.rs`: `label_literals_expand_formatted_lists_and_skip_globs_and_templates`.
* `crates/skif-js-plugins/tests/plugins.rs`:
  `bazel_format_comprehensions_name_their_packages_and_glob_patterns_none`.
* `crates/skif-js/tests/explain_dependency.rs`:
  `a_bazel_format_comprehension_names_its_packages_and_glob_patterns_name_none` and
  `an_unlisted_config_reference_is_explained_with_its_file_and_line`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Expand the one comprehension shape whose value is fixed by its text, and skip other templates and every `glob` pattern

* Good, because the common rules_js shape is read in full.
* Bad, because the reader knows one Starlark expression beyond literals.

### Skip templates and `glob` patterns only

* Good, because the reader stays literal-only.
* Bad, because a package listed through a comprehension is reported unused.

### Evaluate Starlark

* Good, because variables, filters and macros are read too.
* Bad, because it needs a Starlark interpreter and the loaded `.bzl` files of other repositories.
