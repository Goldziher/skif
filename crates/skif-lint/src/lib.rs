//! Rule engine for skif.
//!
//! Rules detect; the [`Emitter`] owns policy. A rule reports every problem it finds, and the
//! emitter applies the configured severity, `[[ignore]]` entries and `workspace.ignore-packages`
//! identically for every rule, including rules a host adds through its own [`RuleProvider`].
//!
//! Providers run concurrently on the current rayon pool (or an [`Executor`]) and their diagnostics are merged in
//! registration order, so a report does not depend on scheduling. Rules that need the same analysis share it through
//! [`LintContext::shared`].
//!
//! The crate performs no printing or process control and keeps no global state. Rules that read files beyond the
//! discovered workspace do so through [`LintContext::fs`], the host's [`FileSystem`](skif_core::FileSystem).
//!
//! # Data-model dependencies
//!
//! Rule options are TOML in `skif.toml` and option schemas are JSON Schema, so [`Rule::validate_options`],
//! [`RuleProvider::validate_options`], [`parse_options`] and [`reject_options`] take a `toml::Table` and the
//! `options_schema` methods return a `serde_json::Value`. These two types are part of this crate's contract: a major
//! release of either is a breaking release here. No other third-party type appears in a public signature; thread
//! pools are wrapped by [`Executor`].
//!
//! The crate is internal to skif and carries no stability promise of its own; `skif` re-exports what hosts need.

#![warn(missing_docs)]

mod context;
mod emitter;
mod ignore;
mod provider;
mod registry;
mod runner;
mod scoped;

pub use context::{DEFAULT_CONFIG_FILE, EnabledRules, LintContext};
pub use emitter::Emitter;
pub use ignore::{dependency_name, ignore_matches};
pub use provider::{OptionsError, ProviderError, Rule, RuleProvider, RulesProvider, parse_options, reject_options};
pub use registry::{Registry, RegistryError};
pub use runner::{Executor, ExecutorError, LintError, RunOptions, run, run_unsorted};
pub use skif_core::{DiagnosticBuilder, diagnostic};

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<Registry>;
    let _ = assert_send_sync::<Executor>;
    let _ = assert_send_sync::<RunOptions<'static>>;
    let _ = assert_send_sync::<EnabledRules>;
    let _ = assert_send_sync::<Emitter<'static>>;
    let _ = assert_send_sync::<RulesProvider>;
    let _ = assert_send_sync::<ProviderError>;
    let _ = assert_send_sync::<OptionsError>;
    let _ = assert_send_sync::<LintError>;
    let _ = assert_send_sync::<RegistryError>;
    let _ = assert_send_sync::<LintContext<'static>>;
    let _ = assert_send_sync::<Box<dyn Rule>>;
    let _ = assert_send_sync::<Box<dyn RuleProvider>>;
};
