//! skif's disk cache store (ADR 0007): a [`CacheStore`] over one cache location that holds a store per base.
//!
//! This crate is an implementation detail of `skif`, re-exported as `skif::cache` behind the `cache-disk` feature;
//! it carries no stability promise of its own. It is the one skif library crate that reads and writes files outside
//! the `FileSystem` trait, and only below the cache location it is given. It reads no environment: the host passes
//! the location and, for [`default_location`], the platform's per-user cache directory.
//!
//! # Layout
//!
//! ```text
//! <location>/
//!   CACHEDIR.TAG, .gitignore      written when skif creates the directory
//!   <base key>/                   one store per base (32 hex digits, see `base_key`)
//!     base                        the base it belongs to, for `skif cache stats`
//!     .lock                       the try-lock of a write
//!     <namespace>.bin             one file per namespace (`modules.bin`)
//!     <file>.<random>.tmp         a write in progress; one a killed writer left goes with the next write
//! ```
//!
//! A namespace file is:
//!
//! ```text
//! magic    8 bytes   "SKIFSTOR"
//! format   u32 LE    2
//! name     u8 length, then the namespace name
//! schema   u32 LE    the namespace schema it was written under
//! count    u32 LE
//! records  count times, sorted by key: key [u8; 32], length u32 LE, value (skif's sealed envelope)
//! ```
//!
//! A file that does not match this layout in every byte, or was written under another schema, is empty; skif
//! verifies every value it reads, so a damaged record is a miss.
//!
//! # Policy
//!
//! * A base inside a git repository is identified by the git directory its worktrees share and its path in the
//!   repository, so the worktrees of one repository share a store and two repositories never do; a base outside git
//!   is identified by its absolute path.
//! * [`DiskStore::finish_run`] writes, per namespace, exactly the entries the run used, so a run never prunes
//!   a namespace it did not ask for, and a namespace the run used in full is not rewritten. Runs that lint different
//!   sets of files in turn (`skif` and `skif --production`) therefore each drop what only the other used.
//!   [`DiskStore::finish_failed_run`], for a run that did not complete, adds what the run computed and drops nothing.
//! * A write takes a try-lock on the store and is skipped when another process holds it; files are written to a
//!   new temporary file with a random name, never through a link, and renamed, so a reader sees the old file or the
//!   new one. Temporary files a killed writer left are removed under the lock before the next write.
//! * A namespace file larger than the cap is not read: no write leaves one, so it is not a file of this store.
//! * After a write, a location over its cap drops the stores of other bases, least recently written first, unless
//!   the host turned that off ([`DiskStoreOptions::with_evict_others`]); a store that alone exceeds the cap keeps
//!   `modules` entries first, then the other namespaces, in key order, up to it.
//! * A directory the store creates gets a `CACHEDIR.TAG` and a `.gitignore` of `*`, written only where no such file
//!   exists, and it never creates a directory named `node_modules`.

#![warn(missing_docs)]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, TryLockError};
use std::hash::{BuildHasher as _, Hasher as _};
use std::io::{self, Read as _, Write as _};
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::SystemTime;

use skif_core::cache::content_digest;
use skif_core::{CacheNamespace, CacheStore};

/// The first bytes of a namespace file.
const MAGIC: [u8; 8] = *b"SKIFSTOR";
/// The layout of a namespace file: 1 was the single `modules.bin` of skif 0.8, 2 is per namespace with sealed values.
const FORMAT: u32 = 2;
/// The extension of a namespace file.
const EXTENSION: &str = "bin";
/// The name of the try-lock file of a store.
const LOCK_FILE: &str = ".lock";
/// The name of the file that records which base a store belongs to.
const BASE_FILE: &str = "base";
/// The name of the cache directory tag written into a directory skif creates.
const TAG_FILE: &str = "CACHEDIR.TAG";
/// The name of the ignore file written into a directory skif creates, and its content.
const IGNORE_FILE: &str = ".gitignore";
const IGNORE_ALL: &str = "*\n";
/// The suffix of the temporary file of a write.
const TEMPORARY_SUFFIX: &str = ".tmp";
/// How many random names a write tries before it gives up.
const TEMPORARY_TRIES: usize = 8;
/// The namespace whose entries a store over the cap keeps first.
const FIRST_NAMESPACE: &str = "modules";

/// The cap of a location when the host sets none: 512 MiB.
pub const DEFAULT_MAX_SIZE: u64 = 512 * 1024 * 1024;

/// The contents of a `CACHEDIR.TAG` (<https://bford.info/cachedir/>).
const CACHEDIR_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n\
                            # This file is a cache directory tag created by skif.\n\
                            # For information about cache directory tags, see https://bford.info/cachedir/\n";

