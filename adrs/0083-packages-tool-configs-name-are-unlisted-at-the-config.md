---
status: accepted
date: 2026-11-10
decision-makers: ["Na'aman Hirschfeld"]
amends: ["0011", "0049"]
---

# Packages Tool Configs Name Are Unlisted at the Config, Declared Subpaths and Workspace Packages Behind Aliases

## Context and Problem Statement

The plugins read the packages a tool config names: the `extends` of a `tsconfig`, Oxlint `jsPlugins`, ESLint plugins
and shareable configs, Babel presets, PostCSS plugins, the `provider` of a Prisma generator and many more
([ADR 0011](0011-plugin-model.md)). Each one counts as a use of a dependency that a manifest lists, but one that no
manifest lists is dropped without a finding. knip reports such a package at the config file. On the parity corpus
(GitHub issue #96) this is most of the unlisted dependencies only knip reports at a config: `@calcom/tsconfig` in five
`tsconfig.json` files of cal-com, three Oxlint JS plugins of n8n, a Prisma generator, a `tsconfig` plugin and a
`types` entry.

The same comparison left ten knip-only findings in one package of cal-com unexplained. Nine import a workspace
package, `@calcom/web/modules/…`, that the importing package does not list: skif drops them because another package
declares `declare module "@calcom/web/public/app-store/svg-hashes.json"`, and a declared subpath counted as a
declaration of the whole package. The tenth imports `@calcom/prisma/enums` through a `tsconfig` mapping into a
gitignored directory the Prisma generator writes: an alias that leads to generated code dropped the package name,
while the same mapping into an existing file kept it and reported the import.

When is a package a config names an unlisted dependency, where is it reported, and how do ambient declarations and
aliases decide whether an import names a package?

## Decision Drivers

* A package a tool needs installed is the same dependency gap whether a module imports it or a config names it
* A finding must point at the file that names the package, where a user can fix or ignore it
* A plugin reference that is a guess, a command's argument or optional must not become a finding
* An import is judged by what it names, the same way whichever route resolves it

## Considered Options

* Report a package a config names at the config, only for references the config names itself
* Report every reference a plugin produces, commands and plugin defaults included
* Keep config references as uses only

## Decision Outcome

Chosen option: "Report a package a config names at the config, only for references the config names itself", because
it closes the gap where knip and skif read the same config, and keeps the references whose need skif cannot know out
of the findings.

**Which references qualify.** A plugin reference qualifies when a config file the plugin read names the package
itself (`DepRef::named_by_config`): an input of `Plugin::resolve_config` for that file. A file of a tool that is not a
config but names packages (a Prisma schema) gives the new `PluginInput::FileDependency`, which records the package at
that file and qualifies the same way. These do not qualify and stay uses only:

* a package that a command in a config names (`node --require x` in a hook), whose need follows from the command and
  is judged with the scripts, not the config;
* a package a plugin assumes without reading a config (`resolve_workspace`, `resolve_without_config`), which is a
  default of the tool rather than a statement of the project;
* an optional reference (`PluginInput::OptionalDependency`). Following knip's classification, Vite's
  `optimizeDeps.include` and the resolvers of `eslint-plugin-import` settings become optional references: the tool runs
  without them.

The plugins already drop what names no package: built-in names (`eslint:recommended`, `prisma-client-js`), relative
paths (entries or further configs) and computed values.

**How a reference is judged.** A qualifying reference belongs to the innermost workspace package whose directory holds
the config file, also when another package's plugins read it (a `tsconfig` reached through `references` or a relative
`extends`). It is a use of the manifest that lists it, as before. One no manifest lists is reported unless the same
exemptions as for imports apply: a runtime package, the package's own name, a peer dependency of a listed package, the
runtime of an `engines` entry, or (for a private package outside `--strict`) a workspace member, which then counts as
a use. `--strict` judges a config against its own package's manifest, in any dependency field: configs serve
development.

**Where and how it is reported.** `unlisted-dependencies` reports it at the config file, once per package, file and
name, with the message "`<file>` references `<name>`, which is not listed in `package.json`", the detail
`ConfigReference` and advice to add it to `devDependencies`. An `ignore-dependencies` entry or a plugin's ignored
dependencies silence it. A config below `node_modules`, outside the analysed root, in a fixture directory or matched by
a global or package `ignore` pattern is not judged. Production mode judges no config, as knip reports only production
references there and configs are development files.

