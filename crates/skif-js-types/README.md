# skif-js-types

The type-checker boundary of [skif](https://github.com/Goldziher/skif): the TypeScript checker of
[tsrs](https://github.com/oxc-project/tsrs) answers which members of a project's classes, enums and namespaces are
read, and skif's analysis on oxc uses the answers where the project has types.

- `analyze` builds a program per selected `tsconfig.json`, queries the checker for the declarations in a `TypeQuery`
  and returns `TypeFacts`: typed member reads, uses of a declaration as a whole, reads it could not settle (credited
  by name), and the type coverage of every package with the reason when a package or file stays untyped.
- Declarations are named by file and the UTF-8 byte offset of their name, as skif's oxc facts name them.

No tsrs type appears in the public API; the tsrs crates are held to one exact version (see ADR 0044). The analysis is
on by default in the skif CLI and npm package and can be turned off (`[js] type-aware = false`, `--no-type-aware`);
see ADR 0043.
