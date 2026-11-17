//! The cache of the command line (ADR 0007): which store a run caches through, where it lives, and what happens to
//! it once the run has finished.
//!
//! The cache is on by default. Its settings come from the flags (`--no-cache`, `--cache`, `--cache-location`,
//! `--cache-max-size`), else the environment (`SKIF_CACHE`, `SKIF_CACHE_DIR`, `SKIF_CACHE_MAX_SIZE`), else `[cache]`
//! of the root configuration, else the defaults: on, 512 MiB, at the first of `$TEST_TMPDIR/skif` under `bazel test`,
//! `node_modules/.cache/skif` below the base when `node_modules` exists, the per-user cache directory, or memory for
//! the run.
//!
//! A cache never fails a run or changes its report or exit code. A store that cannot be read is empty; a store that
//! cannot be written is a warning when the cache was asked for (a flag, a variable or the configuration turned it on
//! or named its location) and silent otherwise, with one line under `--debug`. With `CI` set and no location given
//! by a flag or a variable, the run reads the store and never writes it, since nothing would restore what it wrote.
//!
//! `[cache]` is a file of the project, and linting a checkout nobody vetted must be safe: its `location` stays
//! inside the directory of the file that sets it, never makes a `CI` run write, and its `max-size` caps the
//! project's own store without evicting the stores of other projects.
//!
//! A run that did not complete (it failed, or a root of it could not be linted) adds what it computed to the store
//! and drops nothing; a complete run keeps exactly what it used.
//!
//! `--watch` reads the store once and writes it after each round. `skif lsp` and `skif mcp` keep a memory cache per
//! root, seeded read-only from the store during the root's first lint, and never write it.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};

use skif::cache::{CacheHandle, CacheStore, MemoryStore, StoreModuleCache, salt_digest};
use skif::{CachedModule, MemoryModuleCache, ModuleCache, ModuleKey, SourceKind, Target, parse_byte_size};
use skif_cache::{DEFAULT_MAX_SIZE, DiskStore, DiskStoreOptions, Finish};
use skif_report::escape_controls;

use crate::output::stderr_line;

/// The cache settings given on the command line.
#[derive(Debug, Clone, Default)]
pub struct CacheFlags {
    /// `--cache`.
    pub cache: bool,
    /// `--no-cache`.
    pub no_cache: bool,
    /// `--cache-location`.
    pub location: Option<PathBuf>,
    /// `--cache-max-size`, in bytes.
    pub max_size: Option<u64>,
}

/// The variables the cache settings read, gathered once.
#[derive(Debug, Clone, Default)]
pub struct CacheEnv {
    /// `SKIF_CACHE`.
    enabled: Option<OsString>,
    /// `SKIF_CACHE_DIR`, when set and non-empty.
    dir: Option<PathBuf>,
    /// `SKIF_CACHE_MAX_SIZE`.
    max_size: Option<OsString>,
    /// `CI` is truthy.
    ci: bool,
    /// `TEST_TMPDIR` under `bazel test`.
    bazel: Option<PathBuf>,
    /// The per-user cache directory.
    user_cache_dir: Option<PathBuf>,
}

impl CacheEnv {
    /// No variable set, not CI and no per-user cache directory: for a test.
    #[cfg(test)]
    pub fn empty() -> Self {
        Self::default()
    }

    /// The variables of this process; `ci` is whether `CI` is truthy.
    pub fn detect(ci: bool) -> Self {
        let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        Self {
            enabled: var("SKIF_CACHE"),
            dir: var("SKIF_CACHE_DIR").map(PathBuf::from),
            max_size: var("SKIF_CACHE_MAX_SIZE"),
            ci,
            bazel: crate::env::bazel_test_tmpdir(),
            user_cache_dir: user_cache_dir(
                var("XDG_CACHE_HOME").map(PathBuf::from),
                var("HOME").map(PathBuf::from),
                var("LOCALAPPDATA").map(PathBuf::from),
            ),
        }
    }
}