**Declared subpaths.** `declare module 'pkg'` declares the package, so every import of it, also of a subpath, needs no
dependency, as before. A pattern with one `*` and a literal start (`declare module 'pkg/*'`) declares the imports it
matches. A declared subpath (`declare module 'pkg/data.json'`) declares that module only: an import of another subpath
of the package is judged as usual. The rule compares the specifier of the import, not only the package name.

**Workspace packages behind aliases.** A specifier that names a workspace package is an import of that package
wherever a `tsconfig` mapping or a bundler alias leads, also to generated code that is not there yet or to a file the
analysis does not follow. Until now this held only when the resolver followed the mapping to an existing file. This
amends the "Aliased and augmented packages" paragraph of
[ADR 0049](0049-js-corrections-from-adjudicating-knip-disagreements.md) for workspace packages: an alias that resolves
one is an import of it and is judged like any other; for other packages the paragraph stands.

**Not decided here.** A Sass importer that a config builds in code (`sassOptions.importer` from `sass-alias`, the
sitecore example of next.js loads it from a generated file) is not read, so the imports its aliases serve are
reported as unlisted packages; `ignore-dependencies` takes the alias. The packages that the commands of `package.json`
scripts and configs name stay uses only, and are a later decision for scripts and configs together.

### Consequences

* Good, because a `tsconfig` that extends a package, a lint config that loads a plugin and a schema that names a
  generator nobody installed are reported where they are written.
* Good, because the references whose need skif cannot know (commands, plugin defaults, optional references) produce
  no finding.
* Good, because an import of a workspace package is judged the same way through an existing file, generated code or a
  bundler alias, and a declaration of one module no longer hides the imports of a whole package.
* Bad, because repositories whose configs name packages no manifest lists see new `unlisted-dependencies` errors; they
  add the package or an `ignore-dependencies` entry.
* Bad, because a plugin that reads a config imprecisely now produces a finding where it produced a silent use.
* Bad, because a bundler alias that points a workspace package's name at a local shim now asks for the package.
* Neutral, because knip reports some unscoped packages a config names as unresolved files or not at all, where skif
  reports each as unlisted.

### Confirmation

* `crates/skif-js-plugins/tests/plugins.rs`: `packages_a_config_names_are_recorded_at_the_file_that_names_them` and
  `packages_named_only_to_tune_a_tool_are_optional_references`.
* `crates/skif-js/tests/config_references.rs`: one test each for `tsconfig` files, lint configs, generators with
  presets and PostCSS plugins, the references that are not unlisted, and ignores with production mode.
* `crates/skif-js/tests/unlisted_parity.rs`: `a_declared_subpath_declares_only_that_module` and
  `a_workspace_package_through_an_alias_into_generated_code_is_that_package`.
* `adrlint lint adrs`, `adrlint format --check adrs` and `adrlint lock --check` pass.

## Pros and Cons of the Options

### Report a package a config names at the config, only for references the config names itself

* Good, because a finding names a package the project wrote down.
* Bad, because a plugin's reading decides what is a finding, so each plugin's references must be classified as knip's
  are.

### Report every reference a plugin produces, commands and plugin defaults included

* Good, because it is the simplest rule and closest to knip, which reports every non-optional input.
* Bad, because the defaults a plugin assumes without a config and the packages of commands would be findings no file
  of the project states.

### Keep config references as uses only

* Good, because nothing changes for users.
* Bad, because a config that needs a package nobody installed stays silent.

## More Information

* Amends: [ADR 0011](0011-plugin-model.md) (the dependencies a config names are judged by `unlisted-dependencies`) and
  [ADR 0049](0049-js-corrections-from-adjudicating-knip-disagreements.md) (an alias that resolves a workspace package
  is an import of it)
* Related decisions: [ADR 0068](0068-what-the-knip-comparison-sets-apart.md),
  [ADR 0075](0075-imports-only-another-packages-tsconfig-paths-map.md),
  [ADR 0076](0076-binaries-run-outside-scripts-and-unlisted-corrections.md)
