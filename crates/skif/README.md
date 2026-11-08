# skif

Embeddable facade of [skif](https://github.com/Goldziher/skif), a dependency and package-manager linter for
JavaScript and TypeScript monorepos (npm, pnpm, yarn and bun).

The crate discovers a workspace, runs the rules and plans fixes. It never prints, prompts, exits, reads the
environment or the working directory, or writes files: everything is read through a `FileSystem` (`OsFs` on disk, `MemoryFs` for
tests, `BufferFs` for an editor's unsaved buffers over another file system), and fixes come back as a `FixPlan` for the
caller to show, merge or write. A host that lints repeatedly passes one `ModuleCache` to `Linter::with_module_cache`
(feature `js-analysis`), so only changed files are parsed again.

```rust
use std::path::Path;
use std::sync::Arc;
use skif::{ConfigOverrides, Linter, MemoryFs, Workspace, find_workspace_root, load_config};

let fs = MemoryFs::new()
    .with_file("/repo/package.json", r#"{"name": "root", "private": true, "workspaces": ["packages/*"]}"#)
    .with_file("/repo/packages/a/package.json", r#"{"name": "a", "version": "1.0.0"}"#);

let root = find_workspace_root(Path::new("/repo/packages/a"), &fs)?;
let config = load_config(&root, &fs, &ConfigOverrides::default())?;
let workspace = Workspace::discover_with_fs(&root, &config, &fs)?;
// The linter reads source files (with `js-analysis`) through the same file system as discovery.
let report = Linter::with_fs(&config, Arc::new(fs.clone()))?.run(&workspace)?;
assert_eq!(report.summary.packages, 2);
# Ok::<(), skif::Error>(())
```

## Features

| Feature | Enables |
| --- | --- |
| `manifest` (default) | The built-in manifest, workspace, version and semver group policy and catalog rules. |
| `fix` | `apply_fixes` and `FixPlan` (format-preserving JSON edits). |
| `yaml-fix` | YAML edits for `pnpm-workspace.yaml` (implies `fix`). |
| `report` | Human, JSON, SARIF, GitHub, Code Climate, compact, Markdown and CODEOWNERS renderers, re-exported as `skif::render`. |
| `schema` | The JSON Schema of `skif.toml`, as `skif::config_schema`. |
| `update` | `skif::update`: newer published versions through a registry the host supplies, planned as a `FixPlan`. |
| `js-analysis` | JavaScript and TypeScript analysis and the plugin table; `Linter` adds it to the built-in rules. Off by default. |

The command-line tool is the [`skif-cli`](https://crates.io/crates/skif-cli) crate. See the
[repository](https://github.com/Goldziher/skif) for the rule reference and the architecture decisions.

Licensed under MIT.
