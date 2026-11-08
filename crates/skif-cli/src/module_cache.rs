//! The module caches of the command line (ADR 0066): `--cache` persists parse results in `modules.bin`, and the
//! long-lived hosts (`--watch`, `skif lsp`, `skif mcp`) keep theirs in memory.
//!
//! `modules.bin` is one file:
//!
//! ```text
//! magic    8 bytes   "SKIFMOD\0"
//! format   u32 LE    1
//! version  u32 LE length, then UTF-8: skif::module_cache_version()
//! count    u32 LE
//! records  count times: key [u8; 16], length u32 LE, payload (serde_json of CachedModule)
//! ```
//!
//! Records are sorted by key, so the file is a deterministic function of its entries. A file that does not match
//! this layout in every byte is ignored as a whole and replaced on the next write; a payload that does not decode is
//! a miss. Payloads are decoded on demand, so a run pays only for the hits it uses.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock};

use skif::{CachedModule, MemoryModuleCache, ModuleCache, ModuleKey};
use skif_report::escape_controls;

use crate::args::LintArgs;
use crate::output::stderr_line;

/// The first bytes of `modules.bin`.
const MAGIC: [u8; 8] = *b"SKIFMOD\0";
/// The layout of the records; raised when it changes, for example to a binary payload codec.
const FORMAT: u32 = 1;
/// The name of the store in the cache directory.
pub const FILE_NAME: &str = "modules.bin";

/// Where a run's module cache comes from and what happens to it once the run has finished.
pub trait CacheHost: Sync {
    /// The cache for a run whose report paths are relative to `base`.
    fn modules(&self, base: &Path) -> Arc<dyn ModuleCache>;
    /// Called once the run has finished, whatever its outcome: persists or prunes the entries.
    fn finish(&self);
}

/// An in-memory cache kept by a long-lived host and pruned to what the last run used.
impl CacheHost for Arc<MemoryModuleCache> {
    fn modules(&self, _base: &Path) -> Arc<dyn ModuleCache> {
        Arc::clone(self) as Arc<dyn ModuleCache>
    }

    fn finish(&self) {
        self.prune_unused();
    }
}

/// `--cache`: the store in `--cache-location`, or in `node_modules/.cache/skif` below the run's base, loaded when
/// the first run asks for it and written after each run.
pub struct Persistent {
    /// `--cache-location`, absolute; `None` for the default below the base.
    location: Option<PathBuf>,
    /// `--debug`: say on stderr why a store was ignored.
    debug: bool,
    store: OnceLock<Arc<DiskModuleCache>>,
}

impl Persistent {
    pub fn new(args: &LintArgs) -> Self {
        Self {
            location: args
                .cache_location
                .as_ref()
                .map(|dir| std::path::absolute(dir).unwrap_or_else(|_| dir.clone())),
            debug: args.debug,
            store: OnceLock::new(),
        }
    }
}

/// The directory `--cache` uses without `--cache-location`.
pub fn default_location(base: &Path) -> PathBuf {
    base.join("node_modules").join(".cache").join("skif")
}

impl CacheHost for Persistent {
    fn modules(&self, base: &Path) -> Arc<dyn ModuleCache> {
        let store = self.store.get_or_init(|| {
            let dir = self.location.clone().unwrap_or_else(|| default_location(base));
            Arc::new(DiskModuleCache::load(&dir, self.debug))
        });
        Arc::clone(store) as Arc<dyn ModuleCache>
    }

    fn finish(&self) {
        let Some(store) = self.store.get() else {
            return;
        };
        if let Err(error) = store.finish_run() {
            stderr_line(&format!(
                "warning: cannot write the module cache to {}: {error}",
                escape_controls(&store.file.display().to_string())
            ));
        }
    }
}

/// The records of a loaded store: the file's bytes and where each payload lies in them.
#[derive(Default)]
struct Stored {
    bytes: Vec<u8>,
    index: BTreeMap<ModuleKey, Range<usize>>,
}

/// A [`ModuleCache`] backed by `modules.bin`.
pub struct DiskModuleCache {
    file: PathBuf,
    stored: RwLock<Stored>,
    /// The entries this run parsed, encoded.
    fresh: Mutex<BTreeMap<ModuleKey, Arc<[u8]>>>,
    /// The keys this run read or wrote: what the next write keeps.
    used: Mutex<BTreeSet<ModuleKey>>,
}