/// The per-user cache directory: `$XDG_CACHE_HOME` when set to an absolute path, else `~/Library/Caches` on macOS,
/// `%LOCALAPPDATA%` on Windows and `~/.cache` elsewhere.
fn user_cache_dir(xdg: Option<PathBuf>, home: Option<PathBuf>, local_app_data: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(xdg) = xdg.filter(|dir| dir.is_absolute()) {
        return Some(xdg);
    }
    if cfg!(windows) {
        return local_app_data.filter(|dir| dir.is_absolute());
    }
    let home = home.filter(|dir| dir.is_absolute())?;
    Some(if cfg!(target_os = "macos") {
        home.join("Library").join("Caches")
    } else {
        home.join(".cache")
    })
}

/// Where and how a run caches, resolved from the flags, the environment, the root configuration and the defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Whether the run caches at all.
    pub enabled: bool,
    /// The cache location; `None` caches in memory for the run.
    pub location: Option<PathBuf>,
    /// The cap of the location.
    pub max_size: u64,
    /// Whether the user asked for the cache (turned it on or named its location): its failures are warnings.
    pub asked: bool,
    /// Whether the run writes the store.
    pub write: bool,
    /// What the cap holds down.
    pub cap: CapScope,
}

/// What the size cap of a run holds down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapScope {
    /// The whole location: over the cap, the stores of other bases are dropped. The user's own cap, or the default.
    Location,
    /// The store of this base only: a cap from the configuration, which is a file of the project.
    Store,
}

/// What one layer of the settings sets, before precedence is applied.
#[derive(Default)]
struct Layer {
    enabled: Option<bool>,
    location: Option<PathBuf>,
    max_size: Option<u64>,
}

/// `dir` made absolute against the current directory, as the flags and variables name it.
fn absolute(dir: &Path) -> PathBuf {
    std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf())
}

/// The settings of a run over `targets`, whose report paths are relative to `base`, with the warnings to print: a
/// `[cache]` table outside the root configuration, an environment value that does not parse.
pub fn resolve(flags: &CacheFlags, env: &CacheEnv, base: &Path, targets: &[Target]) -> (Settings, Vec<String>) {
    let mut warnings = Vec::new();
    let flag = Layer {
        enabled: if flags.no_cache {
            Some(false)
        } else {
            flags.cache.then_some(true)
        },
        location: flags.location.as_deref().map(absolute),
        max_size: flags.max_size,
    };
    let variable = Layer {
        enabled: env.enabled.as_ref().and_then(|value| {
            let text = value.to_string_lossy();
            match text.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Some(true),
                "0" | "false" | "no" | "off" => Some(false),
                _ => {
                    warnings.push(format!(
                        "ignoring SKIF_CACHE={}: expected 1 or 0",
                        escape_controls(&text)
                    ));
                    None
                }
            }
        }),
        location: env.dir.as_deref().map(absolute),
        max_size: env.max_size.as_ref().and_then(|value| {
            parse_byte_size(&value.to_string_lossy())
                .inspect_err(|error| warnings.push(format!("ignoring SKIF_CACHE_MAX_SIZE: {}", escape_controls(error))))
                .ok()
        }),
    };
    let config = root_config(base, targets, &mut warnings);
    let enabled = flag.enabled.or(variable.enabled).or(config.enabled);
    // What the user of this run named. The configuration is a file of the project: it may place the store, but it
    // neither makes a `CI` run write nor reaches the stores of other projects.
    let named = flag.location.or(variable.location);
    let explicit = named.is_some();
    let location = named.or(config.location);
    let own_cap = flag.max_size.or(variable.max_size);
    let settings = Settings {
        enabled: enabled.unwrap_or(true),
        asked: enabled == Some(true) || location.is_some(),
        // With `CI` set and no location given, nothing would restore what the run wrote.
        write: !env.ci || explicit,
        location: location.or_else(|| default_location(env, base)),
        cap: if own_cap.is_some() || config.max_size.is_none() {
            CapScope::Location
        } else {
            CapScope::Store
        },
        max_size: own_cap.or(config.max_size).unwrap_or(DEFAULT_MAX_SIZE),
    };
    (settings, warnings)
}