/// How a [`DiskStore`] behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiskStoreOptions {
    /// The most bytes the whole location may hold after a write.
    pub max_size: u64,
    /// Whether [`DiskStore::finish_run`] writes; a read-only store still serves the entries a run puts, until the
    /// run ends.
    pub write: bool,
    /// Whether a write that leaves the location over its cap drops the stores of other bases. A host turns it off
    /// when the cap comes from a file of the project, so a checked-in cap only ever shrinks the project's own store.
    pub evict_others: bool,
}

impl Default for DiskStoreOptions {
    fn default() -> Self {
        Self {
            max_size: DEFAULT_MAX_SIZE,
            write: true,
            evict_others: true,
        }
    }
}

impl DiskStoreOptions {
    /// Caps the location at `max_size` bytes.
    #[must_use]
    pub const fn with_max_size(mut self, max_size: u64) -> Self {
        self.max_size = max_size;
        self
    }

    /// Whether [`DiskStore::finish_run`] writes.
    #[must_use]
    pub const fn with_write(mut self, write: bool) -> Self {
        self.write = write;
        self
    }

    /// Whether a write that leaves the location over its cap drops the stores of other bases.
    #[must_use]
    pub const fn with_evict_others(mut self, evict_others: bool) -> Self {
        self.evict_others = evict_others;
        self
    }
}

/// A store's write failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// A file or directory of the store could not be created, written or renamed.
    #[error("cannot write {path}: {source}")]
    Io {
        /// The file or directory.
        path: PathBuf,
        /// What went wrong.
        #[source]
        source: io::Error,
    },
    /// The location is below a `node_modules` directory that does not exist, which skif never creates.
    #[error("{path} does not exist and skif does not create node_modules directories")]
    NodeModules {
        /// The missing `node_modules` directory.
        path: PathBuf,
    },
    /// The location is the empty path, which would put the store in the current directory.
    #[error("the cache location is empty")]
    EmptyLocation,
}

impl StoreError {
    fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// What [`DiskStore::finish_run`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Finish {
    /// Nothing to write: the run used no entry, or exactly the entries already stored.
    Unchanged,
    /// The namespaces the run used were written.
    Written,
    /// Another process held the store's lock, so this run's write was skipped.
    Contended,
    /// The store is read-only.
    ReadOnly,
}

/// A key and its sealed value.
type Record = ([u8; 32], Arc<[u8]>);

/// The records of one namespace a run used, waiting to be written.
struct Pending {
    name: &'static str,
    schema: u32,
    records: Vec<Record>,
}

/// The records of one namespace file as loaded: its bytes and where each value lies in them.
#[derive(Default)]
struct Stored {
    bytes: Vec<u8>,
    index: BTreeMap<[u8; 32], Range<usize>>,
}

/// One namespace of a store while runs use it.
struct Namespace {
    schema: u32,
    stored: RwLock<Stored>,
    /// The values this run put.
    fresh: Mutex<BTreeMap<[u8; 32], Arc<[u8]>>>,
    /// The keys this run read or wrote: what the next write keeps.
    used: Mutex<BTreeSet<[u8; 32]>>,
}

/// skif's disk cache store: the store of one base in a cache location (see the [crate] documentation).
///
/// Namespace files are read when a run first asks for their namespace and kept in memory;
/// [`finish_run`](Self::finish_run) writes what the run used and prunes the memory to it, so a long-lived host (watch
/// mode) reads the files once and writes them after each run.
pub struct DiskStore {
    location: PathBuf,
    dir: PathBuf,
    base: String,
    options: DiskStoreOptions,
    namespaces: Mutex<BTreeMap<&'static str, Arc<Namespace>>>,
    /// Why files were ignored, for a host that shows them under `--debug`.
    notes: Mutex<Vec<String>>,
}

