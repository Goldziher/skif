# skif-js-types

The type-checker boundary of [skif](https://github.com/polylint/skif): the TypeScript checker of
[tsrs](https://github.com/oxc-project/tsrs) answers which members of a project's classes, enums and namespaces are
read, and skif's analysis on oxc uses the answers where the project has types.

- `analyze` builds a program per selected `tsconfig.json`, queries the checker for the declarations in a `TypeQuery`
  and returns `TypeFacts`: typed member reads, uses of a declaration as a whole, reads it could not settle (credited
  by name), and the type coverage of every package with the reason when a package or file stays untyped.
- `start` builds the programs before the query is known, from the files that may be judged, so they build while the
  host works its query out; `PendingTypes::finish` hands over the judged files and the query and returns the facts
  `analyze` gives for them. A program no judged file needs is freed unwalked.
- Declarations are named by file and the UTF-8 byte offset of their name, as skif's oxc facts name them.

No tsrs type appears in the public API; the tsrs crates are held to one exact version (see ADR 0010). The analysis is
built into the skif CLI and npm package and off by default (`[js] type-aware = true` or `--type-aware` turns it
on); see ADR 0010.

Programs are built a few at a time (a quarter of the threads, the rest shared out as checkers; one thread runs tsrs
single-threaded), each program's memory is freed as soon as its walk ends, and a panic inside the checker leaves that
program's packages untyped with the reason `checker-failed`. Every file a program reads is parsed and bound once and
shared by every program that reads it: the programs of one analysis, or of every analysis of a run that shares
`TypePrograms`, which also builds only once a program that the judged files of several nested roots go to. A bundled lib
file or a declaration file below `node_modules` is kept until the run's last program is freed, a project file until
the last program that read it is. The `plain-ptrs` feature builds tsrs with plain pointers
instead of its compressed handles and their fixed address reservation, for hosts such as Node.js that cannot give up
that range (ADR 0010).
