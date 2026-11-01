//! Rule engine for skif.
//!
//! Rules detect; the [`Emitter`] owns policy. A rule reports every problem it finds, and the
//! emitter applies the configured severity, `[[ignore]]` entries and `workspace.ignore-packages`
//! identically for every rule, including rules a host adds through its own [`RuleProvider`].
//!
//! The crate performs no I/O, printing or process control and keeps no global state.

mod context;
mod emitter;
mod ignore;
mod provider;
mod registry;
mod runner;

pub use context::{EnabledRules, LintContext};
pub use emitter::Emitter;
pub use provider::{ProviderError, Rule, RuleProvider, RulesProvider};
pub use registry::{Registry, RegistryError};
pub use skif_core::{DiagnosticBuilder, diagnostic};
pub use runner::{LintError, RunOptions, run};
