//! The linter: a validated registry plus the configuration it runs with.

use std::num::NonZeroUsize;
use std::sync::atomic::AtomicBool;

use skif_config::{Config, Level};
use skif_core::{Report, RuleMeta};
use skif_lint::{Registry, RuleProvider};
use skif_workspace::Workspace;

use crate::error::Error;

/// The rule providers compiled into this build, in registration order.
///
/// Empty when the `manifest` feature is off. A host that adds its own rules passes these plus its own to
/// [`Linter::with_providers`].
#[must_use]
pub fn builtin_providers() -> Vec<Box<dyn RuleProvider>> {
    #[cfg(feature = "manifest")]
    {
        skif_rules::providers()
    }
    #[cfg(not(feature = "manifest"))]
    {
        Vec::new()
    }
}

/// Host-controlled execution settings for [`Linter::run_with`].
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct RunOptions<'a> {
    /// Set to stop the run before the next rule or provider starts; the run then fails with a cancellation error.
    pub cancel: Option<&'a AtomicBool>,
    /// Run rules on a dedicated pool of this many threads; `None` uses the global pool.
    pub threads: Option<NonZeroUsize>,
    /// Run only these rules (a subset of the otherwise enabled ones); `None` runs every enabled rule.
    pub rules: Option<&'a [String]>,
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
}

impl Linter {
    /// Builds a linter over the built-in rules.
    ///
    /// # Errors
    /// [`Error::Config`] when `config` names a rule that is not registered, [`Error::Registry`] when two rules
    /// share an ID.
    pub fn new(config: &Config) -> Result<Self, Error> {
        Self::with_providers(config, builtin_providers())
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
        let pool = options
            .threads
            .map(|threads| {
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads.get())
                    .build()
                    .map_err(|error| Error::ThreadPool(error.to_string()))
            })
            .transpose()?;
        let run_options = skif_lint::RunOptions {
            pool: pool.as_ref(),
            cancel: options.cancel,
        };
        Ok(skif_lint::run(&self.registry, workspace, &config, &run_options)?)
    }

    /// The configuration with every rule outside `selected` turned off.
    fn restricted(&self, selected: Option<&[String]>) -> Result<Config, Error> {
        let mut config = self.config.clone();
        let Some(selected) = selected else {
            return Ok(config);
        };
        if let Some(unknown) = selected.iter().find(|id| self.registry.get(id).is_none()) {
            return Err(Error::UnknownRule(unknown.clone()));
        }
        for meta in self.registry.rules() {
            if !selected.iter().any(|id| id == meta.id) {
                config.rules.entry(meta.id.to_owned()).or_default().severity = Some(Level::Off);
            }
        }
        Ok(config)
    }
}
