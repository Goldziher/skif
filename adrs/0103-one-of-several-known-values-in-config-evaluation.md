---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0065"]
---

# One of Several Known Values in Config Evaluation, and the Scopes of Computed Reporters and Storybook Addons

## Context and Problem Statement

The static evaluator of [ADR 0010](0010-js-analysis-on-oxc.md) reads `a ? b : c` with a test it cannot decide as what
both branches share. Two objects merge key by key, and lists merged from a conditional spread take the elements of
every branch. Any other pair of different values, two different strings above all, becomes `ConfigValue::Unknown`.

A Playwright `reporter` list whose reporter name was unknown used to be skipped, so the reporter modules it names were
reported by `unused-files` (GitHub issue #151). Marking such a list incomplete fixes that, and a reporter module may be
any file, so the package is then left unjudged as a whole (`IncompleteScope::Package`, [ADR
0065](0065-narrowed-scope-for-incomplete-entries.md)), its unused dependencies and stale ignore patterns included. The
most common reason a reporter name is unknown is a choice between known names. Six of the 22 Playwright configs of the
pinned corpus would lose all judgement that way, and none of them names a reporter the evaluator cannot know: they
pick a name with a ternary (`reporter: process.env.CI ? 'html' : 'list'`, `[ci ? 'blob' : 'list']`), spread a
conditional list (`...(env ? [['./reporters/a11y.ts']] : [])`) or filter a list with `.filter(Boolean)`, which the
evaluator does not follow either.

The Storybook fix of issue #152 made an addon, framework or builder whose package is unknown leave the package
unjudged as a whole too, although a package only hides dependencies and a local addon lives in the Storybook config
directory.

How should the evaluator hand out a value it knows is one of a few known values, so that plugins can credit each,
without changing what any other plugin reads?

## Decision Drivers

* A value the config picks among literals is known well enough to credit every one of them
* No plugin may read more than before unless it asks for it: a plugin that reads a key as one value must not take one
  branch for the whole
* The precision of narrowed scopes (ADR 0065) is not lost to a known choice
* `ConfigValue` is public; a change to it is a change for embedders

## Considered Options

* A `ConfigValue::OneOf` of the alternatives, read as unknown unless a plugin asks for the alternatives
* Merge two strings into a list, as a conditional spread merges lists
* Keep `Unknown` and let each plugin inspect the syntax of its key
* Mark the package incomplete with a narrowed scope instead

## Decision Outcome

Chosen option: "A `ConfigValue::OneOf` of the alternatives, read as unknown unless a plugin asks for the
alternatives", because it keeps every plugin's reading as it was, lets the plugins that read lists of names credit every
branch, and keeps the knowledge in the evaluator, where the branches are known.

### The value

`ConfigValue::OneOf(Vec<ConfigValue>)` is a value that is one of at least two different alternatives, without knowing
which. No alternative is `Unknown` or another `OneOf`; an array or object alternative may hold unknown parts.

* `ConfigValue::is_unknown` is true for `Unknown` and `OneOf`, and `ConfigValue::has_unknown` counts a `OneOf` as an
  unknown part. `as_str`, `as_array`, `as_object` and `get` return `None` for it.
* `ConfigValue::alternatives` returns the alternatives of a `OneOf` and the value itself for any other value.
* Every place a plugin, the plugin runner or `skif migrate` matched `ConfigValue::Unknown` asks `is_unknown` instead,
  so a `OneOf` is read as `Unknown` was. Only code that calls `alternatives` reads it.

### What the evaluator gives it for

The evaluator keeps a `Value::OneOf` of its own, handed out as `ConfigValue::OneOf`. One is made, flattened and
without duplicates, from values the evaluator knows:

* the two branches of `a ? b : c` with an unknown test that are not two objects (two objects still merge, and a key
  whose values differ is one of them);
* what the branches of an `if` with an unknown test leave a binding, and what two returns of a function give;
* a key every branch of a conditional spread sets to values that are not lists or objects, and a fallback over a read
  that is not definite (`x.reporter ?? 'dot'`).

It is unknown when an alternative is unknown, when a key only some branches set is not a list or an object, and when
there would be more than eight alternatives. A test on a `OneOf`, its `length` and a member of it are unknown, a merge
treats it as an unknown value, and a call that evaluates to one keeps its arguments as a call that evaluates to
`Unknown` does. A spread of a `OneOf` adds the keys or elements of each alternative, as a spread of the condition
itself does.

Two further readings come with it, since the lists they produce are where the choices sit:

* a spread into an array of an undecided condition or of a `OneOf` adds the elements of every branch, a branch of
  `null`, `undefined` or a boolean none, and the array is then not definite;
* `.filter(Boolean)` of a known array drops its falsy elements and keeps one that may be falsy as the values it has
  when it is not (`ci ? 'github' : null` as `'github'`, and in an array literal `ci && 'blob'` as `'blob'`), the array
  then not definite. A `Boolean` the module binds itself is not the global.

`undefined` evaluates to `undefined`, handed out as `null`, instead of an unknown value.

### What the plugins read

`support::reporter_names` reads a `reporter` or `reporters` value as a list of `Named::Known(name)` and
`Named::Unknown`, every alternative of the list, of an element and of a pair's name counting. Playwright and Vitest
credit every known name. A name still unknown, or a spread of a value the config computes that may set the key
(`support::reporters_unknown`, through the `support::last_entry` that `path_unknown` shares), leaves the entries
incomplete with the scope `Package`: Playwright's `reporter`, and Vitest's `test.reporters` of the config or an inline
project. Vitest used to skip an unknown reporter name.

Storybook reads every alternative of `addons`, of an addon, of `framework` and of `core.builder`. A name it cannot
know leaves the entries incomplete with the scope `Within` the Storybook config directory of the config (`<dir>/**`)
and the plugin's project globs, `.{storybook,rnstorybook}/**/*.{js,jsx,ts,tsx,mts}`, where a local addon lives; the
package's dependencies are not judged. A Storybook config unknown as a whole, or one with an unknown spread, stays
`Package`, since it may name stories anywhere.

### Scope table of ADR 0065, as amended

| Plugin | Scope, besides the plugin's own entry globs |
| --- | --- |
| `vitest` | as in ADR 0065; also `Package` when the config computes a name of `test.reporters` itself |
| `playwright` | as in ADR 0065; also `Package` when the config computes its `reporter` list itself: a reporter name is not known statically, or a spread of a value the config computes may set the list |
| `storybook` | for an unknown addon, framework or builder, the Storybook config directory and the project globs; `Package` for a config unknown as a whole or with an unknown spread |

### Related readings of the same change

* The directory of a resolved package manifest (`dirname(require.resolve('<pkg>/package.json'))`, also with
  `import.meta.resolve` and `fileURLToPath`, or with the package in one or several `join` arguments) is the package
  only for a bare specifier. A relative directory stays a path the path functions compute; an absolute one, a URL, a
  subpath import (`#x`) and a path with an empty, `.` or `..` segment are unknown.
* A bundler alias whose target is a package rather than a path (`react: 'preact/compat'`, or the
  `import.meta.resolve('bar')` the evaluator reads as `'bar'`) is an alias without a target, whose imports are not
  reported, and the package is a dependency of the config. Such an alias used to be dropped, so its imports counted as
  the package they name.

### Consequences

* Good, because a reporter picked among known names, a conditional reporter list and a filtered one are credited and
  leave the package judged, which covers the ways the six Playwright configs of the corpus name their reporters.
* Good, because a plugin that does not ask for the alternatives reads exactly what it read before.
* Good, because an unknown Storybook addon no longer hides every unused file of its package.
* Bad, because `ConfigValue` gains a variant, which breaks an embedder that matches it exhaustively.
* Bad, because each plugin that should credit every branch has to opt in; until it does, a `OneOf` is unknown to it.
* Bad, because a Vitest reporter name the evaluator cannot know now leaves the package unjudged, where it was
  skipped.

### Confirmation

* `crates/skif-js-syntax/tests/config.rs`: a `OneOf` is unknown to `is_unknown` and `has_unknown`, `as_str` is
  `None`, and `alternatives` lists it.
* `crates/skif-js-syntax/tests/config_functions.rs`: `an_undecided_condition_between_known_values_is_one_of_them`
  (ternaries of strings, nested ones flattened, a list element, a fallback, a reassigned binding, an opaque branch and
  nine alternatives unknown), `a_spread_of_one_of_several_values_adds_the_keys_of_each`,
  `an_array_spreads_the_elements_of_every_branch`, `filter_boolean_keeps_what_may_be_truthy` and
  `only_a_bare_manifest_directory_is_a_package`; the tests that read an undecided choice of known values as unknown
  read it as `OneOf`.
* `crates/skif-js-plugins/tests/fp_config.rs`: Playwright reporter lists picked among known names, filtered or spread
  conditionally are complete and credit their modules, an opaque one is `Package`; Vitest reporters of every branch
  are credited; an unknown Storybook addon, framework or builder is incomplete within the config directory.
* `crates/skif-js-plugins/tests/plugins.rs`: an alias to a package, to `import.meta.resolve('bar')` and to
  `dirname(require.resolve('bar/package.json'))`, is kept without a target and credits the package.
* `crates/skif-js/tests/fixtures.rs`: in `playwright-computed-reporters` the stale reporters of the packages whose
  reporters are known are reported and the package that parses its reporters from the environment stays unjudged; in
  `storybook-addon-helpers` the dead file of the package with an unknown addon is reported.
* `adrlint lint adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### A `ConfigValue::OneOf` of the alternatives, read as unknown unless a plugin asks for the alternatives

* Good, because the evaluator, which knows the branches, says what they are, and plugins choose to use it.
* Good, because the default reading is unchanged.
* Bad, because the public enum changes.

### Merge two strings into a list, as a conditional spread merges lists

* Good, because no new variant is needed.
* Bad, because a key read as one value (`testDir`, `environment`, `root`) would become a list it cannot hold, and a
  plugin that reads `as_str` would see the first branch or nothing without knowing a choice was made.

### Keep `Unknown` and let each plugin inspect the syntax of its key

* Good, because nothing in the evaluator changes.
* Bad, because a plugin sees values, not syntax: a choice made in a helper, a binding or an imported module is out of
  reach, and every plugin would repeat the work.

### Mark the package incomplete with a narrowed scope instead

* Good, because no value changes.
* Bad, because a reporter module may be any file, so no narrower scope is sound, and the choice is known anyway.

## More Information

* Amends: [ADR 0065](0065-narrowed-scope-for-incomplete-entries.md) (the scopes of Playwright and Vitest when the
  config computes its reporters, and a scope for Storybook)
* Related decisions: [ADR 0010](0010-js-analysis-on-oxc.md),
  [ADR 0078](0078-plugin-gaps-config-functions-file-tasks-and-templates.md)
* Issues: [#151](https://github.com/Goldziher/skif/issues/151), [#152](https://github.com/Goldziher/skif/issues/152)
* See the [ADR index](index.md).
