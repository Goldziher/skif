//! The linter: a validated registry plus the configuration it runs with.

use std::borrow::Cow;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};

use skif_config::{Config, Level};
use skif_core::{FileSystem, OsFs, Report, RuleMeta};
use skif_lint::{Executor, Registry, RuleProvider};
use skif_workspace::Workspace;

use crate::cache;
use crate::error::Error;
use crate::targets::Target;

/// The rule providers compiled into this build, in registration order.
///
/// The manifest and toolchain rules come with the `manifest` feature and the JS analysis rules with `js-analysis`;
/// the JS rules read source files through `fs` and take their plugin switches from `config`, so a host that lints
/// unsaved buffers passes its own [`FileSystem`]. The other rules that read files (toolchain files, a lockfile, patch
/// files) read the file system the linter runs with (see [`Linter::with_fs`]). Empty when neither feature is on. A
/// host that adds its own rules passes these plus its own to [`Linter::with_providers`].
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
    cancel: Option<&'a AtomicBool>,
    /// Run discovery, graph building and rules on a dedicated pool of this many threads; `None` uses the global
    /// pool. `1` runs everything on the calling thread and spawns no threads.
    threads: Option<NonZeroUsize>,
    /// Run rules on this reusable pool instead of building one per run; a host that lints often (an editor) creates
    /// one [`Executor`] and passes it to every run. Takes precedence over `threads`, except `threads = 1`, which
    /// stays on the calling thread.
    executor: Option<&'a Executor>,
    /// Run only these rules (a subset of the otherwise enabled ones); `None` runs every enabled rule.
    rules: Option<&'a [String]>,
    /// The configuration file the configuration was loaded from, relative to the workspace root (see
    /// [`config_file`](crate::config_file)); diagnostics about the configuration itself point at it.
    config_path: Option<&'a Path>,
    /// Files of this workspace that other roots of the same run import, relative to the workspace root.
    extra_entries: &'a [PathBuf],
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

    /// Makes `files`, relative to the workspace root, entries of the JS analysis: the files of this workspace that
    /// other roots of the same run import, as listed in their reports' [`Report::other_project_files`]. A host that
    /// lints nested roots passes them so a file only an outer root imports is not reported unused. Files the analysis
    /// does not own (ignored, outside the root, not source) are left out.
    #[must_use]
    pub const fn with_extra_entries(mut self, files: &'a [PathBuf]) -> Self {
        self.extra_entries = files;
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
/// The file system queries of the built-in JS analysis are memoised while runs are in flight: the cache is emptied
/// when the first of a group of overlapping runs starts and again when the last one ends, so concurrent runs share
/// what each has read (a run never empties the cache under another) and an idle linter holds nothing. Files that
/// change while runs overlap are therefore seen as they were when the group started; edit between runs, not during
/// them, or give each concurrent run of an editing host its own `Linter`.
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
    /// The memoising layer between the built-in JS analysis and the host's file system, emptied when no run is in
    /// flight; `None` when the providers were supplied by the host.
    cache: Option<Arc<SharedCache>>,
    /// How many runs are in flight, guarding the moments the cache is emptied.
    in_flight: Mutex<usize>,
}

type SharedCache = skif_core::CachedFs<cache::Through<Arc<dyn FileSystem>>>;

/// Holds the cache of a [`Linter`] for the duration of one run: the first overlapping run empties it on entry, the
/// last one on exit.
struct RunGuard<'a> {
    cache: Option<&'a SharedCache>,
    in_flight: &'a Mutex<usize>,
}

impl<'a> RunGuard<'a> {
    fn enter(cache: Option<&'a SharedCache>, in_flight: &'a Mutex<usize>) -> Self {
        let mut running = in_flight.lock().unwrap_or_else(PoisonError::into_inner);
        if *running == 0
            && let Some(cache) = cache
        {
            cache.clear();
        }
        *running += 1;
        Self { cache, in_flight }
    }
}

impl Drop for RunGuard<'_> {
    fn drop(&mut self) {
        let mut running = self.in_flight.lock().unwrap_or_else(PoisonError::into_inner);
        *running = running.saturating_sub(1);
        if *running == 0
            && let Some(cache) = self.cache
        {
            cache.clear();
        }
    }
}

impl Linter {
    /// Builds a linter over the built-in rules; the toolchain rules and the JS analysis (feature `js-analysis`) read the
    /// real file system.
    ///
    /// # Errors
    /// [`Error::Config`] when `config` names a rule that is not registered, [`Error::Registry`] when two rules
    /// share an ID.
    pub fn new(config: &Config) -> Result<Self, Error> {
        Self::with_fs(config, Arc::new(OsFs))
    }

    /// Builds a linter over the built-in rules whose toolchain rules, JS analysis and rules over a lockfile and patch
    /// files read through `fs`.
    ///
    /// Directory listings, existence checks and small configuration files read through `fs` are remembered for the
    /// duration of the runs in flight: a [`run_with`](Self::run_with) that starts while none is running begins with an
    /// empty cache, and the cache is emptied again when the last one ends, so a host that edits files between runs
    /// sees them. Overlapping runs share the cache (see [`Linter`]).
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