/// The location a run uses when none is given: `$TEST_TMPDIR/skif` under `bazel test`, else as
/// [`skif_cache::default_location`] chooses below `base` or in the per-user cache directory.
fn default_location(env: &CacheEnv, base: &Path) -> Option<PathBuf> {
    if let Some(dir) = &env.bazel {
        return Some(dir.join("skif"));
    }
    skif_cache::default_location(base, env.user_cache_dir.as_deref())
}

/// `[cache]` of the root configuration: the configuration of the target rooted at `base`, as set in its own file or
/// the `--config` file. A `[cache]` key set in any other file (one reached through `extends`, a file above the root,
/// the configuration of a nested root) is ignored with a warning naming the file, and so is a `location` that the
/// file system resolves outside the directory of the file that sets it.
fn root_config(base: &Path, targets: &[Target], warnings: &mut Vec<String>) -> Layer {
    const KEYS: [&str; 3] = ["cache.enabled", "cache.location", "cache.max-size"];
    let root = targets.iter().find(|target| target.root == base);
    let own_files: BTreeSet<&Path> = root.map_or_default(|target| {
        target
            .sources
            .iter()
            .filter(|source| matches!(source.kind, SourceKind::Root | SourceKind::Explicit))
            .map(|source| source.path.as_path())
            .collect()
    });
    let mut ignored: BTreeSet<&Path> = BTreeSet::new();
    for target in targets {
        for key in KEYS {
            for assignment in target.origins.get(key) {
                if let Some(source) = target.sources.get(assignment.source)
                    && !own_files.contains(source.path.as_path())
                {
                    ignored.insert(&source.path);
                }
            }
        }
    }
    for file in ignored {
        warnings.push(format!(
            "ignoring [cache] in {}: only the root configuration sets the cache",
            escape_controls(&file.display().to_string())
        ));
    }
    let Some(root) = root else {
        return Layer::default();
    };
    // The file that set `key` last, when it is the root's own.
    let own = |key: &str| {
        let assignment = root.origins.last(key)?;
        let source = root.sources.get(assignment.source)?;
        own_files
            .contains(source.path.as_path())
            .then_some(source.path.as_path())
    };
    let cache = &root.config.cache;
    Layer {
        enabled: own("cache.enabled").and(cache.enabled),
        location: own("cache.location").and_then(|file| {
            let dir = file.parent().unwrap_or(base);
            let location = skif::join_lexical(dir, Path::new(cache.location.as_deref()?));
            if stays_inside(dir, &location) {
                return Some(location);
            }
            warnings.push(format!(
                "ignoring cache.location in {}: {} is outside the directory of the file",
                escape_controls(&file.display().to_string()),
                escape_controls(&location.display().to_string())
            ));
            None
        }),
        max_size: own("cache.max-size").and(cache.max_size.map(|size| size.0)),
    }
}

/// Whether `location`, as the configuration in `dir` names it, is inside `dir`: as written, and as the file system
/// resolves the part of it that exists, so a link checked in below `dir` cannot lead the store elsewhere.
fn stays_inside(dir: &Path, location: &Path) -> bool {
    if !location.starts_with(dir) {
        return false;
    }
    let Some(existing) = location
        .ancestors()
        .find(|ancestor| ancestor.symlink_metadata().is_ok())
    else {
        return false;
    };
    match (std::fs::canonicalize(dir), std::fs::canonicalize(existing)) {
        (Ok(dir), Ok(existing)) => existing.starts_with(dir),
        _ => false,
    }
}

