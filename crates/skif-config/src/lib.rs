//! Configuration model and `skif.toml` loading for skif.
//!
//! Precedence is defaults < files < CLI: [`Config::default`] gives the defaults, [`ConfigLoader`] merges the chain
//! of files that governs a root (ancestors up to a ceiling, plus `extends`) over them keeping where every key came
//! from ([`ResolvedConfig`]), and [`Config::apply`] layers [`ConfigOverrides`] on top. [`discover`] and
//! [`Config::from_toml_str`] read a single file on its own.

#![warn(missing_docs)]

mod builder;
mod dependency_types;
mod discover;
mod error;
mod layer;
mod merge;
mod model;
mod overrides;
mod package_file;
mod provenance;
mod rules;
#[cfg(feature = "schema")]
mod schema;
mod selector;

pub use builder::ConfigBuilder;
pub use dependency_types::DependencyTypes;
pub use discover::{CONFIG_FILE_NAMES, Ceiling, ConfigCache, ConfigLoader, discover, discover_with_fs};
pub use error::ConfigError;
pub use model::{
    Catalog, CatalogMode, Config, DEFAULT_DISCOVER_IGNORE, DependencyFamily, IgnoreEntry, Js, JsWorkspace, Lint,
    Override, OverrideScope, PackageJs, PackageManager, PluginConfig, PluginOptions, Policy, Prefer, ResetKey,
    SemverGroup, SemverRange, VersionGroup, VersionPolicy, Workspace, WorkspaceProtocol,
};
pub use overrides::ConfigOverrides;
pub use provenance::{Assignment, ConfigSource, Op, Origins, ResolvedConfig, SourceKind, join_key};
pub use rules::{Level, RuleConfig, RuleOptions};
#[cfg(feature = "schema")]
pub use schema::json_schema;
pub use selector::{LOCAL, ROOT, Selector};
pub use skif_core::DependencyKind;

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<Config>;
    let _ = assert_send_sync::<ConfigError>;
    let _ = assert_send_sync::<ConfigOverrides>;
    let _ = assert_send_sync::<ConfigBuilder>;
    let _ = assert_send_sync::<Selector>;
    let _ = assert_send_sync::<DependencyTypes>;
    let _ = assert_send_sync::<RuleConfig>;
    let _ = assert_send_sync::<Level>;
    let _ = assert_send_sync::<IgnoreEntry>;
    let _ = assert_send_sync::<ConfigCache>;
    let _ = assert_send_sync::<ResolvedConfig>;
    let _ = assert_send_sync::<Override>;
    let _ = assert_send_sync::<VersionGroup>;
    let _ = assert_send_sync::<SemverGroup>;
};