impl std::fmt::Debug for DiskStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskStore")
            .field("dir", &self.dir)
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl DiskStore {
    /// The store of `base`, an absolute directory, in the cache `location`. Opening reads nothing and never fails:
    /// files are read when a run first asks for their namespace, and a missing or unreadable file is empty.
    #[must_use]
    pub fn open(location: &Path, base: &Path, options: DiskStoreOptions) -> Self {
        let base = base_identity(base);
        let dir = location.join(base_dir_name(&base));
        Self {
            location: location.to_path_buf(),
            dir,
            base,
            options,
            namespaces: Mutex::new(BTreeMap::new()),
            notes: Mutex::new(Vec::new()),
        }
    }

    /// The cache location.
    #[must_use]
    pub fn location(&self) -> &Path {
        &self.location
    }

    /// The directory of this base's store inside the location.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// How the store behaves.
    #[must_use]
    pub const fn options(&self) -> DiskStoreOptions {
        self.options
    }

    /// Takes the reasons files were ignored since the last call, for a host that shows them under `--debug`.
    #[must_use]
    pub fn take_notes(&self) -> Vec<String> {
        std::mem::take(&mut *self.notes.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn note(&self, note: String) {
        self.notes.lock().unwrap_or_else(PoisonError::into_inner).push(note);
    }

    /// The namespace `namespace` of this store, read from its file the first time it is asked for. A namespace asked
    /// for under another schema than before starts empty.
    fn namespace(&self, namespace: CacheNamespace) -> Option<Arc<Namespace>> {
        if !CacheNamespace::is_valid_name(namespace.name) {
            return None;
        }
        let mut namespaces = self.namespaces.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(loaded) = namespaces.get(namespace.name)
            && loaded.schema == namespace.schema
        {
            return Some(Arc::clone(loaded));
        }
        let file = self.dir.join(namespace_file(namespace.name));
        // No write leaves a namespace file over the cap, so a larger one is not read into memory.
        let limit = self.options.max_size.saturating_add(header_size(namespace.name));
        let stored = match read_at_most(&file, limit) {
            Ok(Some(bytes)) => decode(bytes, namespace).unwrap_or_else(|reason| {
                self.note(format!("ignoring the cache file {}: {reason}", file.display()));
                Stored::default()
            }),
            Ok(None) => {
                self.note(format!(
                    "ignoring the cache file {}: larger than the cap of {} bytes",
                    file.display(),
                    self.options.max_size
                ));
                Stored::default()
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Stored::default(),
            Err(error) => {
                self.note(format!("ignoring the cache file {}: {error}", file.display()));
                Stored::default()
            }
        };
        let loaded = Arc::new(Namespace {
            schema: namespace.schema,
            stored: RwLock::new(stored),
            fresh: Mutex::new(BTreeMap::new()),
            used: Mutex::new(BTreeSet::new()),
        });
        namespaces.insert(namespace.name, Arc::clone(&loaded));
        Some(loaded)
    }

    /// Writes, per namespace, exactly the entries the run used, unless they are the entries already stored, then
    /// keeps the location under its cap, and prunes the memory of every namespace to what the run used.
    ///
    /// # Errors
    /// [`StoreError`] when a directory or file could not be created, written or renamed, or the location is below a
    /// missing `node_modules`. The memory is pruned all the same; the run's report never depends on it.
    pub fn finish_run(&self) -> Result<Finish, StoreError> {
        self.finish(true)
    }

    /// Ends a run that did not complete (it failed, or linted only a part of what it was asked to): writes, per
    /// namespace, the entries the run put next to the ones already stored, and drops none, since what such a run did
    /// not use says nothing about what the next one needs.
    ///
    /// # Errors
    /// As [`finish_run`](Self::finish_run).
    pub fn finish_failed_run(&self) -> Result<Finish, StoreError> {
        self.finish(false)
    }

    /// Ends a run; `prune` drops the entries it did not use.
    fn finish(&self, prune: bool) -> Result<Finish, StoreError> {
        let namespaces: Vec<(&'static str, Arc<Namespace>)> = self
            .namespaces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(name, namespace)| (*name, Arc::clone(namespace)))
            .collect();
        let mut pending: Vec<Pending> = Vec::new();
        for &(name, ref namespace) in &namespaces {
            let used = std::mem::take(&mut *namespace.used.lock().unwrap_or_else(PoisonError::into_inner));
            let fresh = std::mem::take(&mut *namespace.fresh.lock().unwrap_or_else(PoisonError::into_inner));
            if used.is_empty() {
                continue;
            }
            let mut stored = namespace.stored.write().unwrap_or_else(PoisonError::into_inner);
            if fresh.is_empty() && (!prune || used.iter().eq(stored.index.keys())) {
                continue;
            }
            let kept: BTreeSet<[u8; 32]> = if prune {
                used
            } else {
                stored.index.keys().chain(fresh.keys()).copied().collect()
            };
            let records: Vec<Record> = kept
                .iter()
                .filter_map(|key| {
                    let value = match fresh.get(key) {
                        Some(value) => Arc::clone(value),
                        None => Arc::from(&stored.bytes[stored.index.get(key)?.clone()]),
                    };
                    Some((*key, value))
                })
                .collect();
            *stored = encode(
                name,
                namespace.schema,
                records.iter().map(|(key, value)| (key, &value[..])),
            );
            drop(stored);
            pending.push(Pending {
                name,
                schema: namespace.schema,
                records,
            });
        }
        if pending.is_empty() {
            return Ok(Finish::Unchanged);
        }
        if !self.options.write {
            return Ok(Finish::ReadOnly);
        }
        self.write(pending)
    }

    /// Writes the namespace files of `pending` under the store's lock, then keeps the location under its cap.
    fn write(&self, mut pending: Vec<Pending>) -> Result<Finish, StoreError> {
        create_tagged(&self.location)?;
        create_tagged(&self.dir)?;
        let Some(_lock) = try_lock(&self.dir)? else {
            return Ok(Finish::Contended);
        };
        // The lock is held, so a temporary file here belongs to a writer that was killed.
        remove_temporaries(&self.dir);
        let base_file = self.dir.join(BASE_FILE);
        if fs::read_to_string(&base_file).ok().as_deref() != Some(self.base.as_str()) {
            write_atomically(&base_file, self.base.as_bytes())?;
        }
        // A store that alone exceeds the cap keeps `modules` first, then the other namespaces in name order.
        pending.sort_by_key(|pending| (pending.name != FIRST_NAMESPACE, pending.name));
        let others: u64 = self
            .namespace_files()
            .iter()
            .filter(|(name, _)| !pending.iter().any(|pending| pending.name == name))
            .map(|(_, size)| size)
            .sum();
        let mut budget = self.options.max_size.saturating_sub(others);
        for Pending { name, schema, records } in &pending {
            let mut kept = Vec::new();
            let mut size = header_size(name);
            for (key, value) in records {
                let record = 32 + 4 + value.len() as u64;
                if size + record > budget {
                    break;
                }
                size += record;
                kept.push((key, &value[..]));
            }
            budget = budget.saturating_sub(size);
            let encoded = encode(name, *schema, kept.into_iter());
            write_atomically(&self.dir.join(namespace_file(name)), &encoded.bytes)?;
        }
        if self.options.evict_others {
            self.enforce_cap();
        }
        Ok(Finish::Written)
    }

    /// The namespace files of this store and their sizes.
    fn namespace_files(&self) -> Vec<(String, u64)> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_stem()?.to_str()?.to_owned();
                (path.extension()? == EXTENSION).then_some(())?;
                Some((name, entry.metadata().ok()?.len()))
            })
            .collect()
    }

    /// Drops the stores of other bases, least recently written first, while the location is over its cap. A store
    /// whose lock another process holds is kept.
    fn enforce_cap(&self) {
        let Ok(stores) = list_stores(&self.location) else {
            return;
        };
        let mut total: u64 = stores.iter().map(|store| store.bytes).sum();
        let mut others: Vec<&StoreDir> = stores
            .iter()
            .filter(|store| store.readable && store.dir != self.dir)
            .collect();
        others.sort_by_key(|store| (store.modified, store.dir.clone()));
        for store in others {
            if total <= self.options.max_size {
                break;
            }
            let Ok(Some(lock)) = try_lock(&store.dir) else {
                continue;
            };
            drop(lock);
            if remove_store(&store.dir).is_ok() {
                total = total.saturating_sub(store.bytes);
            }
        }
    }
}

