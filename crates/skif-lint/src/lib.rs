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
pub use provider::{OptionsError, ProviderError, Rule, RuleProvider, RulesProvider, parse_options, reject_options};
pub use registry::{Registry, RegistryError};
pub use runner::{LintError, RunOptions, run, run_unsorted};
pub use skif_core::{DiagnosticBuilder, diagnostic};

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<Registry>;
    let _ = assert_send_sync::<EnabledRules>;
    let _ = assert_send_sync::<Emitter<'static>>;
    let _ = assert_send_sync::<RulesProvider>;
    let _ = assert_send_sync::<ProviderError>;
    let _ = assert_send_sync::<OptionsError>;
    let _ = assert_send_sync::<LintError>;
    let _ = assert_send_sync::<RegistryError>;
};
