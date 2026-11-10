use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use skif_config::{Config, ConfigError};
use skif_core::{Diagnostic, FileSystem, IncompletePackage, Report};
use skif_workspace::{Workspace, map_ordered};

use crate::context::{FsRef, LintContext, SharedCache};
use crate::emitter::Emitter;
use crate::provider::ProviderError;
use crate::registry::Registry;

/// A reusable pool of worker threads that lint runs execute on.
///
/// Build one per host and pass it to every run: spawning and joining threads on each run (an editor linting on every
/// keystroke) is what it avoids. Cloning is cheap and shares the threads.
#[derive(Debug, Clone)]
pub struct Executor {
    pool: Arc<rayon::ThreadPool>,
}

impl Executor {
    /// Starts `threads` worker threads; `0` lets the pool pick one per available core.
    ///
    /// # Errors
    /// [`ExecutorError`] when the operating system refuses to spawn the threads.
    pub fn new(threads: usize) -> Result<Self, ExecutorError> {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()
            .map_err(|error| ExecutorError(Box::new(error)))?;
        Ok(Self { pool: Arc::new(pool) })
    }

    /// Number of worker threads.
    #[must_use]
    pub fn threads(&self) -> usize {
        self.pool.current_num_threads()
    }

    /// Runs `job` on the pool and returns its result; rule work started inside it stays on the pool.
    pub fn install<R: Send>(&self, job: impl FnOnce() -> R + Send) -> R {
        self.pool.install(job)
    }
}

/// The worker threads of an [`Executor`] could not be started.
#[derive(Debug, thiserror::Error)]
#[error("cannot build the thread pool: {0}")]
pub struct ExecutorError(#[source] Box<dyn std::error::Error + Send + Sync>);

/// Host-controlled execution settings, built with the `with_*` methods.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct RunOptions<'a> {
    executor: Option<&'a Executor>,
    cancel: Option<&'a AtomicBool>,
    config_path: Option<&'a Path>,
    sequential: bool,
    extra_entries: &'a [PathBuf],
    fs: FsRef<'a>,
}

impl<'a> RunOptions<'a> {
    /// Rules that read files beyond the discovered workspace (a lockfile, patch files) read them through `fs`; the
    /// real file system by default. A host that discovered the workspace through its own [`FileSystem`] passes it here.
    #[must_use]
    pub const fn with_fs(mut self, fs: &'a dyn FileSystem) -> Self {
        self.fs = FsRef(fs);
        self
    }

    /// Runs rules on `executor` instead of the current (global) pool. Ignored when [`with_sequential`](Self::with_sequential)
    /// is set.
    #[must_use]
    pub const fn with_executor(mut self, executor: &'a Executor) -> Self {
        self.executor = Some(executor);
        self
    }

    /// Stops the run before the next rule or provider starts when `cancel` is set.
    #[must_use]
    pub const fn with_cancel(mut self, cancel: &'a AtomicBool) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// The configuration file the configuration was loaded from, relative to the workspace root. Diagnostics about
    /// the configuration itself point at it; without it they fall back to `skif.toml`.
    #[must_use]
    pub const fn with_config_path(mut self, config_path: &'a Path) -> Self {
        self.config_path = Some(config_path);
        self
    }

    /// Files, relative to the workspace root, that other roots of the same run reach (they import them), for a
    /// multi-root host: the JS analysis makes those of its own files entries. Empty by default.
    #[must_use]
    pub const fn with_extra_entries(mut self, extra_entries: &'a [PathBuf]) -> Self {
        self.extra_entries = extra_entries;
        self
    }

    /// Runs everything on the calling thread: no rayon pool is used or created and the executor is ignored.
    #[must_use]
    pub const fn with_sequential(mut self, sequential: bool) -> Self {
        self.sequential = sequential;
        self
    }
}

/// A lint run failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LintError {
    /// The configuration names an unknown rule or invalid rule options.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// A provider failed to run.
    #[error("rule provider \"{provider}\" failed: {source}")]
    Provider {
        /// The failing provider, as reported by [`RuleProvider::id`](crate::RuleProvider::id).
        provider: &'static str,
        /// What went wrong.
        #[source]
        source: ProviderError,
    },
    /// The host set the cancellation flag.
    #[error("lint run cancelled")]
    Cancelled,
}