impl CacheStore for DiskStore {
    fn get(&self, namespace: CacheNamespace, key: &[u8; 32]) -> Option<Arc<[u8]>> {
        let loaded = self.namespace(namespace)?;
        let fresh = loaded
            .fresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .cloned();
        let value = if let Some(value) = fresh {
            value
        } else {
            let stored = loaded.stored.read().unwrap_or_else(PoisonError::into_inner);
            let range = stored.index.get(key)?.clone();
            Arc::from(&stored.bytes[range])
        };
        loaded.used.lock().unwrap_or_else(PoisonError::into_inner).insert(*key);
        Some(value)
    }

    fn put(&self, namespace: CacheNamespace, key: &[u8; 32], value: Arc<[u8]>) {
        let Some(loaded) = self.namespace(namespace) else {
            return;
        };
        loaded
            .fresh
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(*key, value);
        loaded.used.lock().unwrap_or_else(PoisonError::into_inner).insert(*key);
    }
}

/// The base a store belongs to, as its key is made: `git:`, the git directory shared by the worktrees of the
/// repository holding the base, and the base relative to the top level of its checkout, so worktrees of one
/// repository share a key and two repositories never do; else `path:` and the absolute base.
fn base_identity(base: &Path) -> String {
    let Some(top) = base.ancestors().find(|dir| dir.join(".git").exists()) else {
        return format!("path:{}", base.display());
    };
    let parts: Vec<String> = base.strip_prefix(top).map_or_default(|relative| {
        relative
            .components()
            .filter_map(|component| match component {
                Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect()
    });
    format!("git:{}:{}", common_git_dir(top).display(), parts.join("/"))
}

/// The git directory the checkout at `top` shares with the other worktrees of its repository, as the file system
/// resolves it: `.git` itself in the main checkout; in a linked worktree, where `.git` is a file naming the
/// worktree's own git directory, the directory that one's `commondir` names. A submodule's git directory has no
/// `commondir`, so a submodule is a repository of its own.
fn common_git_dir(top: &Path) -> PathBuf {
    let dot_git = top.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        fs::read_to_string(&dot_git)
            .ok()
            .and_then(|text| {
                let named = text.lines().next()?.strip_prefix("gitdir:")?.trim();
                Some(top.join(named))
            })
            .unwrap_or(dot_git)
    };
    let common = fs::read_to_string(git_dir.join("commondir"))
        .map_or_else(|_| git_dir.clone(), |named| git_dir.join(named.trim()));
    fs::canonicalize(&common).unwrap_or(common)
}

/// The directory name of the store of `base`: 32 hex digits of the digest of [`base_identity`].
fn base_dir_name(identity: &str) -> String {
    use std::fmt::Write as _;
    let mut name = String::with_capacity(32);
    for byte in &content_digest(identity.as_bytes())[..16] {
        // Writing to a `String` cannot fail.
        let _ = write!(name, "{byte:02x}");
    }
    name
}

/// The key of the store of `base` in a location, as [`DiskStore::open`] names its directory.
#[must_use]
pub fn base_key(base: &Path) -> String {
    base_dir_name(&base_identity(base))
}

/// The file of namespace `name` in a store.
fn namespace_file(name: &str) -> String {
    format!("{name}.{EXTENSION}")
}

/// The cache location a host uses when the user named none, given the base of the run and the platform's per-user
/// cache directory: `<base>/node_modules/.cache/skif` when `node_modules` exists (skif never creates it), else
/// `<user_cache_dir>/skif`, else `None`, for a cache in memory for the run.
#[must_use]
pub fn default_location(base: &Path, user_cache_dir: Option<&Path>) -> Option<PathBuf> {
    let node_modules = base.join("node_modules");
    if node_modules.is_dir() {
        return Some(node_modules.join(".cache").join("skif"));
    }
    user_cache_dir.map(|dir| dir.join("skif"))
}

/// The size of a namespace file's header.
fn header_size(name: &str) -> u64 {
    (MAGIC.len() + 4 + 1 + name.len().min(usize::from(u8::MAX)) + 4 + 4) as u64
}

/// The namespace file holding `records`, which come sorted by key: its bytes and the index [`decode`] would build.
fn encode<'a>(name: &str, schema: u32, records: impl Iterator<Item = (&'a [u8; 32], &'a [u8])>) -> Stored {
    let name = &name.as_bytes()[..name.len().min(usize::from(u8::MAX))];
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&FORMAT.to_le_bytes());
    #[allow(clippy::cast_possible_truncation)]
    bytes.push(name.len() as u8);
    bytes.extend_from_slice(name);
    bytes.extend_from_slice(&schema.to_le_bytes());
    // The count is known once the records are written; it is filled in then.
    let count_at = bytes.len();
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    let mut index = BTreeMap::new();
    let mut count: u32 = 0;
    for (key, value) in records {
        // A value over 4 GiB cannot be framed; leaving it out is a miss on the next run.
        let Ok(length) = u32::try_from(value.len()) else {
            continue;
        };
        bytes.extend_from_slice(key);
        bytes.extend_from_slice(&length.to_le_bytes());
        let start = bytes.len();
        bytes.extend_from_slice(value);
        index.insert(*key, start..bytes.len());
        count += 1;
    }
    bytes[count_at..count_at + 4].copy_from_slice(&count.to_le_bytes());
    Stored { bytes, index }
}

