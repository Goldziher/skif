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
//! `unlisted-binaries`, `unused-optional-peer-dependencies`, `unused-files`, `unused-exports`, `unused-types`,
//! `unused-namespace-exports`, `unused-namespace-types`, `unused-enum-members`, `unused-namespace-members`,
//! `duplicate-exports`, `unparseable-files`, `stale-js-config` and `circular-imports` (off by default).
//!
//! No oxc type appears in the public API.

mod analysis;
mod ancestors;
mod config_check;
mod deps;
mod discover;
mod entries;
mod explain;
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
mod tsconfig_paths;
mod usage;

pub use analysis::{JsAnalysis, UnusedFile};
pub use deps::{DependencyUsage, UnlistedBinary, UnlistedImport};
pub use explain::{Absence, EntryReason, ExportReport, ExportUser, Importer, ReExport, UsageReport, Verdict};
pub use graph::{Module, ModuleGraph, ModuleId, ResolvedImport, Target};
pub use input::{BuildDir, InputRequest, InputSource, PathAlias, WorkspaceInputs};
pub use plugin_source::PluginSource;
pub use provider::{JsProvider, PROVIDER_ID};
pub use rules::{
    CircularImports, DuplicateExports, JsRule, StaleJsConfig, UnlistedBinaries, UnlistedDependencies, UnparseableFiles,
    UnresolvedImports, UnusedDependencies, UnusedDevDependencies, UnusedEnumMembers, UnusedExports, UnusedFiles,
    UnusedNamespaceExports, UnusedNamespaceMembers, UnusedNamespaceTypes, UnusedOptionalPeerDependencies, UnusedTypes,
    rules,
};
pub use usage::{ExportUsage, MemberUse};

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<JsProvider>;
    let _ = assert_send_sync::<ModuleGraph>;
    let _ = assert_send_sync::<ExportUsage>;
    let _ = assert_send_sync::<DependencyUsage>;
};
