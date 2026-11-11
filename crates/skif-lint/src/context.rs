use skif_core::HashMap;
use std::any::{Any, TypeId};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, OsFs, RuleMeta, Severity};
use skif_workspace::Workspace;

/// One shared value, stored by the first rule that finishes computing it.
type Slot = Arc<OnceLock<Arc<dyn Any + Send + Sync>>>;

/// Values computed once per run and shared by every rule and provider (see [`LintContext::shared`]).
#[derive(Default)]
pub(crate) struct SharedCache {
    slots: Mutex<HashMap<TypeId, Slot>>,
}

impl std::fmt::Debug for SharedCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedCache").finish_non_exhaustive()
    }
}

impl SharedCache {
    fn get_or_init<T: Any + Send + Sync>(&self, init: impl FnOnce() -> T) -> Arc<T> {
        let slot = {
            let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(slots.entry(TypeId::of::<T>()).or_default())
        };
        let value = if let Some(value) = slot.get() {
            Arc::clone(value)
        } else {
            let computed: Arc<dyn Any + Send + Sync> = Arc::new(init());
            Arc::clone(slot.get_or_init(|| computed))
        };
        value
            .downcast::<T>()
            .unwrap_or_else(|_| unreachable!("a cache slot only ever holds the type it is keyed by"))
    }
}

/// Read-only inputs shared by every rule.
///
/// Only the run constructs a context; the type is `#[non_exhaustive]` so inputs can be added without breaking hosts.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct LintContext<'a> {
    /// The workspace being linted.
    pub workspace: &'a Workspace,
    /// The configuration the run was started with.
    pub config: &'a Config,
    /// Which rules run, and at which severity.
    pub enabled: &'a EnabledRules,
    /// The configuration file `config` was loaded from, relative to the workspace root; `None` when the host
    /// does not know it (for example a configuration built in memory).
    pub config_path: Option<&'a Path>,
    /// Providers run their work on the calling thread and must not use rayon.
    pub sequential: bool,
    /// Files, relative to the workspace root, that other roots of a multi-root run import; the JS analysis makes
    /// those it owns entries. Empty for a run of one root.
    pub extra_entries: &'a [PathBuf],
    fs: FsRef<'a>,
    shared: &'a SharedCache,
}

/// The file system of a run; a newtype so [`LintContext`] and [`RunOptions`](crate::RunOptions) stay `Debug` and
/// `Copy` while holding a trait object.
#[derive(Clone, Copy)]
pub(crate) struct FsRef<'a>(pub(crate) &'a dyn FileSystem);

impl Default for FsRef<'_> {
    fn default() -> Self {
        Self(&OsFs)
    }
}

impl std::fmt::Debug for FsRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileSystem")
    }
}

impl<'a> LintContext<'a> {
    pub(crate) const fn new(
        workspace: &'a Workspace,
        config: &'a Config,
        enabled: &'a EnabledRules,
        config_path: Option<&'a Path>,
        sequential: bool,
        shared: &'a SharedCache,
    ) -> Self {
        Self {
            workspace,
            config,
            enabled,
            config_path,
            sequential,
            extra_entries: &[],
            fs: FsRef(&OsFs),
            shared,
        }
    }

    pub(crate) const fn with_fs(mut self, fs: FsRef<'a>) -> Self {
        self.fs = fs;
        self
    }

    /// The file system the host runs on, for rules that read files discovery does not (a lockfile, patch files).
    /// It is the one [`RunOptions::with_fs`](crate::RunOptions::with_fs) set, else the real file system. Read lazily:
    /// a run whose rules need none of those files reads none of them.
    #[must_use]
    pub fn fs(&self) -> &'a dyn FileSystem {
        self.fs.0
    }

    pub(crate) const fn with_extra_entries(mut self, extra_entries: &'a [PathBuf]) -> Self {
        self.extra_entries = extra_entries;
        self
    }

    /// A value of type `T` shared for the whole run: callers after the first stored value (other rules, other
    /// providers) get that value. Callers that arrive while no value is stored yet each run `init` and the first to
    /// finish wins, so `init` may run more than once and must compute the same value every time. No caller ever
    /// waits for another's `init`: a rayon worker blocked on a value whose `init` uses rayon itself can hold a job
    /// that `init` needs, and the run would deadlock. Use it for analyses several rules need, keyed by the type of
    /// the value; the value must not depend on anything that varies between rules.
    pub fn shared<T: Any + Send + Sync>(&self, init: impl FnOnce() -> T) -> Arc<T> {
        self.shared.get_or_init(init)
    }

    /// Name of the configuration file used by diagnostics about the configuration itself, relative to the
    /// workspace root: [`config_path`](Self::config_path), or [`DEFAULT_CONFIG_FILE`] when unknown.
    #[must_use]
    pub fn config_file(&self) -> &Path {
        self.config_path.unwrap_or_else(|| Path::new(DEFAULT_CONFIG_FILE))
    }
}