/// The opaque salt of every key the command line writes. A release build adds nothing beyond the version skif puts in
/// every key; a development build adds the size and modification time of its executable, so rebuilding with changed
/// parse code at the same version never reads entries of the previous build.
fn host_salt() -> Vec<u8> {
    if !cfg!(debug_assertions) {
        return Vec::new();
    }
    let Ok(metadata) = std::env::current_exe().and_then(std::fs::metadata) else {
        return Vec::new();
    };
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    let mut salt = b"dev-build".to_vec();
    salt.extend_from_slice(&metadata.len().to_le_bytes());
    salt.extend_from_slice(&modified.to_le_bytes());
    salt
}

/// The cache a run lints through: a module cache of the host's, a store, or both.
#[derive(Default)]
pub struct Attached {
    /// A module cache in front of the store (the memory of a long-lived host).
    pub modules: Option<Arc<dyn ModuleCache>>,
    /// The store the linters are given, with the salt of [`host_salt`].
    pub store: Option<(Arc<dyn CacheStore>, Vec<u8>)>,
}

/// Where a run's cache comes from and what happens to it once the run has finished.
pub trait CacheHost: Sync {
    /// The cache of a run over `targets` whose report paths are relative to `base`.
    fn attach(&self, base: &Path, targets: &[Target]) -> Attached;
    /// Called once the run has finished, whatever its outcome. `complete` says the run linted everything it was
    /// asked to: such a run writes its entries and drops the ones it did not use. Any other run only adds what it
    /// computed, since what it did not reach says nothing about what the next run needs.
    fn finish(&self, complete: bool);
}

/// An in-memory cache kept by a long-lived host and pruned to what the last run used.
impl CacheHost for Arc<MemoryModuleCache> {
    fn attach(&self, _base: &Path, _targets: &[Target]) -> Attached {
        Attached {
            modules: Some(Arc::clone(self) as Arc<dyn ModuleCache>),
            store: None,
        }
    }

    fn finish(&self, complete: bool) {
        if complete {
            self.prune_unused();
        }
    }
}

/// The store of a lint run or of every round of `--watch`, opened when the first run attaches, with the settings
/// that run resolves, and written when a run finishes.
pub struct DiskHost {
    flags: CacheFlags,
    env: CacheEnv,
    /// `--debug`: report what the store did on stderr.
    debug: bool,
    /// `--watch`: with the cache off, the rounds still share a module cache in memory.
    keep_memory: bool,
    opened: OnceLock<Opened>,
}

/// What a [`DiskHost`] caches through.
enum Opened {
    /// The cache is off and the host runs once.
    Off,
    /// The cache is off and the rounds of a watcher share this.
    Memory(Arc<MemoryModuleCache>),
    Store {
        store: Arc<dyn CacheStore>,
        /// The store on disk; `None` when the run caches in memory.
        disk: Option<Arc<DiskStore>>,
        settings: Settings,
    },
}

impl DiskHost {
    pub const fn new(flags: CacheFlags, env: CacheEnv, debug: bool) -> Self {
        Self {
            flags,
            env,
            debug,
            keep_memory: false,
            opened: OnceLock::new(),
        }
    }

    /// For a host that runs again and again (`--watch`): with the cache off, its runs share a module cache in memory.
    pub const fn keeping_memory(mut self) -> Self {
        self.keep_memory = true;
        self
    }

