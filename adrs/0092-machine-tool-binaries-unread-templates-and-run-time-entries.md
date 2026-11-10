---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0069", "0076", "0078", "0084", "0086"]
---

# Machine-Tool Binaries, Unread Templates and Run-Time Entries

## Context and Problem Statement

A comparison of the JS analysis with knip 6.44.1 on synthetic repositories found findings that follow from five
accepted decisions applied too narrowly or too literally:

* [ADR 0076](0076-binaries-run-outside-scripts-and-unlisted-corrections.md) judges only the binaries that CI steps,
  Git hooks and task runners reach through a package manager, "as knip's `optionalBinaries` does for the same
  plugins", but its list leaves out five of knip's: `mcp`, `tauri`, `travis`, `railway` and `mise`. Every repository
  with an `.mcp.json` that runs `uvx` gets a finding, and the binaries of a mise file task are reported at
  `package.json`.
* [ADR 0078](0078-plugin-gaps-config-functions-file-tasks-and-templates.md) replaces a Serverless variable with its
  quoted fallback (`${opt:stage, 'dev'}`), so the handlers of every other stage are unused files. It also reads no
  extensionless mise file task run by `node`, the usual shape of one, so the modules it loads are unused files.
* [ADR 0084](0084-template-scopes-auto-imported-vue-components-and-marko-files.md) credits every registered Vue
  component only where a package may use them in TSX, JSX or Markdown files. A `<template lang="pug">` or a
  `<script setup lang="tsx">` render function in a `.vue` file also uses components skif records no tags for, and
  those components are reported as unused files, which `--allow-remove-files` deletes. knip reports the same files.
* [ADR 0069](0069-a-published-path-into-nothing-is-reported-not-an-incomplete-entry.md) lists the build steps that may
  write a package's `bin`, without `bun build`, `vite build` or a command that names its output in `bin`, and says
  the stale path "is reported where knip reports it". knip 6.44.1 reports neither `bin/cli.js` that `bun build
  --outfile bin/cli.js` writes nor one `vite build` writes with `build.outDir: 'bin'`.
* [ADR 0086](0086-packages-tool-configs-name-are-unlisted-at-the-config.md) reports a package at a config only when
  the config names it itself. A config that imports its values from a shared config package (`extends: [base]` of
  Oxlint, a spread of an imported Stylelint config) was charged with every package the imported module names: the
  consumer got unlisted findings and the shared package, which lists them, got unused ones.

How far do these decisions reach?

## Decision Drivers

* A finding must be about a file of the project that states the fact, and about its owner
* A use skif cannot read must not turn into an unused file, since the fix deletes it
* Parity with knip where knip is right, and a stated deviation where skif goes further

## Considered Options

* Extend each decision to the cases it was meant to cover
* Keep the decisions and document the gaps

## Decision Outcome

Chosen option: "Extend each decision to the cases it was meant to cover", because each gap gives an error-level or
deleting finding about a file that is in use, and each extension follows the principle its decision states.

**Machine-tool commands.** `PluginSpec::optional_binaries` marks `mcp`, `tauri`, `travis`, `railway` and `mise` too,
as knip marks them (mise skips bare binaries in knip). The binaries a mise file task runs are recorded with the task
file, as `PluginInput::SourceScript`, which now also follows the plugin's `optional_binaries`.

**Extensionless JavaScript file tasks.** A mise file task without an extension whose shebang runs it with `node`,
`bun`, `deno`, `tsx` or `ts-node`, also through `npx`, `pnpx` or `bunx`, is read as JavaScript (as TypeScript for a
runner other than `node`). The graph knows a file's language by its extension, so the task is no module of it; the
relative modules it imports or requires are entries in its place, a specifier without an extension standing for its
files and `index` files. The packages it imports are not credited.

**Serverless variables read when the CLI runs.** Only a `${self:…}` variable takes its quoted fallback, since the
config holds its value. The fallback of any other source (`opt:`, `env:`, `sls:`, `param:`, `ssm:`) is one of the
values it takes, so such a variable is a wildcard like one without a fallback: `src/${opt:stage, 'dev'}/b.main` is
the entry `src/**/b.{js,ts}`.

