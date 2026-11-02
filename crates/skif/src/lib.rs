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
//! | `js-analysis` | no | the oxc based JS and TypeScript analysis and the plugin table (`Linter` adds it to the built-in rules) |
//! | `fix` | no | `apply_fixes` and `FixPlan` |
//! | `yaml-fix` | no | YAML edits for `pnpm-workspace.yaml` (implies `fix`) |
//! | `report` | no | the human, JSON, SARIF and GitHub renderers, as `skif::render` |
//! | `schema` | no | `config_schema` |
//!
//! No `oxc`, `nodejs_semver` or `rayon` type appears in a public signature. Two data-model types remain public on
//! purpose (ADR 0003, ADR 0008): [`JsonAction`] and [`YamlAction`] carry a `serde_json::Value`, because a fix is
//! data a host may serialize, and [`RuleProvider::validate_options`] receives the `toml::Table` of a rule's options,
//! because a rule's options are TOML in `skif.toml`. The renderers that take `serde_json` members
//! (`render_json_with`) are not part of the facade; only the fixed renderers are re-exported as [`render`].

mod error;
#[cfg(feature = "fix")]
mod fix;
#[cfg(feature = "js-analysis")]
mod js;
mod linter;
mod root;
mod targets;

pub use error::Error;
#[cfg(feature = "fix")]
pub use fix::{
    Alternatives, AppliedFix, FixError, FixPlan, FixSelection, SkipReason, SkippedFix, apply_fixes,
    preferred_alternative,
};
pub use linter::{Linter, RunOptions, builtin_providers};
pub use root::{config_file, find_workspace_root, load_config};
pub use targets::{Target, discover_all, load_target};

pub use skif_config::{
    CONFIG_FILE_NAMES, Config, ConfigError, ConfigOverrides, IgnoreEntry, Level, PackageManager, Prefer, RuleConfig,
    Selector,
};
pub use skif_core::{
    Category, Diagnostic, DiagnosticBuilder, DirEntry, FileChange, FileSystem, Fix, FixOp, FixSafety, Fixability,
    InsertPosition, JsonAction, Location, MemoryFs, OsFs, PathSeg, Related, Report, RuleId, RuleMeta, Severity, Span,
    Summary, TextEdit, YamlAction, diagnostic, slash,
};
pub use skif_lint::{
    Emitter, EnabledRules, LintContext, LintError, ProviderError, RegistryError, Rule, RuleProvider, RulesProvider,
};
pub use skif_workspace::{DiscoverOptions, Package, PackageId, RootKind, Workspace, WorkspaceError, discover_roots};

/// The renderers of [`Report`]s: human, JSON, SARIF and GitHub annotations.
#[cfg(feature = "report")]
pub mod render {
    pub use skif_report::{
        ErrorInfo, Format, RenderOptions, SCHEMA_VERSION, render, render_github, render_human, render_json,
        render_json_error, render_json_error_info, render_sarif,
    };
}

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