/// Reads bytes off the front of a file, every length bounded by what is left.
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

    fn u8(&mut self) -> Result<u8, &'static str> {
        let range = self.take(1)?;
        Ok(self.bytes[range.start])
    }
}

/// What a namespace file's header says: its name, schema and record count.
struct Header {
    name: String,
    schema: u32,
    count: u32,
}

fn read_header(reader: &mut Reader<'_>) -> Result<Header, &'static str> {
    if reader.bytes.get(reader.take(MAGIC.len())?) != Some(&MAGIC[..]) {
        return Err("not a skif cache file");
    }
    if reader.u32()? != FORMAT {
        return Err("another format");
    }
    let length = usize::from(reader.u8()?);
    let name = reader.take(length)?;
    let name = String::from_utf8_lossy(&reader.bytes[name]).into_owned();
    let schema = reader.u32()?;
    let count = reader.u32()?;
    Ok(Header { name, schema, count })
}

/// Indexes the records of a namespace file without reading the values, or says why the bytes are not a file of
/// `namespace`.
fn decode(bytes: Vec<u8>, namespace: CacheNamespace) -> Result<Stored, &'static str> {
    let mut reader = Reader { bytes: &bytes, at: 0 };
    let header = read_header(&mut reader)?;
    if header.name != namespace.name {
        return Err("another namespace");
    }
    if header.schema != namespace.schema {
        return Err("written under another schema");
    }
    let mut index = BTreeMap::new();
    let mut last: Option<[u8; 32]> = None;
    for _ in 0..header.count {
        let mut key = [0; 32];
        key.copy_from_slice(&bytes[reader.take(32)?]);
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

/// Creates `dir` and the missing directories above it, writing a `CACHEDIR.TAG` and a `.gitignore` of `*` into the
/// innermost one when it was created. A missing directory named `node_modules` is never created.
fn create_tagged(dir: &Path) -> Result<(), StoreError> {
    // The empty path is the current directory to every call below: the tag files would land in the project.
    if dir.as_os_str().is_empty() {
        return Err(StoreError::EmptyLocation);
    }
    if dir.is_dir() {
        return Ok(());
    }
    let missing: Vec<&Path> = dir.ancestors().take_while(|ancestor| !ancestor.exists()).collect();
    if let Some(node_modules) = missing
        .iter()
        .find(|ancestor| ancestor.file_name() == Some("node_modules".as_ref()))
    {
        return Err(StoreError::NodeModules {
            path: node_modules.to_path_buf(),
        });
    }
    fs::create_dir_all(dir).map_err(|error| StoreError::io(dir, error))?;
    // A tag that cannot be written leaves a working cache behind; it is only a courtesy to backup tools and git.
    tag(dir);
    Ok(())
}

/// Writes the `CACHEDIR.TAG` and the `.gitignore` of a directory skif created, each only where no file is: a file
/// that is already there, whoever wrote it, is never replaced.
fn tag(dir: &Path) {
    for (name, content) in [(TAG_FILE, CACHEDIR_TAG), (IGNORE_FILE, IGNORE_ALL)] {
        if let Ok(mut file) = create_new(&dir.join(name)) {
            let _ = file.write_all(content.as_bytes());
        }
    }
}

/// The bytes of `file`, or `None` when it holds more than `limit` bytes.
fn read_at_most(file: &Path, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let handle = File::open(file)?;
    if handle.metadata()?.len() > limit {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    // The file may grow between the check and the read.
    handle.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= limit).then_some(bytes))
}

/// The lock of the store in `dir`, or `None` when another process holds it.
fn try_lock(dir: &Path) -> Result<Option<File>, StoreError> {
    let path = dir.join(LOCK_FILE);
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|error| StoreError::io(&path, error))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(StoreError::io(&path, error)),
    }
}