/// The configuration file name assumed when the host does not say where the configuration came from.
pub const DEFAULT_CONFIG_FILE: &str = "skif.toml";

/// The effective severity of every enabled rule.
///
/// A configured level overrides [`RuleMeta::default`]; `off`, or no default and no level,
/// disables the rule. A rule that is off at the base but turned on by an `[[overrides]]` block is enabled too: it
/// runs, and the [`Emitter`](crate::Emitter) keeps only the diagnostics inside the override's scope. A rule that sets
/// [`Diagnostic::default_severity`] replaces its [`RuleMeta::default`] for that finding, but never a configured
/// level; see [`effective`](Self::effective).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnabledRules {
    /// Per enabled rule: the base severity (`None` when only overrides turn the rule on), and whether the
    /// configuration set it (as opposed to the rule's default). Hashed rather than ordered: every emitted diagnostic
    /// looks its rule up here, and nothing iterates the map.
    severities: HashMap<&'static str, (Option<Severity>, bool)>,
}

impl EnabledRules {
    /// Resolves the effective severity of each rule in `metas` against `config`.
    #[must_use]
    pub fn resolve(metas: impl IntoIterator<Item = &'static RuleMeta>, config: &Config) -> Self {
        let severities = metas
            .into_iter()
            .filter_map(|meta| {
                let configured = config.rule(meta.id).and_then(|rule| rule.severity);
                let severity = configured.map_or(meta.default, skif_config::Level::severity);
                let scoped_on = !config.cli_levels.contains_key(meta.id)
                    && config.overrides.iter().any(|scoped| {
                        scoped
                            .rules
                            .get(meta.id)
                            .and_then(|rule| rule.severity)
                            .is_some_and(|level| level.severity().is_some())
                    });
                (severity.is_some() || scoped_on).then_some((meta.id, (severity, configured.is_some())))
            })
            .collect();
        Self { severities }
    }

    /// Whether rule `id` runs, at the base or in some override.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.severities.contains_key(id)
    }

    /// The base severity of rule `id`, or `None` when it is disabled at the base (even if an override enables it) or
    /// unknown.
    #[must_use]
    pub fn severity(&self, id: &str) -> Option<Severity> {
        self.severities.get(id).and_then(|(severity, _)| *severity)
    }

    /// Whether the configuration sets the level of rule `id`, as opposed to the level coming from the rule's own
    /// default.
    #[must_use]
    pub fn is_configured(&self, id: &str) -> bool {
        self.severities.get(id).is_some_and(|(_, configured)| *configured)
    }

    /// The severity `diagnostic` is reported with outside any override, or `None` when its rule is disabled at the
    /// base: the configured level when there is one, otherwise the severity the rule proposed for the finding,
    /// otherwise the rule's default.
    #[must_use]
    pub fn effective(&self, diagnostic: &Diagnostic) -> Option<Severity> {
        let (severity, configured) = *self.severities.get(diagnostic.rule.as_str())?;
        let severity = severity?;
        Some(if configured {
            severity
        } else {
            diagnostic.default_severity.unwrap_or(severity)
        })
    }

    /// The base severity and configured flag of an enabled rule.
    pub(crate) fn base(&self, id: &str) -> Option<(Option<Severity>, bool)> {
        self.severities.get(id).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::SharedCache;
    use rayon::prelude::*;
    use std::sync::{Arc, mpsc};
    use std::time::Duration;

    #[test]
    fn a_rule_asking_for_a_value_while_its_own_worker_computes_it_gets_the_stored_value() {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .expect("a thread pool");
            let cache = SharedCache::default();
            // The only worker runs the first job, whose `init` lets the worker run the second job, which asks for
            // the same value: a blocking cache would wait for the `init` below it on the same thread forever.
            let values: Vec<Arc<u32>> = pool.install(|| {
                (0..2)
                    .into_par_iter()
                    .with_max_len(1)
                    .map(|_| {
                        cache.get_or_init(|| {
                            rayon::yield_now();
                            7_u32
                        })
                    })
                    .collect()
            });
            sender.send(values).expect("the test is waiting");
        });
        let values = receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("both jobs got the value");
        assert_eq!(values.iter().map(|value| **value).collect::<Vec<_>>(), [7, 7]);
        assert!(Arc::ptr_eq(&values[0], &values[1]));
    }
}
