//! skif as a library: discover a JavaScript or TypeScript monorepo, lint its dependency graph and plan fixes.
//!
//! The crate is a thin facade over the `skif-*` crates and behaves like a pure function of its inputs: it never
//! prints, prompts, exits, reads the environment or the working directory, or writes files. Everything is read
//! through a [`FileSystem`] ([`OsFs`] on disk, [`MemoryFs`] for unsaved buffers and tests), every path in a
//! diagnostic or edit is relative to the workspace root, and fixes are returned as data ([`FixPlan`]) for the caller
//! to show, merge or write. The phase timings of `skif_core::timing` are off unless the host installs a sink there.
//!
//! ```
//! use std::path::Path;
//! use std::sync::Arc;
//! use skif::{ConfigOverrides, Linter, MemoryFs, Workspace, find_workspace_root, load_config};
//!
//! let fs = MemoryFs::new()
//!     .with_file("/repo/package.json", r#"{"name": "root", "private": true, "workspaces": ["packages/*"]}"#)
//!     .with_file("/repo/packages/a/package.json", r#"{"name": "a", "version": "1.0.0"}"#);
//!
//! let root = find_workspace_root(Path::new("/repo/packages/a"), &fs)?;
//! let config = load_config(&root, &fs, &ConfigOverrides::default())?;
//! let workspace = Workspace::discover_with_fs(&root, &config, &fs)?;
//! // The linter reads source files (with `js-analysis`) through the same file system as discovery.
//! let report = Linter::with_fs(&config, Arc::new(fs.clone()))?.run(&workspace)?;
//! assert_eq!(report.summary.packages, 2);
//! # Ok::<(), skif::Error>(())
//! ```
//!
//! A host that lints many projects discovers every root below a directory with [`discover_targets`] (or
//! [`discover_all`]) and resolves the layered configuration of a single root with [`resolve_config`] and a
//! [`ConfigCache`] (the [`ConfigLoader`] behind it is public for hosts that load packages themselves):
//!
//! ```
//! use std::path::{Path, PathBuf};
//! use std::sync::Arc;
//! use skif::{Ceiling, ConfigCache, ConfigOverrides, Linter, MemoryFs, RunOptions, discover_targets, resolve_config};
//!
//! let fs = MemoryFs::new()
//!     .with_file("/r/skif.toml", "[lint]\nfail-on-warnings = true\n")
//!     .with_file("/r/a/package.json", r#"{"name": "a", "version": "1.0.0"}"#)
//!     .with_file("/r/b/skif.toml", "")
//!     .with_file("/r/b/package.json", r#"{"name": "b", "version": "1.0.0"}"#);
//!
//! // Layered configuration: every skif.toml from the root up to the ceiling, merged farthest first. The cache is
//! // shared by the roots of a run so each file is read once; `origins` says which file set which key.
//! let cache = ConfigCache::new();
//! let ceiling = Ceiling::VcsOr(PathBuf::from("/r"));
//! let resolved = resolve_config(Path::new("/r/a"), &ceiling, &fs, &ConfigOverrides::default(), &cache)?;
//! assert!(resolved.config.lint.fail_on_warnings);
//! assert_eq!(resolved.sources.len(), 1);
//! assert!(resolved.origins.last("lint.fail-on-warnings").is_some());
//!
//! // Multi-root: every workspace and standalone package below a directory, each with its own configuration.
//! let discovery = discover_targets(Path::new("/r"), &fs, &ConfigOverrides::default())?;
//! assert!(discovery.failures.is_empty());
//! assert_eq!(discovery.targets.len(), 2);
//! for target in &discovery.targets {
//!     let linter = Linter::with_fs(&target.config, Arc::new(fs.clone()))?;
//!     let config_path = target.config_path();
//!     let mut options = RunOptions::default();
//!     if let Some(path) = &config_path {
//!         options = options.with_config_path(path);
//!     }
//!     let report = linter.run_with(&target.workspace, &options)?;
//!     assert!(report.summary.packages >= 1);
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Features
//!
//! | Feature | Default | Brings in |
//! | --- | --- | --- |
//! | `manifest` | yes | the built-in rules: manifest rules, version and semver group policies and catalog rules |
//! | `js-analysis` | no | the oxc based JS and TypeScript analysis and the plugin table (`Linter` adds it to the built-in rules) |
//! | `fix` | no | `apply_fixes` and `FixPlan` |
//! | `yaml-fix` | no | YAML edits for `pnpm-workspace.yaml` (implies `fix`) |
//! | `report` | no | the renderers (human, JSON, SARIF, GitHub, Code Climate, compact, Markdown, CODEOWNERS), as `skif::render` |
//! | `schema` | no | `config_schema` |
//! | `update` | no | [`update`]: newer published versions from a registry the host supplies, planned as a `FixPlan` |
//!
//! # Third-party types in the API
//!
//! No `oxc` or `rayon` type appears in a public signature; reusable thread pools are an [`Executor`]. The data model
//! is the contract for the libraries below, and nothing else from outside skif and the standard library may appear in
//! a public item (`tests/public_api.rs` fails when it does). A major release of a listed library is a breaking
//! release of skif.
//!
//! | Library | Where it shows |
//! | --- | --- |
//! | `serde` | the `Serialize` and `Deserialize` impls of the data model |
//! | `serde_json` | [`JsonAction`] and [`YamlAction`] (a fix is data a host may serialize) and the manifest values of a [`Package`] |
//! | `toml` | a rule's options ([`RuleProvider::validate_options`] receives the `toml::Table`) |
//! | `indexmap` | the ordered maps of [`Config`] |
//! | `nodejs_semver` | the parsed ranges of a dependency specifier |
//!
//! <!-- allow-list: serde, serde_json, toml, indexmap, nodejs_semver -->
//!
//! The renderers that take `serde_json` members (`render_json_with`) are not part of the facade; only the fixed
//! renderers are re-exported as [`render`].
//!
//! # The configuration model
//!
//! [`Config`] and every type it holds are re-exported and readable. A host builds one in code with [`ConfigBuilder`]
//! or loads it with [`load_config`] and [`resolve_config`]. The `[workspace]` table is [`WorkspaceConfig`], to tell
//! it from the discovered [`Workspace`].

