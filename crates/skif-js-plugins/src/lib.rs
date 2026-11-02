//! JavaScript and TypeScript tool knowledge for skif.
//!
//! The [`scripts`] module turns a `package.json` script into the binaries, entry files and
//! dependencies it references. The declarative plugin table of ADR 0011 lives in [`plugin`],
//! [`plugins`] and [`registry`]; [`resolve_plugins`] is the seam the module graph consumes.
//!
//! The plugins' behaviour follows [knip](https://github.com/webpro-nl/knip) (ISC), which is the
//! reference for each tool's enablers, config files, entry patterns and config-referenced
//! dependencies. Configs are read statically and never executed (ADR 0010).

mod config_load;
mod files;
pub mod plugin;
pub mod plugins;
pub mod registry;
mod resolve;
pub mod scripts;
mod support;

pub use plugin::{
    BuildDir, ConfigAst, DepRef, Enabler, KNIP_COMMIT, PathAlias, Plugin, PluginContext, PluginId, PluginInput,
    PluginOutput, PluginSpec,
};
pub use resolve::resolve_plugins;