impl DiskModuleCache {
    /// The store in `dir`. A missing file is an empty store; a file that cannot be read or does not match the layout
    /// is ignored, with the reason on stderr when `debug` is set.
    pub fn load(dir: &Path, debug: bool) -> Self {
        let file = dir.join(FILE_NAME);
        let stored = match std::fs::read(&file) {
            Ok(bytes) => decode(bytes).unwrap_or_else(|reason| {
                note(debug, &file, reason);
                Stored::default()
            }),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Stored::default(),
            Err(error) => {
                note(debug, &file, &error.to_string());
                Stored::default()
            }
        };
        Self {
            file,
            stored: RwLock::new(stored),
            fresh: Mutex::new(BTreeMap::new()),
            used: Mutex::new(BTreeSet::new()),
        }
    }

    fn mark(&self, key: ModuleKey) {
        self.used.lock().unwrap_or_else(PoisonError::into_inner).insert(key);
    }

    /// Writes the entries the run used, unless they are exactly the entries already stored, and forgets the others.
    /// A run that used no entry (no JS analysis ran) leaves the store as it is.
    ///
    /// # Errors
    /// Creating the directory, writing the temporary file or renaming it over the store failed. The in-memory state
    /// is pruned all the same.
    pub fn finish_run(&self) -> io::Result<()> {
        let used = std::mem::take(&mut *self.used.lock().unwrap_or_else(PoisonError::into_inner));
        let fresh = std::mem::take(&mut *self.fresh.lock().unwrap_or_else(PoisonError::into_inner));
        if used.is_empty() {
            return Ok(());
        }
        let mut stored = self.stored.write().unwrap_or_else(PoisonError::into_inner);
        if fresh.is_empty() && used.iter().eq(stored.index.keys()) {
            return Ok(());
        }
        let records = used.iter().filter_map(|key| {
            let payload = match fresh.get(key) {
                Some(payload) => &payload[..],
                None => &stored.bytes[stored.index.get(key)?.clone()],
            };
            Some((*key, payload))
        });
        let bytes = encode(records);
        *stored = decode(bytes.clone()).unwrap_or_default();
        drop(stored);
        write_atomically(&self.file, &bytes)
    }
}

impl ModuleCache for DiskModuleCache {
    fn get(&self, key: ModuleKey) -> Option<CachedModule> {
        let fresh = self
            .fresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned();
        let module = if let Some(payload) = fresh {
            serde_json::from_slice(&payload).ok()?
        } else {
            let stored = self.stored.read().unwrap_or_else(PoisonError::into_inner);
            let range = stored.index.get(&key)?.clone();
            serde_json::from_slice(&stored.bytes[range]).ok()?
        };
        self.mark(key);
        Some(module)
    }

    fn put(&self, key: ModuleKey, module: CachedModule) {
        let Ok(payload) = serde_json::to_vec(&module) else {
            return;
        };
        self.fresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(key, Arc::from(payload));
        self.mark(key);
    }
}

fn note(debug: bool, file: &Path, reason: &str) {
    if debug {
        stderr_line(&format!(
            "skif: ignoring the module cache {}: {reason}",
            escape_controls(&file.display().to_string())
        ));
    }
}

/// The bytes of a store holding `records`, which come sorted by key.
fn encode<'a>(records: impl Iterator<Item = (ModuleKey, &'a [u8])>) -> Vec<u8> {
    let version = skif::module_cache_version();
    let mut body = Vec::new();
    let mut count: u32 = 0;
    for (key, payload) in records {
        // A payload over 4 GiB cannot be framed; leaving it out is a miss on the next run.
        let Ok(length) = u32::try_from(payload.len()) else {
            continue;
        };
        body.extend_from_slice(&key.to_bytes());
        body.extend_from_slice(&length.to_le_bytes());
        body.extend_from_slice(payload);
        count += 1;
    }
    let mut bytes = Vec::with_capacity(body.len() + 32 + version.len());
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&FORMAT.to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(version.len()).unwrap_or(u32::MAX).to_le_bytes());
    bytes.extend_from_slice(version.as_bytes());
    bytes.extend_from_slice(&count.to_le_bytes());
    bytes.extend_from_slice(&body);
    bytes
}

/// Reads bytes off the front of a store, every length bounded by what is left.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn take(&mut self, length: usize) -> Result<Range<usize>, &'static str> {
        let end = self
            .at
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or("truncated")?;
        let range = self.at..end;
        self.at = end;
        Ok(range)
    }

    fn u32(&mut self) -> Result<u32, &'static str> {
        let range = self.take(4)?;
        let mut word = [0; 4];
        word.copy_from_slice(&self.bytes[range]);
        Ok(u32::from_le_bytes(word))
    }
}

