//! skif as a library: discover a JavaScript or TypeScript monorepo, lint its dependency graph and plan fixes.
//!
//! The crate is a thin facade over the `skif-*` crates and behaves like a pure function of its inputs: it never
//! prints, prompts, exits, reads the environment or the working directory, or writes files. Everything is read
//! through a [`FileSystem`] ([`OsFs`] on disk, [`MemoryFs`] for unsaved buffers and tests), every path in a
//! diagnostic or edit is relative to the workspace root, and fixes are returned as data ([`FixPlan`]) for the caller
//! to show, merge or write.
//!
//! ```
//! use std::path::Path;
//! use skif::{ConfigOverrides, Linter, MemoryFs, Workspace, find_workspace_root, load_config};
//!
//! let fs = MemoryFs::new()
//!     .with_file("/repo/package.json", r#"{"name": "root", "private": true, "workspaces": ["packages/*"]}"#)
//!     .with_file("/repo/packages/a/package.json", r#"{"name": "a", "version": "1.0.0"}"#);
//!
//! let root = find_workspace_root(Path::new("/repo/packages/a"), &fs)?;
//! let config = load_config(&root, &fs, &ConfigOverrides::default())?;
//! let workspace = Workspace::discover_with_fs(&root, &config, &fs)?;
//! let report = Linter::new(&config)?.run(&workspace)?;
//! assert_eq!(report.summary.packages, 2);
//! # Ok::<(), skif::Error>(())
//! ```
//!
//! # Features
//!
//! | Feature | Default | Brings in |
//! | --- | --- | --- |
//! | `manifest` | yes | the manifest rules |
//! | `policy` | yes | version, semver and catalog policies |
//! | `js-analysis` | no | reserved for the oxc based analysis |
//! | `fix` | no | `apply_fixes` and `FixPlan` |
//! | `yaml-fix` | no | YAML edits for `pnpm-workspace.yaml` (implies `fix`) |
//! | `report` | no | the human and JSON renderers, as `skif::render` |
//! | `schema` | no | `config_schema` |
//!
//! No `oxc`, `serde_json::Value`, `toml::Value`, `nodejs_semver` or `rayon` type appears in a public signature.

mod error;
#[cfg(feature = "fix")]
mod fix;
mod linter;
mod root;

pub use error::Error;
#[cfg(feature = "fix")]
pub use fix::{Alternatives, AppliedFix, FixError, FixPlan, FixSelection, SkipReason, SkippedFix, apply_fixes};
pub use linter::{Linter, RunOptions, builtin_providers};
pub use root::{find_workspace_root, load_config};

pub use skif_config::{
    CONFIG_FILE_NAMES, Config, ConfigError, ConfigOverrides, IgnoreEntry, Level, PackageManager, Prefer, RuleConfig,
    Selector,
};
pub use skif_core::{
    Category, Diagnostic, DiagnosticBuilder, DirEntry, FileChange, FileSystem, Fix, FixOp, FixSafety, Fixability,
    InsertPosition, JsonAction, Location, MemoryFs, OsFs, PathSeg, Related, Report, RuleId, RuleMeta, Severity, Span,
    Summary, TextEdit, YamlAction, diagnostic,
};
pub use skif_lint::{
    Emitter, EnabledRules, LintContext, LintError, ProviderError, RegistryError, Rule, RuleProvider, RulesProvider,
};
pub use skif_workspace::{Package, PackageId, Workspace, WorkspaceError};

/// The human and JSON renderers of [`Report`]s.
#[cfg(feature = "report")]
pub use skif_report as render;

/// The JSON Schema of `skif.toml`, pretty printed.
#[cfg(feature = "schema")]
#[must_use]
pub fn config_schema() -> String {
    let mut text = serde_json::to_string_pretty(&skif_config::json_schema()).unwrap_or_default();
    text.push('\n');
    text
}

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<Linter>;
    let _ = assert_send_sync::<Error>;
    let _ = assert_send_sync::<RunOptions<'static>>;
    let _ = assert_send_sync::<Config>;
    let _ = assert_send_sync::<Report>;
    let _ = assert_send_sync::<Workspace>;
    #[cfg(feature = "fix")]
    {
        let _ = assert_send_sync::<FixPlan>;
        let _ = assert_send_sync::<FixSelection>;
        let _ = assert_send_sync::<FixError>;
        let _ = assert_send_sync::<SkipReason>;
    }
};
