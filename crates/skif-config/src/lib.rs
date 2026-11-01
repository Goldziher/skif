//! Configuration model and `skif.toml` loading for skif.
//!
//! Precedence is defaults < file < CLI: [`Config::default`] gives the defaults, [`discover`] or
//! [`Config::from_toml_str`] merge a file over them, and [`Config::apply`] layers
//! [`ConfigOverrides`] on top.

mod builder;
mod dependency_types;
mod discover;
mod error;
mod model;
mod overrides;
mod rules;
#[cfg(feature = "schema")]
mod schema;
mod selector;

pub use builder::ConfigBuilder;
pub use dependency_types::DependencyTypes;
pub use discover::{CONFIG_FILE_NAMES, discover};
pub use error::ConfigError;
pub use model::{
    Catalog, CatalogMode, Config, DependencyFamily, IgnoreEntry, Js, JsWorkspace, Lint, PackageManager, PluginConfig,
    PluginOptions, Policy, Prefer, SemverGroup, SemverRange, VersionGroup, VersionPolicy, Workspace, WorkspaceProtocol,
};
pub use overrides::ConfigOverrides;
pub use rules::{ForbidMode, Level, MemberToolchainFieldsOptions, RuleConfig, RuleOptions, UnusedExportsOptions};
#[cfg(feature = "schema")]
pub use schema::json_schema;
pub use selector::{LOCAL, Selector};
pub use skif_core::DependencyKind;
