# skif

[skif](https://github.com/polylint/skif) is a dependency and package-manager linter, auditor and enforcer for
JavaScript and TypeScript, with first-class monorepo support: workspace rules and version and catalog policies,
for npm, pnpm, Yarn and Bun. It also finds unused and unlisted dependencies, unresolved imports, and unused files
and exports.

## Requirements

Node.js versions that are not end-of-life (22, 24 and 26 today; see the
[Node.js release schedule](https://nodejs.org/en/about/previous-releases)) or Bun 1.x. Older versions may work but are not
supported. The package is ESM only; from CommonJS use `import()`, or `require()` on
Node.js 22.12 or newer.

## Install

```sh
pnpm add --save-dev @polylint/skif
# or: npm install --save-dev @polylint/skif / yarn add --dev @polylint/skif / bun add --dev @polylint/skif
```

Earlier releases were published as `skif-cli`. That package is deprecated on npm and receives no new versions;
replace it with `@polylint/skif`. The command is still `skif`.

Run it without installing:

```sh
pnpm dlx @polylint/skif
npx @polylint/skif
bunx @polylint/skif
```

## Use from code

```js
import { lint, lintAsync, run } from "@polylint/skif";

const report = JSON.parse(lint("./my-monorepo", { production: true }));
console.log(report.summary);

const outcome = await lintAsync("./my-monorepo");
if (outcome.error) console.error(outcome.error.kind, outcome.error.message, outcome.error.path);
else console.log(JSON.parse(outcome.report).summary);

const exitCode = run(["--format", "json", "./my-monorepo"]);
```

- `lint(path, options?)` lints the project containing `path` and returns the JSON report as a string. It applies
  the CLI's configuration checks, writes no files, and prints only the CLI's warnings, to stderr. When the lint cannot run it throws an `Error` with `kind` and, when there is one,
  `path` properties. It blocks the event loop, so use `lintAsync` in a server.
- `lintAsync(path, options?)` does the same on a worker thread and resolves with `{ report }` or, when the lint
  cannot run, `{ error: { kind, message, path? } }` (`kind` is `config`, `workspace`, `io`, `usage`, `ci-refusal` or
  `internal`).
- `run(argv)` runs the full command line (`argv` excludes the program name) and returns its exit code. Output
  streams to the process's stdout and stderr. Unlike the `skif` bin, it does not change directory under `bazel run`:
  relative paths resolve against `process.cwd()`.

`lint` and `lintAsync` read configuration files the way the command line does. Each option mirrors a flag:

| Option | Flag |
| --- | --- |
| `recursive` | `--recursive`: lint every workspace and standalone package at or below `path`. The report gains a `roots` array, an `errors` array of the roots that could not be loaded (the others are still linted) and, when the depth limit stopped the search, a `truncated` array of the directories it did not enter. |
| `config` | `--config`: use this file (and the files it extends) instead of discovering configuration files |
| `noConfig` | `--no-config`: read no configuration file; wins over `config` |
| `production` | `--production` |
| `noJs` | `--no-js` |
| `failOnWarnings` | `--fail-on-warnings` |
| `cache` | `--cache`, `--no-cache`, `--cache-location` and `--cache-max-size`: `false` turns the cache off, `true` turns it on over `SKIF_CACHE` and `[cache]`, and `{ location?: string, maxSize?: number \| string }` names its directory and cap (`maxSize` in bytes or as `"256MiB"`; an empty `location` is an error). The cache is on by default, opened for each call, and never changes the report; the environment, `[cache]` and the defaults apply below the option, and with `CI` set it is read but not written unless a location is given |
| `ignoreRules`, `ignorePackages`, `ignoreDependencies` | `--ignore-rule`, `--ignore-package`, `--ignore-dependency`, as arrays |

The `typeAware` and `noTypeAware` options were removed in skif 0.9.0 with the type-aware analysis: setting either
fails with a `usage` error.

Types ship in `index.d.ts`.

## How the native code gets here

skif is a native addon built with [napi-rs](https://napi.rs). There is no install script and nothing is
downloaded at install time or at first run. This package lists one platform package per target as
`optionalDependencies`, each carrying the prebuilt addon and declaring the `os`, `cpu` and, on Linux, `libc` it is
for. Your package manager installs only the one that matches the machine, so installs work offline and are
covered by the registry's integrity hashes.

| Platform | Package |
| --- | --- |
| macOS (Apple silicon) | `@polylint/skif-darwin-arm64` |
| macOS (Intel) | `@polylint/skif-darwin-x64` |
| Linux arm64 (glibc) | `@polylint/skif-linux-arm64-gnu` |
| Linux arm64 (musl, Alpine) | `@polylint/skif-linux-arm64-musl` |
| Linux x64 (glibc) | `@polylint/skif-linux-x64-gnu` |
| Linux x64 (musl, Alpine) | `@polylint/skif-linux-x64-musl` |
| Windows x64 | `@polylint/skif-win32-x64-msvc` |
| Windows arm64 | `@polylint/skif-win32-arm64-msvc` |

If no platform package is installed (for example because optional dependencies were disabled), importing
`@polylint/skif` fails with an error naming the missing package.

## polylint organization

skif is part of [polylint](https://github.com/polylint), a set of Rust linters and formatters. Sibling tools:
[polylint](https://github.com/polylint/polylint) (one linter and formatter for every language; it can run skif as an
engine), [adrlint](https://github.com/polylint/adrlint) (MADR decision records),
[uncomment](https://github.com/polylint/uncomment) (comment removal and linting). Install any of them from the Homebrew
tap (`brew tap polylint/tap`) or the Scoop bucket.

## Links

- [Documentation](https://polylint.github.io/skif/)
- [Changelog](https://github.com/polylint/skif/blob/main/CHANGELOG.md)
- [Issues](https://github.com/polylint/skif/issues)