/// Writes `bytes` to `file` through a temporary file in the same directory, so a concurrent reader sees the old file
/// or the new one, never a part.
fn write_atomically(file: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let written = create_temporary(file).and_then(|(mut handle, temporary)| {
        let renamed = handle.write_all(bytes).and_then(|()| {
            // Closed before the rename: a file that is still open cannot be renamed everywhere.
            drop(handle);
            fs::rename(&temporary, file)
        });
        if renamed.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        renamed
    });
    written.map_err(|error| StoreError::io(file, error))
}

/// A new, empty temporary file next to `file`, named `<file>.<random>.tmp`. It is created only where nothing exists,
/// so the write never goes through a link or into a file another process made, and its name cannot be guessed.
fn create_temporary(file: &Path) -> io::Result<(File, PathBuf)> {
    let mut last = io::Error::from(io::ErrorKind::AlreadyExists);
    for _ in 0..TEMPORARY_TRIES {
        // `RandomState` is keyed from the operating system's randomness.
        let random = std::collections::hash_map::RandomState::new().build_hasher().finish();
        let mut name = file.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".{random:016x}{TEMPORARY_SUFFIX}"));
        let temporary = file.with_file_name(name);
        match create_new(&temporary) {
            Ok(handle) => return Ok((handle, temporary)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => last = error,
            Err(error) => return Err(error),
        }
    }
    Err(last)
}

/// Creates `path` for writing, failing when anything is there already, a link included.
fn create_new(path: &Path) -> io::Result<File> {
    File::options().write(true).create_new(true).open(path)
}

