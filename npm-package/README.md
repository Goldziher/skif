# skif

[skif](https://github.com/Goldziher/skif) is a fast dependency linter for JavaScript and TypeScript
monorepos: workspace rules, version and catalog policies, and unused or unlisted dependencies, in one
single-binary CLI.

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

## How the binary gets here

There is no install script and nothing is downloaded at install time or at first run. This package lists
eight platform packages as `optionalDependencies`, each carrying one prebuilt binary and declaring the
`os`, `cpu` and, on Linux, `libc` it is for. Your package manager installs only the one that matches
the machine, so installs work offline and are covered by the registry's integrity hashes.

| Platform | Package |
| --- | --- |
| macOS (Apple silicon) | `skif-darwin-arm64` |
| macOS (Intel) | `skif-darwin-x64` |
| Linux arm64 (glibc) | `skif-linux-arm64-gnu` |
| Linux arm64 (musl, Alpine) | `skif-linux-arm64-musl` |
| Linux x64 (glibc) | `skif-linux-x64-gnu` |
| Linux x64 (musl, Alpine) | `skif-linux-x64-musl` |
| Windows arm64 | `skif-win32-arm64` |
| Windows x64 | `skif-win32-x64` |

If no platform package is installed (for example because optional dependencies were disabled), `skif`
exits with code 2 and a message naming the missing package.

Set `SKIF_BINARY` to the path of a skif executable to run that binary instead of the bundled one.

## Links

- [Documentation](https://github.com/Goldziher/skif#readme)
- [Changelog](https://github.com/Goldziher/skif/blob/main/CHANGELOG.md)
- [Issues](https://github.com/Goldziher/skif/issues)
