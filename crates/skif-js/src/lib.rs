//! Module graph and JavaScript/TypeScript rules for skif: the knip issue types on top of oxc.
//!
//! [`JsProvider`] is a [`skif_lint::RuleProvider`] with id `"js"`. For a run it
//!
//! 1. discovers source files through a [`skif_core::FileSystem`] and assigns each to its workspace package,
//! 2. collects entry files from configuration, defaults, `package.json` (`main`, `bin`, `exports`), scripts and
//!    [`InputSource`]s (the plugin seam),
//! 3. follows imports with `oxc_resolver` (tsconfig `paths` and references, `exports` and `imports` conditions,
//!    workspace packages, builtins), parsing each round of files in parallel,
//! 4. propagates usage through re-export chains and compares referenced packages and binaries with each manifest.
//!
//! The rules are `unused-dependencies`, `unused-dev-dependencies`, `unlisted-dependencies`, `unresolved-imports`,
//! `stale-mocks`, `unlisted-binaries`, `unused-optional-peer-dependencies`, `unused-files`, `unused-exports`,
//! `unused-types`, `unused-namespace-exports`, `unused-namespace-types`, `unused-enum-members`,
//! `unused-namespace-members`, `duplicate-exports`, `unparseable-files`, `stale-js-config` and `circular-imports`
//! (off by default).
//!
//! No oxc type appears in the public API.

#![warn(missing_docs)]

mod analysis;
mod ancestors;
mod components;
mod config_check;
mod config_hints;
mod config_modules;
mod deps;
mod discover;
mod entries;
mod explain;
mod fixture;
mod git_index;
mod globs;
mod graph;
mod input;
mod listed;
mod lockfile;
mod manifest_paths;
mod plugin_source;
mod provider;
mod resolve;
mod rules;
mod scripts;
mod setup;
mod source_scan;
mod specifier;
mod trace;
mod tsconfig_files;
mod tsconfig_paths;
mod usage;

pub use analysis::{JsAnalysis, UnusedFile, discovery_file_system};
pub use config_check::relative_tsconfig;
pub use deps::{DependencyUsage, ForeignMapping, UnlistedBinary, UnlistedConfigDependency, UnlistedImport};
pub use explain::{Absence, EntryReason, ExportReport, ExportUser, Importer, ReExport, UsageReport, Verdict};
#[doc(hidden)]
pub use globs::IncompleteScopeGlobs;
pub use graph::{Module, ModuleGraph, ModuleId, ResolvedImport, Target};
pub use input::{
    BuildDir, ConfigDependency, CustomElementLibrary, IncompleteCause, IncompleteEntries, IncompleteScope,
    InputRequest, InputSource, PathAlias, RunBinary, WorkspaceInputs,
};
#[doc(hidden)]
pub use manifest_paths::is_out_dir;
#[doc(hidden)]
pub use manifest_paths::source_candidates;
pub use plugin_source::PluginSource;
pub use provider::{JsProvider, PROVIDER_ID};
pub use rules::{
    CircularImports, DuplicateExports, JsRule, StaleJsConfig, StaleMocks, UnlistedBinaries, UnlistedDependencies,
    UnparseableFiles, UnresolvedImports, UnusedDependencies, UnusedDevDependencies, UnusedEnumMembers, UnusedExports,
    UnusedFiles, UnusedNamespaceExports, UnusedNamespaceMembers, UnusedNamespaceTypes, UnusedOptionalPeerDependencies,
    UnusedTypes, rules,
};
/// The module cache of ADR 0066, re-exported for hosts that keep one.
pub use skif_js_syntax::{CachedModule, MemoryModuleCache, ModuleCache, ModuleKey, cache_version};
pub use trace::{DependencyImport, DependencyListing, DependencyTrace};
pub use tsconfig_files::empty_tsconfig_projects;
pub use usage::{ExportUsage, MemberUse};

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<JsProvider>;
    let _ = assert_send_sync::<ModuleGraph>;
    let _ = assert_send_sync::<ExportUsage>;
    let _ = assert_send_sync::<DependencyUsage>;
};
