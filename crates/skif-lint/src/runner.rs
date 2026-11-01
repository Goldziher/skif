use std::sync::atomic::AtomicBool;

use skif_config::{Config, ConfigError};
use skif_core::{Diagnostic, Report};
use skif_workspace::Workspace;

use crate::context::LintContext;
use crate::emitter::Emitter;
use crate::provider::ProviderError;
use crate::registry::Registry;

/// Host-controlled execution settings.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunOptions<'a> {
    /// Thread pool to run rules on; `None` uses the current (global) pool.
    pub pool: Option<&'a rayon::ThreadPool>,
    /// Set to stop the run before the next rule or provider starts.
    pub cancel: Option<&'a AtomicBool>,
}

/// A lint run failed.
#[derive(Debug, thiserror::Error)]
pub enum LintError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("rule provider \"{provider}\" failed: {source}")]
    Provider {
        provider: &'static str,
        #[source]
        source: ProviderError,
    },
    #[error("lint run cancelled")]
    Cancelled,
}

/// Runs every enabled rule and returns the diagnostics in deterministic order.
///
/// # Errors
/// [`LintError::Config`] for unknown rule IDs in `config`, [`LintError::Provider`] when a provider
/// fails, and [`LintError::Cancelled`] when `options.cancel` was set.
pub fn run(
    registry: &Registry,
    workspace: &Workspace,
    config: &Config,
    options: &RunOptions<'_>,
) -> Result<Report, LintError> {
    registry.validate_config(config)?;
    let enabled = registry.enabled(config);
    let cx = LintContext {
        workspace,
        config,
        enabled: &enabled,
    };
    let job = || execute(registry, &cx, options);
    let diagnostics = match options.pool {
        Some(pool) => pool.install(job),
        None => job(),
    }?;
    Ok(Report::new(diagnostics, workspace.all_packages().count()))
}

fn execute(registry: &Registry, cx: &LintContext<'_>, options: &RunOptions<'_>) -> Result<Vec<Diagnostic>, LintError> {
    let mut root = Emitter::new(cx).with_cancel(options.cancel);
    for provider in &registry.providers {
        if root.is_cancelled() {
            return Err(LintError::Cancelled);
        }
        let mut part = root.fork();
        provider
            .run(cx, cx.enabled, &mut part)
            .map_err(|source| LintError::Provider {
                provider: provider.id(),
                source,
            })?;
        root.merge(part);
    }
    if root.is_cancelled() {
        return Err(LintError::Cancelled);
    }
    Ok(root.into_diagnostics())
}
