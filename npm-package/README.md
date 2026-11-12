# skif

[skif](https://github.com/Goldziher/skif) is a dependency and package-manager linter, auditor and enforcer for
JavaScript and TypeScript, with first-class monorepo support: workspace rules and version and catalog policies,
for npm, pnpm, Yarn and Bun. It also finds unused, unlisted and unresolved dependencies, unused files and
exports.

## Requirements

Node.js versions that are not end-of-life (22, 24 and 26 today; see the
[Node.js release schedule](https://nodejs.org/en/about/previous-releases)) or Bun 1.x. Older versions may work but are not
supported. The package is ESM only; from CommonJS use `import()`, or `require()` on
Node.js 22.12 or newer.

## Install

```sh
pnpm add --save-dev skif-cli
# or: npm install --save-dev skif-cli / yarn add --dev skif-cli / bun add --dev skif-cli
```

Run it without installing:

```sh
pnpm dlx skif-cli
npx skif-cli
bunx -p skif-cli skif
```

## Use from code

```js
import { lint, lintAsync, run } from "skif-cli";

const report = JSON.parse(lint("./my-monorepo", { production: true }));
console.log(report.summary);

const outcome = await lintAsync("./my-monorepo");
if (outcome.error) console.error(outcome.error.kind, outcome.error.message, outcome.error.path);
else console.log(JSON.parse(outcome.report).summary);

const exitCode = run(["--format", "json", "./my-monorepo"]);
```

- `lint(path, options?)` lints the project containing `path` and returns the JSON report as a string. With
  `{ recursive: true }` it lints every workspace and standalone package at or below `path`, like `skif -r`, and
  the report gains a `roots` array, an `errors` array listing the roots that could not be loaded (the others are still
  linted) and, when the depth limit stopped the search, a `truncated` array of the directories it did not enter. It
  prints nothing and writes no files. When the lint cannot run it throws an `Error` with `kind` and, when there is one,
  `path` properties, the fields `lintAsync` returns in `error`. It blocks the event loop, so use `lintAsync` in a server. `{ config: "path/to/skif.toml" }`
  uses that file (and the files it extends) instead of discovering configuration files, like `--config`, and
  `{ noConfig: true }` reads none, like `--no-config`.
- `lintAsync(path, options?)` does the same on a worker thread and resolves with `{ report }` or, when the lint
  cannot run, `{ error: { kind, message, path? } }` (`kind` is `config`, `workspace`, `io`, `usage`, `ci-refusal` or `internal`). Both
  functions read the configuration file the way the command line does.
- `run(argv)` runs the full command line (`argv` excludes the program name) and returns its exit code. Output
  streams to the process's stdout and stderr. Unlike the `skif` bin, it does not change directory under `bazel run`:
  relative paths resolve against `process.cwd()`.

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

Windows has no platform package, so importing `skif-cli` on Windows fails; use the
[release archives](https://github.com/Goldziher/skif/releases), `cargo binstall skif-cli` or
[Scoop](https://github.com/Goldziher/skif#installation) instead ([ADR 0114](https://github.com/Goldziher/skif/blob/main/adrs/0114-windows-npm-packages-deferred.md)).

If no platform package is installed (for example because optional dependencies were disabled), importing
`skif-cli` fails with an error naming the missing package.

## Links

- [Documentation](https://github.com/Goldziher/skif#readme)
- [Changelog](https://github.com/Goldziher/skif/blob/main/CHANGELOG.md)
- [Issues](https://github.com/Goldziher/skif/issues)
