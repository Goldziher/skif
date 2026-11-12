//! skif as a library: discover a JavaScript or TypeScript monorepo, lint its dependency graph and plan fixes.
//!
//! The crate is a thin facade over the `skif-*` crates and behaves like a pure function of its inputs: it never
//! prints, prompts, exits, reads the environment or the working directory, or writes files. Everything is read
//! through a [`FileSystem`] ([`OsFs`] on disk, [`MemoryFs`] for tests, [`BufferFs`] for unsaved buffers), every path in a
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
//! // One linter per root; with `js-analysis` they share one module cache, so code several roots import is parsed
//! // once for the run.
//! let linters = Linter::for_targets(&discovery.targets, Arc::new(fs.clone()))?;
//! for (target, linter) in discovery.targets.iter().zip(&linters) {
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
//! A host that lints repeatedly, such as an editor, passes the same [`ModuleCache`] to every
//! [`Linter::with_module_cache`] (feature `js-analysis`), so only changed files are parsed again, and lints unsaved
//! buffers through a [`BufferFs`] over the disk. Neither changes a report: a cold, a warm and no cache report alike.
//! A host with a cache of its own lints the targets of a multi-root run with [`Linter::for_targets_with_cache`], where
//! a lookup of a file another root is parsing waits for that parse; a host that builds its linters itself shares one
//! [`SharedModules`] from [`Linter::shared_modules`] between them with [`Linter::with_shared_modules`].
//!
//! A root nested in another cannot see what the outer root imports from it. A host that wants those files counted
//! lints the enclosing roots first and passes the files their reports list in [`Report::other_project_files`],
//! made relative to the nested root, to [`RunOptions::with_extra_entries`] of the nested root's run, as `skif -r` does.
//! The other way round, an outer root cannot see that a nested root imports or runs a dependency only the outer
//! manifest lists, which Node.js resolves from the outer `node_modules`: the nested root's report lists those in
//! [`Report::ancestor_dependency_uses`], and `skif -r` drops the outer root's unused-dependency findings for them
//! (with [`Report::retain`]). Files of a root that code outside it reaches (a nested root requiring
//! `../tools/detect.js`, a sibling root) are listed in [`Report::other_project_files`] too, and `skif -r` drops the
//! owning root's `unused-files` findings for them the same way; the dependencies those files import count as used by
//! the manifest that lists them, through [`Report::ancestor_dependency_uses`]. Every path there is absolute.
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
mod path;
mod root;
mod shared_config;
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
pub use js::{explain_dependency, explain_usage, trace_dependency};
pub use linter::{Linter, RunOptions, builtin_providers};
pub use path::join_lexical;
pub use root::{config_file, find_workspace_root, load_config, resolve_config};
pub use shared_config::settle_shared_config;
/// The version a persisted [`ModuleCache`] records: a store written under another version is stale as a whole.
#[cfg(feature = "js-analysis")]
pub use skif_js::cache_version as module_cache_version;
#[cfg(feature = "js-analysis")]
pub use skif_js::{
    Absence, CachedModule, DependencyCredit, DependencyExplanation, DependencyImport, DependencyListing,
    DependencyTrace, DependencyUse, DependencyVerdict, EntryReason, ExportReport, ExportUser, Importer,
    MemoryModuleCache, ModuleCache, ModuleKey, PackageDependencyUsage, ReExport, SharedModules, UsageReport, UseKind,
    Verdict, empty_tsconfig_projects, relative_tsconfig,
};
/// The state of one dependency instance, from [`Linter::instances`].
#[cfg(feature = "manifest")]
pub use skif_rules::policy::{GroupRef, InstanceStatus, StatusType};
pub use targets::{
    Discovery, RootFailure, Target, UnappliedConfig, UnappliedReason, discover_all, discover_targets,
    discover_targets_below, find_target, load_target, unapplied_config_files, unapplied_config_files_all,
};

/// The `[format]` table of the configuration: what `package-json-format` checks.
pub use skif_config::Format as FormatConfig;
/// The `[update]` table of the configuration (an [`update::Update`] is one proposed version).
pub use skif_config::Update as UpdateConfig;
/// The `[workspace]` table of the configuration (the discovered [`Workspace`] is a different type).
pub use skif_config::Workspace as WorkspaceConfig;
pub use skif_config::{
    Assignment, CONFIG_FILE_NAMES, Catalog, CatalogMode, Ceiling, Config, ConfigBuilder, ConfigCache, ConfigError,
    ConfigLoader, ConfigOverrides, ConfigSource, DependencyFamily, DependencyTypes, ExportKinds, FamilyScope,
    GroupScope, IgnoreEntry, IgnoreExportsUsedInFile, IgnoreItem, InstanceFilter, Js, JsWorkspace, Level, Lint, Op,
    Origins, Override, OverrideScope, PackageJs, PackageManager, PluginConfig, PluginOptions, Policy, Prefer, ResetKey,
    ResolvedConfig, RuleConfig, RuleOptions, Selector, SemverGroup, SemverRange, SourceKind, UpdateGroup, UpdateTarget,
    VersionGroup, VersionPolicy, WorkspaceProtocol, join_key,
};
/// `[[custom-types]]` and the places a version can be declared.
pub use skif_config::{CustomStrategy, CustomType, DependencyType};
pub use skif_core::{
    BufferFs, CachedFs, Category, ConfigJudgement, Diagnostic, DiagnosticBuilder, DirEntry, FileChange, FileSystem,
    Fix, FixOp, FixPreference, FixSafety, Fixability, IncompleteCause, IncompleteEntries, IncompletePackage,
    IncompleteScope, InsertPosition, JsonAction, Location, MemoryFs, OsFs, PathKind, PathSeg, Related, Report, RuleId,
    RuleMeta, Severity, Span, Summary, TextEdit, YamlAction, diagnostic, slash,
};
pub use skif_lint::{
    Emitter, EnabledRules, Executor, ExecutorError, LintContext, LintError, ProviderError, RegistryError, Rule,
    RuleProvider, RulesProvider, dependency_name, ignore_matches,
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
    let _ = assert_send_sync::<BufferFs<OsFs>>;
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
        let _ = assert_send_sync::<MemoryModuleCache>;
        let _ = assert_send_sync::<SharedModules>;
        let _ = assert_send_sync::<CachedModule>;
        let _ = assert_send_sync::<ModuleKey>;
        let _ = assert_send_sync::<std::sync::Arc<dyn ModuleCache>>;
        let _ = assert_send_sync::<DependencyTrace>;
        let _ = assert_send_sync::<DependencyExplanation>;
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