#![warn(missing_docs)]

// Compiles the examples of the README, which crates.io and GitHub show, so they cannot drift from the API.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
mod readme_examples {}

mod cache;
mod error;
#[cfg(feature = "fix")]
mod fix;
#[cfg(feature = "js-analysis")]
mod js;
mod linter;
mod root;
mod targets;
#[cfg(feature = "update")]
pub mod update;

pub use error::Error;
#[cfg(feature = "fix")]
pub use fix::{
    Alternatives, AppliedFix, FixError, FixPlan, FixSelection, SkipReason, SkippedFix, apply_fixes, content_hash,
    preferred_alternative,
};
#[cfg(feature = "js-analysis")]
pub use js::explain_usage;
pub use linter::{Linter, RunOptions, builtin_providers};
pub use root::{config_file, find_workspace_root, load_config, resolve_config};
#[cfg(feature = "js-analysis")]
pub use skif_js::{Absence, EntryReason, ExportReport, ExportUser, Importer, ReExport, UsageReport, Verdict};
pub use targets::{
    Discovery, RootFailure, Target, UnappliedConfig, UnappliedReason, discover_all, discover_targets, find_target,
    load_target, unapplied_config_files, unapplied_config_files_all,
};

/// The `[format]` table of the configuration: what `package-json-format` checks.
pub use skif_config::Format as FormatConfig;
/// The `[update]` table of the configuration (an [`update::Update`] is one proposed version).
pub use skif_config::Update as UpdateConfig;
/// The `[workspace]` table of the configuration (the discovered [`Workspace`] is a different type).
pub use skif_config::Workspace as WorkspaceConfig;
pub use skif_config::{
    Assignment, CONFIG_FILE_NAMES, Catalog, CatalogMode, Ceiling, Config, ConfigBuilder, ConfigCache, ConfigError,
    ConfigLoader, ConfigOverrides, ConfigSource, DependencyFamily, DependencyTypes, ExportKinds, IgnoreEntry,
    IgnoreExportsUsedInFile, Js, JsWorkspace, Level, Lint, Op, Origins, Override, OverrideScope, PackageJs,
    PackageManager, PluginConfig, PluginOptions, Policy, Prefer, ResetKey, ResolvedConfig, RuleConfig, RuleOptions,
    Selector, SemverGroup, SemverRange, SourceKind, UpdateGroup, UpdateTarget, VersionGroup, VersionPolicy,
    WorkspaceProtocol, join_key,
};
/// `[[custom-types]]` and the places a version can be declared.
pub use skif_config::{CustomStrategy, CustomType, DependencyType};
pub use skif_core::{
    CachedFs, Category, Diagnostic, DiagnosticBuilder, DirEntry, FileChange, FileSystem, Fix, FixOp, FixPreference,
    FixSafety, Fixability, IncompleteCause, IncompleteEntries, IncompletePackage, InsertPosition, JsonAction, Location,
    MemoryFs, OsFs, PathKind, PathSeg, Related, Report, RuleId, RuleMeta, Severity, Span, Summary, TextEdit,
    YamlAction, diagnostic, slash,
};
pub use skif_lint::{
    Emitter, EnabledRules, Executor, ExecutorError, LintContext, LintError, ProviderError, RegistryError, Rule,
    RuleProvider, RulesProvider,
};
pub use skif_workspace::{DiscoverOptions, Package, PackageId, RootKind, Workspace, WorkspaceError};