    /// Like [`with_fs`](Self::with_fs), with the JS analysis parsing source files through `modules` (ADR 0066).
    ///
    /// A host that lints repeatedly (watch mode, an editor) keeps the `Arc` and passes it to every linter it builds,
    /// so unchanged files are parsed once; [`MemoryModuleCache`](crate::MemoryModuleCache) is an in-process cache,
    /// and a host that persists one implements [`ModuleCache`](crate::ModuleCache) itself. The cache never changes a
    /// report: its keys cover the file name, the text and everything else a parse depends on.
    ///
    /// # Errors
    /// As [`Linter::new`].
    #[cfg(feature = "js-analysis")]
    pub fn with_module_cache(
        config: &Config,
        fs: Arc<dyn FileSystem>,
        modules: Arc<dyn skif_js::ModuleCache>,
    ) -> Result<Self, Error> {
        Self::with_js_provider(config, fs, |provider| provider.with_module_cache(modules))
    }

    /// Like [`with_fs`](Self::with_fs), with the JS analysis parsing source files through `modules`, the cache the
    /// linters of one run share (ADR 0101), as [`shared_modules`](Self::shared_modules) builds it. A host that builds
    /// one linter per distinct configuration of a multi-root run passes the same `modules` to each;
    /// [`for_targets`](Self::for_targets) builds one per target.
    ///
    /// # Errors
    /// As [`Linter::new`].
    #[cfg(feature = "js-analysis")]
    pub fn with_shared_modules(
        config: &Config,
        fs: Arc<dyn FileSystem>,
        modules: &skif_js::SharedModules,
    ) -> Result<Self, Error> {
        Self::with_js_provider(config, fs, |provider| provider.with_shared_modules(modules.clone()))
    }

    /// The module cache the linters of a run over `targets` share, through `host` when the host keeps a cache, or
    /// `None` when there is nothing to share (one target and no host cache); see
    /// [`SharedModules::for_run`](crate::SharedModules::for_run) for what it holds. [`for_targets`](Self::for_targets)
    /// and [`for_targets_with_cache`](Self::for_targets_with_cache) go through it.
    #[cfg(feature = "js-analysis")]
    #[must_use]
    pub fn shared_modules(
        targets: &[Target],
        host: Option<Arc<dyn skif_js::ModuleCache>>,
    ) -> Option<skif_js::SharedModules> {
        let roots = targets.iter().map(|target| target.workspace.root.clone()).collect();
        skif_js::SharedModules::for_run(roots, host)
    }

    /// One linter per target of a multi-root run (as [`discover_targets`](crate::discover_targets) lists them), in
    /// the order of `targets`, each reading through `fs`.
    ///
    /// With `js-analysis` and several targets the linters share one in-process module cache. It holds only the source
    /// files that can be shared, those outside the root parsing them (a directory of shared code outside every root)
    /// or inside a root nested in it, so such a file is parsed once for the run instead of once per root, also while
    /// the roots are linted in parallel. Each root parses its own files outside the cache. The entries live as long
    /// as the linters do. One target is [`with_fs`](Self::with_fs). A host that keeps a cache of its own passes it
    /// to [`for_targets_with_cache`](Self::for_targets_with_cache).
    ///
    /// # Errors
    /// As [`Linter::new`], for the first target whose configuration is rejected, located with [`Target::locate`].
    pub fn for_targets(targets: &[Target], fs: Arc<dyn FileSystem>) -> Result<Vec<Self>, Error> {
        #[cfg(feature = "js-analysis")]
        {
            Self::for_run(targets, fs, None)
        }
        #[cfg(not(feature = "js-analysis"))]
        {
            targets
                .iter()
                .map(|target| Self::with_fs(&target.config, Arc::clone(&fs)).map_err(|error| target.locate(error)))
                .collect()
        }
    }

    /// Like [`for_targets`](Self::for_targets), the linters parsing every source file through `modules`, such as a
    /// cache the host persists. With several targets a lookup of a file another root is parsing waits for that
    /// parse, so each file is parsed at most once per run even when `modules` is cold.
    ///
    /// # Errors
    /// As [`for_targets`](Self::for_targets).
    #[cfg(feature = "js-analysis")]
    pub fn for_targets_with_cache(
        targets: &[Target],
        fs: Arc<dyn FileSystem>,
        modules: Arc<dyn skif_js::ModuleCache>,
    ) -> Result<Vec<Self>, Error> {
        Self::for_run(targets, fs, Some(modules))
    }

    #[cfg(feature = "js-analysis")]
    fn for_run(
        targets: &[Target],
        fs: Arc<dyn FileSystem>,
        host: Option<Arc<dyn skif_js::ModuleCache>>,
    ) -> Result<Vec<Self>, Error> {
        let modules = Self::shared_modules(targets, host);
        targets
            .iter()
            .zip(std::iter::repeat_n(fs, targets.len()))
            .map(|(target, fs)| {
                let linter = match &modules {
                    Some(modules) => Self::with_shared_modules(&target.config, fs, modules),
                    None => Self::with_fs(&target.config, fs),
                };
                linter.map_err(|error| target.locate(error))
            })
            .collect()
    }

