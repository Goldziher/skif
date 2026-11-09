---
status: accepted
date: 2026-11-09
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0059", "0060"]
---

# Plugin Gaps: Config Functions Called Statically, File Tasks, Templates and Installed Versions

## Context and Problem Statement

[ADR 0059](0059-twenty-one-more-plugins-and-commands-run-from-source.md) and
[ADR 0060](0060-thirty-five-more-plugins-frameworks-task-runners-and-options-written-anywhere.md) list where nine
plugins read less than knip: `sst` does not read SST 2 stacks, `serverless-framework` skips a handler holding a
variable other than `${file(…)}` or a glob, `vue` does not call a function `configureWebpack`, `eleventy` does not read
passthrough copies, `rstest` does not apply `exclude` and does not read `rs.mock()`, `mise` does not read file tasks,
`taskfile` skips commands holding a `{{…}}` template, `stencil` does not mark component style URLs as used, and
`react-email` credits both preview server packages (GitHub issue #103). knip runs the configs or calls their
functions with stubs; skif never executes a config. Which of these can a static reading close, and how?

## Decision Drivers

* skif never executes a config, a task or a script
* A reading that cannot be completed must not name files or packages the tool does not use
* Each plugin keeps its conventional entries unless keeping them defeats what the config says
* New extraction belongs in `skif-js-syntax`, so the plugin crate holds no oxc type

## Considered Options

* Close each gap with a static reading, adding two config extractions to `skif-js-syntax`
* Document every gap as a limit of static analysis
* Evaluate config functions with stub objects, as knip does

## Decision Outcome

Chosen option: "Close each gap with a static reading, adding two config extractions to `skif-js-syntax`", because
every gap has a static form that covers the common configs, and the stub evaluation knip uses would need an
interpreter for arbitrary code.

**Two extractions.** `extract_property_calls(source, path, key, argument)` calls every function that is the property
`key` of an object literal (a method, a function or an arrow) with `argument` as its first parameter, through the
static evaluator of ADR 0010: the value is what the function returns or, when its body ends without a `return`, what it
leaves the parameter as. `extract_method_calls(source, path, method)` evaluates the arguments of every call of a method
of that name, whatever object it is called on.

**Per plugin.**

* `sst` reads the handler shorthand of SST 2 constructs in the config and the modules it imports, which already
  include the stacks of `app.stack(…)`: the string values of `routes`, `subscribers`, `notifications` and `targets`,
  and a `function`, `job` or `consumer` string.
* `serverless-framework` replaces a `${self:a.b}` variable of a handler with the string at `a.b` of the config,
  expanded in turn up to eight levels, and any other variable with `*`; a handler that is a glob stays one.
* `vue` calls a function `configureWebpack` with knip's base config (`mode`, empty `entry`, `resolve`, `plugins` and
  `module.rules`) and reads the result as a webpack config. A part the function changes in a way the evaluator does
  not follow does not mark the entries incomplete: Vue CLI takes its entries from `pages` and `src/main`.
* `eleventy` reads `addPassthroughCopy` calls: a directory makes its JavaScript and TypeScript files production
  entries, a file or glob is one, and a path into `node_modules` names a package.
* `rstest` applies `exclude` (a list or `{ patterns }`) and negated `include` patterns as `!` entries below the
  project's root. knip lists each project's files instead; a `!` entry needs no walk (no depth limit, no git-ignored
  build output) and removes the files it matches from every entry of the package, so the default test and mock globs
  stay and do not bring an excluded file back. Rstest's default exclusions are not added, as they would drop
  `node_modules`, `dist` and tool directories from every plugin's entries. `rs.mock()`, `rs.doMock()` and the other module calls of `rs`
  and `rstest` are mock imports like `vi.mock()`, so the parse cache schema is raised.
* `mise` reads file tasks under `mise-tasks/`, `.mise-tasks/`, `mise/tasks/`, `.mise/tasks/`, `.config/mise/tasks/`
  and `task_config.includes`: a JavaScript or TypeScript file is an entry, a shell script (no shebang with no other
  extension, or `sh`, `bash`, `zsh`, `dash` or `ksh`) is a script run in its `#MISE dir` or the workspace, and a task
  for another interpreter is not read.
* `taskfile` replaces `{{.NAME}}` with the string the task's or the Taskfile's `vars` give it, `ROOT_DIR` with the
  workspace and `TASKFILE_DIR` with the Taskfile's directory, and any other template with a placeholder the script
  parser reads as an unknown word; an include that keeps a placeholder is skipped.
* `stencil` needs no change: the `styleUrl` and `styleUrls` of a `@Component` from `@stencil/core` are imports of the
  component since the stylesheets of Angular and Stencil components are followed, so the stylesheets are used.
* `react-email` takes the major version of the installed `react-email` (`node_modules/react-email/package.json` in
  the workspace or an ancestor), else the first number of the declared range as knip does, and credits
  `@react-email/ui` from 6 and `@react-email/preview-server` before; both only when neither is known.

### Consequences

* Good, because the common forms of all nine configs are read without running anything.
* Good, because the two extractions serve any plugin whose options a function or a method call holds.
* Neutral, because a `*` for an unknown Serverless variable may match more files than the variable names; they are
  entries, so nothing is reported for them.
* Bad, because a `configureWebpack` that reads its argument's members or a file task for another interpreter is
  still not read, and an `rstest` exclusion applies to the whole package, so a file one project excludes is no entry
  of another project with the same root that includes it.

### Confirmation

* `crates/skif-js-plugins/tests/plugins.rs` has a red-green fixture test per plugin change.
* `crates/skif-js-syntax/tests/property_values.rs` covers both extractions and unparsable files, and
  `crates/skif-js-syntax/tests/fp_graph.rs` the `rs` and `rstest` mock calls.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Close each gap with a static reading, adding two config extractions to `skif-js-syntax`

* Good, because every listed gap closes for the forms configs commonly use.
* Bad, because each reading has its own edge (an unknown variable, a function that mutates nested members).

### Document every gap as a limit of static analysis

* Good, because nothing changes.
* Bad, because none of the gaps is a limit of static analysis: each has a static form.

### Evaluate config functions with stub objects, as knip does

* Good, because the config's own code decides what it names.
* Bad, because it needs an interpreter for arbitrary JavaScript, which skif deliberately does not run.
