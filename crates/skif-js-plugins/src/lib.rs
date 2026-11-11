//! JavaScript and TypeScript tool knowledge for skif.
//!
//! The [`scripts`] module turns a `package.json` script into the binaries, entry files and
//! dependencies it references. The declarative plugin table of ADR 0011 lives in [`plugin`],
//! the per-tool modules and [`registry`]; [`resolve_plugins`] is the seam the module graph consumes, and
//! [`resolve_plugins_with`] runs the plugins in the Nx projects of a package too ([`NxProjects`], ADR 0109).
//!
//! The plugins' behaviour follows [knip](https://github.com/webpro-nl/knip) (ISC), which is the
//! reference for each tool's enablers, config files, entry patterns and config-referenced
//! dependencies. Configs are read statically and never executed (ADR 0010).

#![warn(missing_docs)]

mod config_load;
mod files;
pub mod glob;
pub mod plugin;
#[doc(hidden)]
pub mod plugins;
pub mod registry;
mod resolve;
mod scopes;
mod script_config;
pub mod scripts;
mod support;

pub use config_load::{load_config, load_config_with};
pub use plugin::{
    BinaryRef, BuildDir, ConfigAst, CustomElements, DepRef, Enabler, IncompleteCause, IncompleteEntries,
    IncompleteScope, KNIP_COMMIT, PathAlias, Plugin, PluginContext, PluginId, PluginInput, PluginOutput, PluginSpec,
    ScopedEntryRegex,
};
pub use resolve::{resolve_plugins, resolve_plugins_with};
pub use scopes::NxProjects;
pub use skif_js_syntax::ModuleLoader;
