# skif-js-syntax

The oxc boundary of [skif](https://github.com/Goldziher/skif): one JavaScript or TypeScript file in, skif-owned facts out.

- `parse_module` returns a `ModuleInfo`: imports (static, dynamic, `require`, `import.meta.glob`, re-exports, with the
  members accessed through each binding), exports (kind, JSDoc tags, enum and namespace members, removal ranges,
  custom element registrations), in-file references to exports, and CommonJS/JSX/directive hints.
- `extract_config_object` statically evaluates the object a tool config file exports; `extract_config_object_with`
  also follows the modules it imports through a `ModuleLoader`. `extract_config` returns an `ExtractedConfig`, the
  value with the evaluated arguments of the calls the evaluator cannot follow (a factory of an installed package) that
  the file exports, returns from its config function, merges or nests in the config; the loader is optional. Nothing is ever executed.
- `parse_cached` parses through a `ModuleCache` (`MemoryModuleCache` in memory) under a `ModuleKey` of the file name,
  its text, the parse options and the crate version, so an unchanged file is not parsed again.

No oxc type appears in the public API; the oxc crates are held to one release (see ADR 0010).