    fn open(&self, base: &Path, targets: &[Target]) -> Opened {
        let (settings, warnings) = resolve(&self.flags, &self.env, base, targets);
        for warning in warnings {
            stderr_line(&format!("warning: {warning}"));
        }
        if !settings.enabled {
            return if self.keep_memory {
                Opened::Memory(Arc::new(MemoryModuleCache::new()))
            } else {
                Opened::Off
            };
        }
        let disk = settings.location.as_ref().map(|location| {
            let options = DiskStoreOptions::default()
                .with_max_size(settings.max_size)
                .with_write(settings.write)
                .with_evict_others(settings.cap == CapScope::Location);
            Arc::new(DiskStore::open(location, base, options))
        });
        if self.debug {
            match &disk {
                Some(disk) => stderr_line(&format!(
                    "skif debug: cache store {}{}",
                    escape_controls(&disk.dir().display().to_string()),
                    if settings.write { "" } else { " (read only)" }
                )),
                None => stderr_line("skif debug: cache in memory for this run: no cache location"),
            }
        }
        let store: Arc<dyn CacheStore> = match &disk {
            Some(disk) => Arc::clone(disk) as Arc<dyn CacheStore>,
            None => Arc::new(MemoryStore::new()),
        };
        Opened::Store { store, disk, settings }
    }
}

impl CacheHost for DiskHost {
    fn attach(&self, base: &Path, targets: &[Target]) -> Attached {
        match self.opened.get_or_init(|| self.open(base, targets)) {
            Opened::Off => Attached::default(),
            Opened::Memory(modules) => Attached {
                modules: Some(Arc::clone(modules) as Arc<dyn ModuleCache>),
                store: None,
            },
            Opened::Store { store, .. } => Attached {
                modules: None,
                store: Some((Arc::clone(store), host_salt())),
            },
        }
    }

    fn finish(&self, complete: bool) {
        let (disk, settings) = match self.opened.get() {
            Some(Opened::Memory(modules)) => {
                if complete {
                    modules.prune_unused();
                }
                return;
            }
            Some(Opened::Store {
                disk: Some(disk),
                settings,
                ..
            }) => (disk, settings),
            _ => return,
        };
        let result = if complete {
            disk.finish_run()
        } else {
            disk.finish_failed_run()
        };
        if self.debug {
            for note in disk.take_notes() {
                stderr_line(&format!("skif debug: {}", escape_controls(&note)));
            }
        }
        let location = escape_controls(&disk.location().display().to_string()).into_owned();
        match result {
            Ok(Finish::Contended) if self.debug => {
                stderr_line(&format!(
                    "skif debug: another run is writing the cache in {location}; not written"
                ));
            }
            Err(error) if settings.asked => stderr_line(&format!(
                "warning: cannot write the cache in {location}: {}",
                escape_controls(&error.to_string())
            )),
            Err(error) if self.debug => stderr_line(&format!(
                "skif debug: cannot write the cache in {location}: {}",
                escape_controls(&error.to_string())
            )),
            Ok(_) | Err(_) => {}
        }
    }
}

/// A memory cache of one root, its misses read from the store on disk and never written to it: the cache of
/// `skif lsp` and `skif mcp`.
///
/// The seed serves the root's first lint and is dropped with the first [`prune`](Self::prune): by then the memory
/// holds every module the lint parsed, and the server would otherwise keep the whole store file for its life next
/// to it. A later miss (an edited file) is parsed.
pub struct Seeded {
    memory: MemoryModuleCache,
    seed: RwLock<Option<StoreModuleCache>>,
}

impl Seeded {
    /// The memory cache of a root linted with `targets` from `base`, seeded from the store the settings name, when
    /// the cache is on and has a location.
    fn new(env: &CacheEnv, base: &Path, targets: &[Target]) -> Self {
        let (settings, _) = resolve(&CacheFlags::default(), env, base, targets);
        let seed = settings.location.filter(|_| settings.enabled).map(|location| {
            let options = DiskStoreOptions::default()
                .with_write(false)
                .with_max_size(settings.max_size);
            let store = DiskStore::open(&location, base, options);
            StoreModuleCache::new(CacheHandle::new(Arc::new(store), salt_digest(&host_salt()))).read_only()
        });
        Self {
            memory: MemoryModuleCache::new(),
            seed: RwLock::new(seed),
        }
    }

    /// Drops the entries the last lint did not use, and the seed, which has served.
    pub fn prune(&self) {
        self.memory.prune_unused();
        self.seed.write().unwrap_or_else(PoisonError::into_inner).take();
    }

