//! The linter: a validated registry plus the configuration it runs with.

use std::borrow::Cow;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use skif_config::{Config, Level};
use skif_core::{FileSystem, OsFs, Report, RuleMeta};
use skif_lint::{Executor, Registry, RuleProvider};
use skif_workspace::Workspace;

use crate::cache;
use crate::error::Error;

/// The rule providers compiled into this build, in registration order.
///
/// The manifest rules come with the `manifest` feature and the JS analysis rules with `js-analysis`; the latter read
/// source files through `fs` and take their plugin switches from `config`, so a host that lints unsaved buffers
/// passes its own [`FileSystem`]. Empty when neither feature is on. A host that adds its own rules passes these plus
/// its own to [`Linter::with_providers`].
#[must_use]
pub fn builtin_providers(config: &Config, fs: Arc<dyn FileSystem>) -> Vec<Box<dyn RuleProvider>> {
    #[cfg(any(feature = "manifest", feature = "js-analysis"))]
    let mut providers: Vec<Box<dyn RuleProvider>> = Vec::new();
    #[cfg(not(any(feature = "manifest", feature = "js-analysis")))]
    let providers: Vec<Box<dyn RuleProvider>> = Vec::new();
    #[cfg(feature = "manifest")]
    providers.extend(skif_rules::providers());
    #[cfg(feature = "js-analysis")]
    providers.push(Box::new(crate::js::provider(config, fs)));
    #[cfg(not(feature = "js-analysis"))]
    let _ = (config, fs);
    providers
}

/// Host-controlled execution settings for [`Linter::run_with`].
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct RunOptions<'a> {
    /// Set to stop the run before the next rule or provider starts; the run then fails with a cancellation error.
    pub cancel: Option<&'a AtomicBool>,
    /// Run discovery, graph building and rules on a dedicated pool of this many threads; `None` uses the global
    /// pool. `1` runs everything on the calling thread and spawns no threads.
    pub threads: Option<NonZeroUsize>,
    /// Run rules on this reusable pool instead of building one per run; a host that lints often (an editor) creates
    /// one [`Executor`] and passes it to every run. Takes precedence over `threads`, except `threads = 1`, which
    /// stays on the calling thread.
    pub executor: Option<&'a Executor>,
    /// Run only these rules (a subset of the otherwise enabled ones); `None` runs every enabled rule.
    pub rules: Option<&'a [String]>,
    /// The configuration file the configuration was loaded from, relative to the workspace root (see
    /// [`config_file`](crate::config_file)); diagnostics about the configuration itself point at it.
    pub config_path: Option<&'a Path>,
}

impl<'a> RunOptions<'a> {
    /// Stops the run when `cancel` is set.
    #[must_use]
    pub const fn with_cancel(mut self, cancel: &'a AtomicBool) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// Runs rules on a dedicated pool of `threads` threads.
    #[must_use]
    pub const fn with_threads(mut self, threads: NonZeroUsize) -> Self {
        self.threads = Some(threads);
        self
    }

    /// Runs rules on the shared `executor` pool.
    #[must_use]
    pub const fn with_executor(mut self, executor: &'a Executor) -> Self {
        self.executor = Some(executor);
        self
    }

    /// Names the configuration file that diagnostics about the configuration point at.
    #[must_use]
    pub const fn with_config_path(mut self, config_path: &'a Path) -> Self {
        self.config_path = Some(config_path);
        self
    }

    /// Restricts the run to the rules named in `rules`.
    #[must_use]
    pub const fn with_rules(mut self, rules: &'a [String]) -> Self {
        self.rules = Some(rules);
        self
    }
}

/// Runs the registered rules over a [`Workspace`] and returns a [`Report`].
///
/// A `Linter` owns its [`Registry`] and a copy of the configuration it was validated against, so one value can lint
/// many workspaces from many threads. It never prints, prompts, reads the environment or writes files.
///
/// # Examples
/// ```
/// use std::path::Path;
/// use skif::{ConfigOverrides, Linter, MemoryFs, Workspace, load_config};
///
/// let fs = MemoryFs::new()
///     .with_file("/repo/package.json", r#"{"name": "root", "private": true, "workspaces": ["packages/*"]}"#)
///     .with_file("/repo/packages/a/package.json", r#"{"name": "a", "version": "1.0.0"}"#);
/// let root = Path::new("/repo");
/// let config = load_config(root, &fs, &ConfigOverrides::default())?;
/// let workspace = Workspace::discover_with_fs(root, &config, &fs)?;
/// let linter = Linter::new(&config)?;
/// let report = linter.run(&workspace)?;
/// assert_eq!(report.summary.packages, 2);
/// # Ok::<(), skif::Error>(())
/// ```
pub struct Linter {
    registry: Registry,
    config: Config,
    /// The memoising layer between the built-in JS analysis and the host's file system, emptied at the start of every
    /// run; `None` when the providers were supplied by the host.
    cache: Option<Arc<SharedCache>>,
}

type SharedCache = skif_core::CachedFs<cache::Through<Arc<dyn FileSystem>>>;

impl Linter {
    /// Builds a linter over the built-in rules; the JS analysis (feature `js-analysis`) reads the real file system.
    ///
    /// # Errors
    /// [`Error::Config`] when `config` names a rule that is not registered, [`Error::Registry`] when two rules
    /// share an ID.
    pub fn new(config: &Config) -> Result<Self, Error> {
        Self::with_fs(config, Arc::new(OsFs))
    }