/// Removes the temporary files of writes from the store in `dir`. The caller holds the store's lock, so none of them
/// is being written.
fn remove_temporaries(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name();
        if is_temporary(&name.to_string_lossy()) && entry.file_type().is_ok_and(|kind| !kind.is_dir()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// One store in a location, as [`list_stores`] finds it.
struct StoreDir {
    dir: PathBuf,
    bytes: u64,
    modified: SystemTime,
    /// Whether its files could be listed; one that cannot be is never sized, evicted or read.
    readable: bool,
}

/// Whether `path` is a file; `None` when that cannot be told (the directory holding it cannot be searched).
fn is_file(path: &Path) -> Option<bool> {
    match fs::metadata(path) {
        Ok(metadata) => Some(metadata.is_file()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    }
}

/// The stores in `location`: its directories named like a base key. A store whose directory cannot be read is
/// listed as unreadable, and an entry of the location that cannot be read is skipped, so one of them never hides
/// the others.
fn list_stores(location: &Path) -> io::Result<Vec<StoreDir>> {
    let mut stores = Vec::new();
    for entry in fs::read_dir(location)?.filter_map(Result::ok) {
        let name = entry.file_name();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) || !is_base_dir_name(&name.to_string_lossy()) {
            continue;
        }
        let dir = entry.path();
        // Only a directory a store wrote its lock or its base into is one: another tool's directory that happens to
        // be named like a key is left alone.
        let markers = [is_file(&dir.join(LOCK_FILE)), is_file(&dir.join(BASE_FILE))];
        if markers == [Some(false), Some(false)] {
            continue;
        }
        let mut store = StoreDir {
            dir,
            bytes: 0,
            modified: SystemTime::UNIX_EPOCH,
            readable: false,
        };
        if markers.contains(&Some(true))
            && let Ok(files) = fs::read_dir(&store.dir)
        {
            store.readable = true;
            for file in files.filter_map(Result::ok) {
                let Ok(metadata) = file.metadata() else { continue };
                let name = file.file_name();
                let name = name.to_string_lossy();
                if metadata.is_file() && is_store_file(&name) && !is_temporary(&name) {
                    store.bytes += metadata.len();
                    if let Ok(time) = metadata.modified() {
                        store.modified = store.modified.max(time);
                    }
                }
            }
        }
        stores.push(store);
    }
    stores.sort_by(|a, b| a.dir.cmp(&b.dir));
    Ok(stores)
}

/// The file a temporary file named `name` is written for: `name` is `<file>.<hex digits>.tmp`.
fn temporary_of(name: &str) -> Option<&str> {
    name.strip_suffix(TEMPORARY_SUFFIX)
        .and_then(|rest| rest.rsplit_once('.'))
        .filter(|(_, random)| !random.is_empty() && random.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(|(file, _)| file)
}

/// Whether `name` is the temporary file of a write of a store.
fn is_temporary(name: &str) -> bool {
    temporary_of(name).is_some_and(is_store_file)
}

/// Whether `name` is a file a store writes into its directory: the lock, the base, a namespace file, a temporary
/// file of a write, or the tag files of a directory skif created.
fn is_store_file(name: &str) -> bool {
    if matches!(name, LOCK_FILE | BASE_FILE | TAG_FILE | IGNORE_FILE) {
        return true;
    }
    let written = temporary_of(name).unwrap_or(name);
    written == BASE_FILE
        || written
            .strip_suffix(EXTENSION)
            .and_then(|stem| stem.strip_suffix('.'))
            .is_some_and(CacheNamespace::is_valid_name)
}

/// Removes the files the store in `dir` wrote, then the directory when nothing else is in it: a file skif did not
/// write survives, with its directory.
fn remove_store(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && is_store_file(&entry.file_name().to_string_lossy()) {
            fs::remove_file(entry.path())?;
        }
    }
    match fs::remove_dir(dir) {
        Err(error) if error.kind() != io::ErrorKind::DirectoryNotEmpty => Err(error),
        _ => Ok(()),
    }
}

fn is_base_dir_name(name: &str) -> bool {
    name.len() == 32
        && name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// One namespace of a store, as [`stats`] reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct NamespaceStats {
    /// The namespace name.
    pub name: String,
    /// The entries its file holds; `None` when the file is not one skif can read.
    pub entries: Option<u64>,
    /// The size of its file.
    pub bytes: u64,
}

/// One store of a location, as [`stats`] reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct StoreStats {
    /// The store's directory name, its base key.
    pub key: String,
    /// The base it belongs to (`git:<the repository's git directory>:<path in the repository>` or
    /// `path:<absolute path>`), when recorded.
    pub base: Option<String>,
    /// Its namespaces, by name.
    pub namespaces: Vec<NamespaceStats>,
    /// The size of every file in it.
    pub bytes: u64,
    /// When a file in it was last written.
    pub modified: Option<SystemTime>,
    /// Whether its directory could be read; the sizes and namespaces of one that could not are unknown.
    pub readable: bool,
}

/// What a cache location holds, as [`stats`] reports it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct CacheStats {
    /// Its stores, by key.
    pub stores: Vec<StoreStats>,
    /// The size of every store.
    pub bytes: u64,
}