    /// The built-in providers over `fs`, with the JS provider finished by `modules`.
    #[cfg(feature = "js-analysis")]
    fn with_js_provider(
        config: &Config,
        fs: Arc<dyn FileSystem>,
        modules: impl FnOnce(skif_js::JsProvider) -> skif_js::JsProvider,
    ) -> Result<Self, Error> {
        let cache = cache::shared(fs);
        let shared = Arc::clone(&cache) as Arc<dyn FileSystem>;
        #[cfg(feature = "manifest")]
        let mut providers: Vec<Box<dyn RuleProvider>> = skif_rules::providers();
        #[cfg(not(feature = "manifest"))]
        let mut providers: Vec<Box<dyn RuleProvider>> = Vec::new();
        providers.push(Box::new(modules(crate::js::provider(config, shared))));
        let mut linter = Self::with_providers(config, providers)?;
        linter.cache = Some(cache);
        Ok(linter)
    }

    /// Builds a linter over exactly `providers`; pass [`builtin_providers`] plus your own to extend the built-ins.
    ///
    /// Rules that read files beyond the discovered workspace (toolchain files, a lockfile, patch files) read the real
    /// file system.
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
            in_flight: Mutex::new(0),
        })
    }

    /// The state of every dependency instance the version groups and the catalog rules govern, valid or not, as
    /// `skif list` and `skif json` show it. `report` is this linter's report of `workspace`: an instance it reports
    /// takes that diagnostic's state and severity.
    #[cfg(feature = "manifest")]
    #[must_use]
    pub fn instances(&self, workspace: &Workspace, report: &Report) -> Vec<skif_rules::policy::InstanceStatus> {
        let enabled = self.registry.enabled(&self.config);
        skif_rules::policy::inspect(workspace, &self.config, &|id| enabled.contains(id), &report.diagnostics)
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
        let _guard = RunGuard::enter(self.cache.as_deref(), &self.in_flight);
        let sequential = options.threads.is_some_and(|threads| threads.get() == 1);
        let built = match (options.executor, options.threads) {
            (None, Some(threads)) if !sequential => {
                Some(Executor::new(threads.get()).map_err(|error| Error::ThreadPool(error.to_string()))?)
            }
            _ => None,
        };
        let mut run_options = skif_lint::RunOptions::default().with_sequential(sequential);
        if let Some(executor) = options.executor.or(built.as_ref()) {
            run_options = run_options.with_executor(executor);
        }
        if let Some(cancel) = options.cancel {
            run_options = run_options.with_cancel(cancel);
        }
        if let Some(path) = options.config_path {
            run_options = run_options.with_config_path(path);
        }
        run_options = run_options.with_extra_entries(options.extra_entries);
        // The rules that read a lockfile or patch files read them through the file system this linter was built with.
        if let Some(cache) = self.cache.as_deref() {
            run_options = run_options.with_fs(cache);
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

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use skif_core::{DirEntry, MemoryFs};

    use super::*;

    /// A file system that counts the reads that reach it.
    struct Counting {
        inner: MemoryFs,
        reads: Arc<AtomicUsize>,
    }

    impl FileSystem for Counting {
        fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.inner.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
            self.inner.read_dir(path)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.inner.is_file(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.inner.is_dir(path)
        }
    }

    fn counting_cache() -> (Arc<SharedCache>, Arc<AtomicUsize>) {
        let reads = Arc::new(AtomicUsize::new(0));
        let fs = Counting {
            inner: MemoryFs::new().with_file("/r/package.json", "{}"),
            reads: Arc::clone(&reads),
        };
        (cache::shared(Arc::new(fs)), reads)
    }

    fn read(cache: &SharedCache) {
        cache.read_to_string(Path::new("/r/package.json")).unwrap();
    }

    #[test]
    fn overlapping_runs_share_the_cache_and_the_last_one_empties_it() {
        let (cache, reads) = counting_cache();
        let in_flight = Mutex::new(0);

        let first = RunGuard::enter(Some(&cache), &in_flight);
        read(&cache);
        let second = RunGuard::enter(Some(&cache), &in_flight);
        read(&cache);
        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "the second run must not empty the cache"
        );
        drop(first);
        read(&cache);
        assert_eq!(
            reads.load(Ordering::SeqCst),
            1,
            "a run ending while another runs keeps the cache"
        );
        drop(second);
        read(&cache);
        assert_eq!(reads.load(Ordering::SeqCst), 2, "the last run ending empties the cache");
    }

    #[test]
    fn a_run_that_starts_on_an_idle_linter_begins_with_an_empty_cache() {
        let (cache, reads) = counting_cache();
        let in_flight = Mutex::new(0);
        read(&cache);
        let _run = RunGuard::enter(Some(&cache), &in_flight);
        read(&cache);
        assert_eq!(reads.load(Ordering::SeqCst), 2);
    }
}