/// The renderers of [`Report`]s: human, JSON, SARIF, GitHub annotations, Code Climate, compact, Markdown and
/// CODEOWNERS groups.
#[cfg(feature = "report")]
pub mod render {
    pub use skif_report::{
        CODEOWNERS_LOCATIONS, CodeOwners, ErrorInfo, Format, RenderOptions, SCHEMA_VERSION, render, render_codeclimate,
        render_codeowners, render_compact, render_disclosure, render_github, render_human, render_json,
        render_json_error, render_json_error_info, render_markdown, render_sarif,
    };
}

/// The JSON Schema of `skif.toml`, pretty printed.
#[cfg(feature = "schema")]
#[must_use]
pub fn config_schema() -> String {
    // `Value`'s alternate `Display` is its pretty print, which cannot fail where `to_string_pretty` returns a `Result`.
    format!("{:#}\n", skif_config::json_schema())
}

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<Linter>;
    let _ = assert_send_sync::<Error>;
    let _ = assert_send_sync::<RunOptions<'static>>;
    let _ = assert_send_sync::<Config>;
    let _ = assert_send_sync::<ConfigCache>;
    let _ = assert_send_sync::<Report>;
    let _ = assert_send_sync::<Executor>;
    let _ = assert_send_sync::<ResolvedConfig>;
    let _ = assert_send_sync::<Target>;
    let _ = assert_send_sync::<Discovery>;
    let _ = assert_send_sync::<Workspace>;
    let _ = assert_send_sync::<Package>;
    let _ = assert_send_sync::<Diagnostic>;
    let _ = assert_send_sync::<RuleId>;
    let _ = assert_send_sync::<Selector>;
    let _ = assert_send_sync::<Fix>;
    let _ = assert_send_sync::<FixOp>;
    let _ = assert_send_sync::<CachedFs<OsFs>>;
    let _ = assert_send_sync::<OsFs>;
    let _ = assert_send_sync::<MemoryFs>;
    let _ = assert_send_sync::<ConfigLoader>;
    let _ = assert_send_sync::<PathKind>;
    let _ = assert_send_sync::<Emitter<'static>>;
    let _ = assert_send_sync::<LintContext<'static>>;
    let _ = assert_send_sync::<DiscoverOptions>;
    let _ = assert_send_sync::<Box<dyn Rule>>;
    let _ = assert_send_sync::<Box<dyn RuleProvider>>;
    #[cfg(feature = "js-analysis")]
    {
        let _ = assert_send_sync::<UsageReport>;
        let _ = assert_send_sync::<skif_js::JsProvider>;
    }
    #[cfg(feature = "fix")]
    {
        let _ = assert_send_sync::<FixPlan>;
        let _ = assert_send_sync::<FixSelection>;
        let _ = assert_send_sync::<FixError>;
        let _ = assert_send_sync::<SkipReason>;
    }
};