    /// Whether the store on disk still backs the memory.
    #[cfg(test)]
    pub fn is_seeded(&self) -> bool {
        self.seed.read().unwrap_or_else(PoisonError::into_inner).is_some()
    }
}

impl ModuleCache for Seeded {
    fn get(&self, key: ModuleKey) -> Option<CachedModule> {
        if let Some(found) = self.memory.get(key) {
            return Some(found);
        }
        let found = self
            .seed
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()?
            .get(key)?;
        self.memory.put(key, found.clone());
        Some(found)
    }

    fn get_again(&self, key: ModuleKey) -> Option<CachedModule> {
        // The seed counts nothing a run reports, so a second look is a first one.
        self.get(key)
    }

    fn put(&self, key: ModuleKey, module: CachedModule) {
        self.memory.put(key, module);
    }
}

/// The seeded memory cache of one lint a long-lived server repeats (`skif mcp`), pruned after each run to what the
/// run used.
#[derive(Default)]
pub struct SeededHost {
    env: CacheEnv,
    cache: OnceLock<Arc<Seeded>>,
}

impl SeededHost {
    pub const fn new(env: CacheEnv) -> Self {
        Self {
            env,
            cache: OnceLock::new(),
        }
    }
}

impl CacheHost for SeededHost {
    fn attach(&self, base: &Path, targets: &[Target]) -> Attached {
        let cache = self
            .cache
            .get_or_init(|| Arc::new(Seeded::new(&self.env, base, targets)));
        Attached {
            modules: Some(Arc::clone(cache) as Arc<dyn ModuleCache>),
            store: None,
        }
    }

    fn finish(&self, complete: bool) {
        if complete && let Some(cache) = self.cache.get() {
            cache.prune();
        }
    }
}

/// The seeded memory caches of a long-lived server that lints many roots (`skif lsp`), one per root.
#[derive(Default)]
pub struct SeededCaches {
    env: CacheEnv,
    caches: Mutex<BTreeMap<PathBuf, Arc<Seeded>>>,
}

impl SeededCaches {
    pub const fn new(env: CacheEnv) -> Self {
        Self {
            env,
            caches: Mutex::new(BTreeMap::new()),
        }
    }

    /// Prunes the cache of `base` to what its last lint used.
    pub fn prune(&self, base: &Path) {
        if let Some(cache) = self.caches.lock().unwrap_or_else(PoisonError::into_inner).get(base) {
            cache.prune();
        }
    }
}

impl CacheHost for SeededCaches {
    fn attach(&self, base: &Path, targets: &[Target]) -> Attached {
        let cache = {
            let mut caches = self.caches.lock().unwrap_or_else(PoisonError::into_inner);
            Arc::clone(
                caches
                    .entry(base.to_path_buf())
                    .or_insert_with(|| Arc::new(Seeded::new(&self.env, base, targets))),
            )
        };
        Attached {
            modules: Some(cache as Arc<dyn ModuleCache>),
            store: None,
        }
    }

    /// The server prunes the cache of the root it linted; see [`SeededCaches::prune`].
    fn finish(&self, _complete: bool) {}
}

/// The directories a run writes the cache to that `--watch` must leave out of its scan: the location the flags or the
/// environment name, as given and as the file system resolves it. A store directory skif creates holds a `.gitignore`
/// of `*`, which the scan honours, so a location from `[cache]` or a default one is left out as well.
pub fn written_dirs(flags: &CacheFlags, env: &CacheEnv) -> Vec<PathBuf> {
    if flags.no_cache {
        return Vec::new();
    }
    flags
        .location
        .iter()
        .chain(env.dir.iter())
        .flat_map(|dir| {
            let dir = absolute(dir);
            let resolved = std::fs::canonicalize(&dir)
                .ok()
                .map(crate::lint::simplified)
                .filter(|resolved| *resolved != dir);
            std::iter::once(dir).chain(resolved)
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