/// What `location` holds: every store with its namespaces, entries and sizes. A missing location holds nothing, and
/// a store that cannot be read is listed as unreadable.
///
/// # Errors
/// The location exists but cannot be listed.
pub fn stats(location: &Path) -> io::Result<CacheStats> {
    let stores = match list_stores(location) {
        Ok(stores) => stores,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(CacheStats::default()),
        Err(error) => return Err(error),
    };
    let mut stats = CacheStats::default();
    for store in stores {
        let key = store
            .dir
            .file_name()
            .map_or_default(|name| name.to_string_lossy().into_owned());
        let base = fs::read_to_string(store.dir.join(BASE_FILE)).ok();
        let mut namespaces = Vec::new();
        let entries = fs::read_dir(&store.dir).into_iter().flatten();
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != EXTENSION) {
                continue;
            }
            let Some(name) = path.file_stem().map(|stem| stem.to_string_lossy().into_owned()) else {
                continue;
            };
            let bytes = entry.metadata().map_or(0, |metadata| metadata.len());
            namespaces.push(NamespaceStats {
                entries: header_count(&path, &name),
                name,
                bytes,
            });
        }
        namespaces.sort_by(|a, b| a.name.cmp(&b.name));
        stats.bytes += store.bytes;
        stats.stores.push(StoreStats {
            key,
            base,
            namespaces,
            bytes: store.bytes,
            modified: (store.modified != SystemTime::UNIX_EPOCH).then_some(store.modified),
            readable: store.readable,
        });
    }
    Ok(stats)
}

/// The record count in the header of the namespace file `path` of namespace `name`.
fn header_count(path: &Path, name: &str) -> Option<u64> {
    let mut head = Vec::new();
    File::open(path)
        .ok()?
        .take(header_size(name) + 8)
        .read_to_end(&mut head)
        .ok()?;
    let header = read_header(&mut Reader { bytes: &head, at: 0 }).ok()?;
    (header.name == name).then_some(u64::from(header.count))
}

/// What [`clean`] removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct CleanReport {
    /// The stores removed.
    pub stores: usize,
    /// Their size.
    pub bytes: u64,
}

/// Removes the store of `base` from `location`, or with `base` `None` every store in it, then the location itself
/// when nothing but skif's own tag files is left. Only the files a store wrote are removed, from directories named
/// like a base key that hold a store's lock or base file, so a location shared with another tool (the downloads of
/// the pre-commit launcher, which also reads `SKIF_CACHE_DIR`) keeps every file skif did not write. A missing
/// location is clean. A store that cannot be removed does not keep the others: they are removed first.
///
/// # Errors
/// The location exists but cannot be listed, or a store to remove could not be.
pub fn clean(location: &Path, base: Option<&Path>) -> io::Result<CleanReport> {
    let stores = match list_stores(location) {
        Ok(stores) => stores,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(CleanReport::default()),
        Err(error) => return Err(error),
    };
    let only = base.map(|base| location.join(base_key(base)));
    let mut report = CleanReport::default();
    let mut failed: Option<io::Error> = None;
    for store in stores {
        if only.as_ref().is_some_and(|only| *only != store.dir) {
            continue;
        }
        match remove_store(&store.dir) {
            Ok(()) => {
                report.stores += 1;
                report.bytes += store.bytes;
            }
            Err(error) => failed = failed.or(Some(error)),
        }
    }
    if let Some(error) = failed {
        return Err(error);
    }
    let leftover: Vec<String> = fs::read_dir(location)?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    // The location goes only when nothing but the tag files skif wrote is left in it.
    let own_tags = fs::read_to_string(location.join(TAG_FILE)).is_ok_and(|tag| tag == CACHEDIR_TAG)
        && fs::read_to_string(location.join(IGNORE_FILE)).is_ok_and(|ignore| ignore == IGNORE_ALL);
    if own_tags && leftover.iter().all(|name| name == TAG_FILE || name == IGNORE_FILE) {
        for name in leftover {
            fs::remove_file(location.join(name))?;
        }
        fs::remove_dir(location)?;
    }
    Ok(report)
}

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<DiskStore>;
    let _ = assert_send_sync::<StoreError>;
};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
