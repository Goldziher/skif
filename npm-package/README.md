# skif

[skif](https://github.com/Goldziher/skif) is a dependency and package-manager linter, auditor and enforcer for
JavaScript and TypeScript, with first-class monorepo support: workspace rules and version and catalog policies,
for npm, pnpm, Yarn and Bun. Unused and unlisted dependency analysis is in progress.

## Requirements

Node.js 24 or newer, or Bun. The package is ESM.

## Install

```sh
pnpm add --save-dev skif
# or: npm install --save-dev skif / yarn add --dev skif / bun add --dev skif
```

Run it without installing:

```sh
pnpm dlx skif
npx skif
bunx skif
```

## Use from code

```js
import { lint, lintAsync, run } from "skif";

const report = JSON.parse(lint("./my-monorepo", { production: true }));
console.log(report.summary);

const outcome = await lintAsync("./my-monorepo");
if (outcome.error) console.error(outcome.error.kind, outcome.error.message, outcome.error.path);
else console.log(JSON.parse(outcome.report).summary);

const exitCode = run(["--format", "json", "./my-monorepo"]);
```

- `lint(path, options?)` lints the workspace containing `path` and returns the JSON report as a string. It prints
  nothing and writes no files. It blocks the event loop, so use `lintAsync` in a server.
- `lintAsync(path, options?)` does the same on a worker thread and resolves with `{ report }` or, when the lint
  cannot run, `{ error: { kind, message, path? } }` (`kind` is `config`, `workspace`, `io` or `internal`). Both
  functions read the configuration file the way the command line does.
- `run(argv)` runs the full command line (`argv` excludes the program name) and returns its exit code. Output
  streams to the process's stdout and stderr.

Types ship in `index.d.ts`.

## How the native code gets here

skif is a native addon built with [napi-rs](https://napi.rs). There is no install script and nothing is
downloaded at install time or at first run. This package lists one platform package per target as
`optionalDependencies`, each carrying the prebuilt addon and declaring the `os`, `cpu` and, on Linux, `libc` it is
for. Your package manager installs only the one that matches the machine, so installs work offline and are
covered by the registry's integrity hashes.

| Platform | Package |
| --- | --- |
| macOS (Apple silicon) | `skif-darwin-arm64` |
| macOS (Intel) | `skif-darwin-x64` |
| Linux arm64 (glibc) | `skif-linux-arm64-gnu` |
| Linux arm64 (musl, Alpine) | `skif-linux-arm64-musl` |
| Linux x64 (glibc) | `skif-linux-x64-gnu` |
| Linux x64 (musl, Alpine) | `skif-linux-x64-musl` |

Windows is not supported by this package. If no platform package is installed (for example because optional
dependencies were disabled), importing `skif` fails with an error naming the missing package.

## Links

- [Documentation](https://github.com/Goldziher/skif#readme)
- [Changelog](https://github.com/Goldziher/skif/blob/main/CHANGELOG.md)
- [Issues](https://github.com/Goldziher/skif/issues)