/// Runs every enabled rule and returns the sorted [`Report`].
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
    let collected = collect(registry, workspace, config, options)?;
    let root_stand_in = usize::from(!workspace.has_root_manifest());
    Ok(
        Report::new(collected.diagnostics, workspace.all_packages().count() - root_stand_in)
            .with_incomplete_packages(collected.incomplete_packages)
            .with_other_project_files(collected.other_project_files)
            .with_ancestor_dependency_uses(collected.ancestor_dependency_uses),
    )
}

/// Runs every enabled rule and returns the diagnostics in merge order, before [`Report`] sorting: providers in
/// registration order, rules in registration order within a provider, each rule's diagnostics in emission order.
///
/// The order does not depend on scheduling, which is what makes the sorted report reproducible.
///
/// # Errors
/// As [`run`].
pub fn run_unsorted(
    registry: &Registry,
    workspace: &Workspace,
    config: &Config,
    options: &RunOptions<'_>,
) -> Result<Vec<Diagnostic>, LintError> {
    collect(registry, workspace, config, options).map(|collected| collected.diagnostics)
}

/// What the providers of a run recorded.
struct Collected {
    /// The diagnostics, in merge order.
    diagnostics: Vec<Diagnostic>,
    /// The packages the providers could not judge.
    incomplete_packages: Vec<IncompletePackage>,
    /// The files of other projects the analysed code imports.
    other_project_files: Vec<PathBuf>,
    /// The dependencies the analysed code uses through manifests outside the workspace root.
    ancestor_dependency_uses: Vec<(PathBuf, String)>,
}

/// Runs every enabled rule and returns the diagnostics, the incomplete packages, the files of other projects and the
/// ancestor dependency uses the providers recorded.
fn collect(
    registry: &Registry,
    workspace: &Workspace,
    config: &Config,
    options: &RunOptions<'_>,
) -> Result<Collected, LintError> {
    registry.validate_config(config)?;
    let enabled = registry.enabled(config);
    let shared = SharedCache::default();
    let cx = LintContext::new(
        workspace,
        config,
        &enabled,
        options.config_path,
        options.sequential,
        &shared,
    )
    .with_extra_entries(options.extra_entries)
    .with_fs(options.fs);
    let job = || execute(registry, &cx, options);
    match options.executor {
        Some(executor) if !options.sequential => executor.install(job),
        _ => job(),
    }
}

/// Runs the providers concurrently (rule work inside a provider is parallel too) and merges their diagnostics in
/// registration order, so the result does not depend on scheduling. The first failure in registration order wins.
fn execute(registry: &Registry, cx: &LintContext<'_>, options: &RunOptions<'_>) -> Result<Collected, LintError> {
    let mut root = Emitter::new(cx).with_cancel(options.cancel);
    let base = &root;
    let parts = map_ordered(&registry.providers, cx.sequential, |provider| {
        if base.is_cancelled() {
            return Err(LintError::Cancelled);
        }
        let mut part = base.fork();
        skif_core::timing::phase(format_args!("provider {}", provider.id()), || {
            provider.run(cx, cx.enabled, &mut part)
        })
        .map_err(|source| LintError::Provider {
            provider: provider.id(),
            source,
        })?;
        Ok(part)
    });
    root.merge_all(parts.into_iter().collect::<Result<_, _>>()?);
    if root.is_cancelled() {
        return Err(LintError::Cancelled);
    }
    let other_project_files = root.take_other_project_files();
    let ancestor_dependency_uses = root.take_ancestor_dependency_uses();
    let (diagnostics, incomplete_packages) = root.into_parts();
    Ok(Collected {
        diagnostics,
        incomplete_packages,
        other_project_files,
        ancestor_dependency_uses,
    })
}