/// Indexes the records of a store without decoding them, or says why the bytes are not a store of this version.
fn decode(bytes: Vec<u8>) -> Result<Stored, &'static str> {
    let mut reader = Reader { bytes: &bytes, at: 0 };
    if bytes.get(reader.take(MAGIC.len())?) != Some(&MAGIC[..]) {
        return Err("not a module cache");
    }
    if reader.u32()? != FORMAT {
        return Err("another format");
    }
    let length = reader.u32()? as usize;
    let version = reader.take(length)?;
    if bytes[version] != *skif::module_cache_version().as_bytes() {
        return Err("written by another version");
    }
    let count = reader.u32()?;
    let mut index = BTreeMap::new();
    let mut last: Option<ModuleKey> = None;
    for _ in 0..count {
        let mut key = [0; 16];
        key.copy_from_slice(&bytes[reader.take(16)?]);
        let key = ModuleKey::from_bytes(key);
        if last.is_some_and(|last| last >= key) {
            return Err("records out of order");
        }
        last = Some(key);
        let length = reader.u32()? as usize;
        index.insert(key, reader.take(length)?);
    }
    if reader.at != bytes.len() {
        return Err("bytes after the last record");
    }
    Ok(Stored { bytes, index })
}

/// Writes `bytes` to `file` through a temporary file in the same directory, so a concurrent reader sees the old file
/// or the new one, never a part.
fn write_atomically(file: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = file.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let temporary = temporary_file(file);
    let written = std::fs::write(&temporary, bytes).and_then(|()| std::fs::rename(&temporary, file));
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

/// The temporary file a write of `file` by this process goes through.
pub fn temporary_file(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    file.with_file_name(name)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::time::SystemTime;

    use skif::{MemoryFs, ModuleCache};

    use super::*;

    /// A real parse result to store: the facade exposes [`CachedModule`] only through a cache.
    fn module(text: &str) -> (ModuleKey, CachedModule) {
        let fs = Arc::new(
            MemoryFs::new()
                .with_file(
                    "/ws/package.json",
                    r#"{"name": "ws", "version": "1.0.0", "main": "a.js"}"#,
                )
                .with_file("/ws/a.js", text),
        );
        let config = skif::Config::default();
        let workspace = skif::Workspace::discover_with_fs(Path::new("/ws"), &config, &*fs).unwrap();
        let recorder = Arc::new(Recorder::default());
        let linter =
            skif::Linter::with_module_cache(&config, fs, Arc::clone(&recorder) as Arc<dyn ModuleCache>).unwrap();
        linter.run(&workspace).unwrap();
        let mut entries = recorder.0.lock().unwrap();
        assert_eq!(entries.len(), 1);
        entries.pop_first().unwrap()
    }

    #[derive(Default)]
    struct Recorder(Mutex<BTreeMap<ModuleKey, CachedModule>>);

    impl ModuleCache for Recorder {
        fn get(&self, key: ModuleKey) -> Option<CachedModule> {
            self.0.lock().unwrap().get(&key).cloned()
        }

        fn put(&self, key: ModuleKey, module: CachedModule) {
            self.0.lock().unwrap().insert(key, module);
        }
    }

    fn file(dir: &Path) -> PathBuf {
        dir.join(FILE_NAME)
    }

    fn modified(dir: &Path) -> SystemTime {
        std::fs::metadata(file(dir)).unwrap().modified().unwrap()
    }

    #[test]
    fn a_written_store_loads_with_the_same_entries() {
        let dir = tempfile::tempdir().unwrap();
        let (a, module_a) = module("export const a = 1;\n");
        let (b, module_b) = module("export const b = 2;\n");
        let cache = DiskModuleCache::load(dir.path(), false);
        cache.put(b, module_b.clone());
        cache.put(a, module_a.clone());
        cache.finish_run().unwrap();
        assert!(std::fs::read(file(dir.path())).unwrap().starts_with(&MAGIC));

        let loaded = DiskModuleCache::load(dir.path(), false);
        assert_eq!(loaded.get(a), Some(module_a));
        assert_eq!(loaded.get(b), Some(module_b));
        assert_eq!(
            loaded.stored.read().unwrap().index.keys().copied().collect::<Vec<_>>(),
            {
                let mut keys = vec![a, b];
                keys.sort();
                keys
            }
        );
    }

    #[test]
    fn a_fully_warm_run_does_not_rewrite_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let (a, module_a) = module("export const a = 1;\n");
        let cache = DiskModuleCache::load(dir.path(), false);
        cache.put(a, module_a);
        cache.finish_run().unwrap();
        let before = std::fs::read(file(dir.path())).unwrap();
        // Make the store's timestamp distinguishable from a rewrite.
        let stamp = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        std::fs::File::options()
            .write(true)
            .open(file(dir.path()))
            .unwrap()
            .set_modified(stamp)
            .unwrap();

        let warm = DiskModuleCache::load(dir.path(), false);
        assert!(warm.get(a).is_some());
        warm.finish_run().unwrap();
        assert_eq!(modified(dir.path()), stamp);
        assert_eq!(std::fs::read(file(dir.path())).unwrap(), before);
        // A run that used nothing (no JS analysis) leaves the store alone too.
        DiskModuleCache::load(dir.path(), false).finish_run().unwrap();
        assert_eq!(modified(dir.path()), stamp);
    }

    #[test]
    fn a_write_keeps_exactly_the_entries_the_run_used() {
        let dir = tempfile::tempdir().unwrap();
        let (a, module_a) = module("export const a = 1;\n");
        let (b, module_b) = module("export const b = 2;\n");
        let (c, module_c) = module("export const c = 3;\n");
        let first = DiskModuleCache::load(dir.path(), false);
        first.put(a, module_a);
        first.put(b, module_b);
        first.finish_run().unwrap();

        let second = DiskModuleCache::load(dir.path(), false);
        assert!(second.get(a).is_some());
        second.put(c, module_c);
        second.finish_run().unwrap();
        // The in-memory state is pruned like the file.
        assert!(second.get(b).is_none());

        let third = DiskModuleCache::load(dir.path(), false);
        assert!(third.get(a).is_some());
        assert!(third.get(b).is_none());
        assert!(third.get(c).is_some());
    }

    #[test]
    fn a_damaged_or_foreign_store_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let (a, module_a) = module("export const a = 1;\n");
        let (b, module_b) = module("export const b = 2;\n");
        let cache = DiskModuleCache::load(dir.path(), false);
        cache.put(a, module_a);
        cache.put(b, module_b);
        cache.finish_run().unwrap();
        let good = std::fs::read(file(dir.path())).unwrap();
        assert!(decode(good.clone()).is_ok());

        let version_at = MAGIC.len() + 8;
        let mut wrong_magic = good.clone();
        wrong_magic[0] = b'X';
        let mut wrong_format = good.clone();
        wrong_format[MAGIC.len()] = 9;
        let mut wrong_version = good.clone();
        wrong_version[version_at] ^= 1;
        let mut long_length = good.clone();
        let records_at = version_at + skif::module_cache_version().len() + 4;
        long_length[records_at + 16..records_at + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        // The two records swapped: keys out of order.
        let first_length = u32::from_le_bytes(good[records_at + 16..records_at + 20].try_into().unwrap()) as usize;
        let second_at = records_at + 20 + first_length;
        let mut unsorted = good[..records_at].to_vec();
        unsorted.extend_from_slice(&good[second_at..]);
        unsorted.extend_from_slice(&good[records_at..second_at]);
        let mut trailing = good.clone();
        trailing.push(0);
        for (name, bytes) in [
            ("truncated", good[..good.len() - 1].to_vec()),
            ("empty", Vec::new()),
            ("wrong magic", wrong_magic),
            ("wrong format", wrong_format),
            ("wrong version", wrong_version),
            ("length past the end", long_length),
            ("unsorted", unsorted),
            ("trailing bytes", trailing),
        ] {
            std::fs::write(file(dir.path()), &bytes).unwrap();
            assert!(decode(bytes).is_err(), "{name}");
            let loaded = DiskModuleCache::load(dir.path(), false);
            assert!(loaded.get(a).is_none(), "{name}");
            assert!(loaded.stored.read().unwrap().index.is_empty(), "{name}");
        }
    }

    #[test]
    fn a_payload_that_does_not_decode_is_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let (a, _) = module("export const a = 1;\n");
        std::fs::write(file(dir.path()), encode([(a, &b"not json"[..])].into_iter())).unwrap();
        let cache = DiskModuleCache::load(dir.path(), false);
        assert_eq!(cache.stored.read().unwrap().index.len(), 1);
        assert!(cache.get(a).is_none());
    }

    #[test]
    fn a_write_failure_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the cache directory should be.
        let blocked = dir.path().join("blocked");
        std::fs::write(&blocked, "").unwrap();
        let (a, module_a) = module("export const a = 1;\n");
        let cache = DiskModuleCache::load(&blocked, false);
        cache.put(a, module_a);
        assert!(cache.finish_run().is_err());
    }
}
