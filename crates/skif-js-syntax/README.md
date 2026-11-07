# skif-js-syntax

The oxc boundary of [skif](https://github.com/Goldziher/skif): one JavaScript or TypeScript file in, skif-owned facts out.

- `parse_module` returns a `ModuleInfo`: imports (static, dynamic, `require`, `import.meta.glob`, re-exports, with the
  members accessed through each binding), exports (kind, JSDoc tags, enum and namespace members, removal ranges,
  custom element registrations), in-file references to exports, and CommonJS/JSX/directive hints.
- `extract_config_object` statically evaluates the object a tool config file exports. Nothing is ever executed.

No oxc type appears in the public API; the oxc crates are held to one release (see ADR 0010).