**Vue blocks skif reads no tags in.** A package with a `.vue` file whose `<template>` has a `lang` other than `html`
(`pug`) or whose `<script>` is `tsx` or `jsx` credits every component it registers, as a package with a `.tsx` file
does. An unreadable template is taken to use every component rather than none.

**Build steps that write `bin`.** A script running `bun build`, `vite build`, `deno compile` or `parcel build`, or
`bunchee`, `microbundle`, `pkg` or `nexe`, is a build step, and so is any script that names a path at or in `bin` as
the output of a command (`--outfile bin/cli.js`, `-o bin/cli`, `--out-dir=bin`), whatever the script is called. ADR
0069's sentence that knip reports such a path is wrong: knip reports neither case, so this is no deviation from it.

**Packages a shared config module names.** When a config imports values from a module of another workspace package
and only that module names a package (a string literal of it, not of the config), the reference is recorded with the
module as its file, so the module's owner is judged for it, as for a config of its own: listed there, it is used
there, and the importing config's package is not asked to list it. A package the config names itself stays with the
config. knip makes such a reference optional (`isFilterTransitiveDependencies` of its Oxlint, Prettier, webpack and
Metro plugins); skif keeps it required for the owner, which is the package that has to install it.

### Consequences

* Good, because MCP, Tauri, Travis, Railway and mise commands no longer report the machine tools they run.
* Good, because the handlers of every Serverless stage, the modules of a JavaScript file task and the components a
  Pug template or a TSX render function uses are no longer unused files.
* Good, because a shared lint or style config package is judged for the plugins it names, and its consumers are not.
* Bad, because a wider entry or a credit of every registered component can keep an unused file from being reported.
* Bad, because a `bin` path that a stale `vite build` or output flag no longer writes is not reported.
* Bad, because the packages an extensionless file task imports are not credited, so a dependency only such a task
  uses is still reported unused.
* Neutral, because a package a shared module names by a shorthand (`plugins: ['foo']` for `eslint-plugin-foo`) is not
  matched to the module and stays with the config.

### Confirmation

* `crates/skif-js-plugins/tests/plugins.rs`:
  `machine_tool_commands_judge_only_binaries_reached_through_a_package_manager`,
  `mise_extensionless_javascript_file_tasks_make_what_they_load_entries` and
  `serverless_framework_handlers_plugins_and_file_variables`.
* `crates/skif-js/tests/frameworks.rs`:
  `registered_components_are_all_used_where_files_without_tags_may_use_them` with the `vue-components-pug` and
  `vue-components-jsx-script` fixtures, and the unit test
  `templates_in_other_languages_and_jsx_scripts_are_untagged_blocks`.
* `crates/skif-js/tests/config_hints.rs`: `a_bin_path_a_compiling_command_may_write_keeps_the_package_unjudged`.
* `crates/skif-js/tests/config_references.rs`: `packages_an_imported_shared_config_names_are_charged_to_its_package`.
* `crates/skif-js/tests/fp_graph.rs`: `modules_an_extensionless_node_mise_task_loads_are_used`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Extend each decision to the cases it was meant to cover

* Good, because the findings that delete used files or ask for packages in the wrong manifest go away.
* Bad, because each extension widens what is assumed used.

### Keep the decisions and document the gaps

* Good, because nothing changes for users.
* Bad, because the false positives stay, some of them errors and some removed by a fix.

## More Information

* Amends: [ADR 0069](0069-a-published-path-into-nothing-is-reported-not-an-incomplete-entry.md) (the build steps that
  may write `bin`, and its knip claim), [ADR 0076](0076-binaries-run-outside-scripts-and-unlisted-corrections.md) (the
  plugins with optional binaries), [ADR 0078](0078-plugin-gaps-config-functions-file-tasks-and-templates.md)
  (Serverless fallbacks and extensionless mise file tasks),
  [ADR 0084](0084-template-scopes-auto-imported-vue-components-and-marko-files.md) (the packages that credit every
  registered component) and [ADR 0086](0086-packages-tool-configs-name-are-unlisted-at-the-config.md) (the file a
  package of an imported config module is charged to)