    /// Builds a linter over the built-in rules whose JS analysis reads source files through `fs`.
    ///
    /// Directory listings, existence checks and small configuration files read through `fs` are remembered for the
    /// duration of one run: every [`run_with`](Self::run_with) starts from an empty cache, so a host that edits files
    /// between runs sees them.
    ///
    /// # Errors
    /// As [`Linter::new`].
    pub fn with_fs(config: &Config, fs: Arc<dyn FileSystem>) -> Result<Self, Error> {
        let cache = cache::shared(fs);
        let providers = builtin_providers(config, Arc::clone(&cache) as Arc<dyn FileSystem>);
        let mut linter = Self::with_providers(config, providers)?;
        linter.cache = Some(cache);
        Ok(linter)
    }

    /// Builds a linter over exactly `providers`; pass [`builtin_providers`] plus your own to extend the built-ins.
    ///
    /// # Errors
    /// As [`Linter::new`].
    pub fn with_providers(config: &Config, providers: Vec<Box<dyn RuleProvider>>) -> Result<Self, Error> {
        let registry = Registry::new(providers)?;
        registry.validate_config(config)?;
        Ok(Self {
            registry,
            config: config.clone(),
            cache: None,
        })
    }

    /// Metadata of every registered rule, enabled or not.
    pub fn rules(&self) -> impl Iterator<Item = &'static RuleMeta> + '_ {
        self.registry.rules()
    }

    /// The configuration this linter runs with.
    #[must_use]
    pub const fn config(&self) -> &Config {
        &self.config
    }

    /// Discovers the workspace at `root` through `fs` under this linter's configuration, loading members on the
    /// pool `options.threads` selects (see [`Workspace::discover_with_threads`]). Use it with
    /// [`run_with`](Self::run_with) so one thread budget covers both stages.
    ///
    /// # Errors
    /// [`Error::Workspace`] when discovery fails or the pool cannot be built.
    pub fn discover_with_fs(
        &self,
        root: &Path,
        fs: &dyn FileSystem,
        options: &RunOptions<'_>,
    ) -> Result<Workspace, Error> {
        Ok(Workspace::discover_with_threads(
            root,
            &self.config,
            fs,
            options.threads,
        )?)
    }

    /// Lints `workspace` with the default execution settings.
    ///
    /// # Errors
    /// See [`Linter::run_with`].
    pub fn run(&self, workspace: &Workspace) -> Result<Report, Error> {
        self.run_with(workspace, &RunOptions::default())
    }

    /// Lints `workspace` under `options`.
    ///
    /// # Errors
    /// [`Error::UnknownRule`] for a rule in `options.rules` that is not registered, [`Error::ThreadPool`] when the
    /// pool cannot be built, and [`Error::Lint`] when a provider fails or the run is cancelled.
    pub fn run_with(&self, workspace: &Workspace, options: &RunOptions<'_>) -> Result<Report, Error> {
        let config = self.restricted(options.rules)?;
        if let Some(cache) = &self.cache {
            cache.clear();
        }
        let sequential = options.threads.is_some_and(|threads| threads.get() == 1);
        let built = match (options.executor, options.threads) {
            (None, Some(threads)) if !sequential => Some(
                Executor::new(threads.get()).map_err(|error| Error::ThreadPool(error.to_string()))?,
            ),
            _ => None,
        };
        let mut run_options = skif_lint::RunOptions::default().sequential(sequential);
        if let Some(executor) = options.executor.or(built.as_ref()) {
            run_options = run_options.with_executor(executor);
        }
        if let Some(cancel) = options.cancel {
            run_options = run_options.with_cancel(cancel);
        }
        if let Some(path) = options.config_path {
            run_options = run_options.with_config_path(path);
        }
        Ok(skif_lint::run(&self.registry, workspace, &config, &run_options)?)
    }

    /// The configuration with every rule outside `selected` turned off.
    fn restricted(&self, selected: Option<&[String]>) -> Result<Cow<'_, Config>, Error> {
        let Some(selected) = selected else {
            return Ok(Cow::Borrowed(&self.config));
        };
        let mut config = self.config.clone();
        if let Some(unknown) = selected.iter().find(|id| self.registry.get(id).is_none()) {
            return Err(Error::UnknownRule(unknown.clone()));
        }
        for meta in self.registry.rules() {
            if !selected.iter().any(|id| id == meta.id) {
                config.rules.entry(meta.id.to_owned()).or_default().severity = Some(Level::Off);
                config.cli_levels.insert(meta.id.to_owned(), Level::Off);
            }
        }
        Ok(Cow::Owned(config))
    }
}
