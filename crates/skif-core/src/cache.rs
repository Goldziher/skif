//! The cache store a host gives the linter, the keys skif derives for it and the envelope skif wraps every value in
//! (ADR 0007).
//!
//! A host owns where entries live: it implements [`CacheStore`] over its own storage (a directory, poly's cache, a
//! map in memory) and hands it to the linter. skif computes every key and writes and verifies every value, so a
//! store only moves opaque bytes. Nothing here does I/O.
//!
//! * Keys are 32-byte blake3 digests built with [`KeyBuilder`] over the cache version, the skif version, the
//!   namespace and its schema, the host's salt and the inputs the entry depends on, which each namespace names. A
//!   parse result (`modules`) depends on the file's name, its text and the parse options and on nothing else, so
//!   that is its key: no path, and identical files share an entry wherever they lie. A namespace whose entries
//!   depend on where a file lies adds its path relative to a root ([`KeyBuilder::relative_path`]). A key never
//!   holds a modification time or an absolute path, so worktrees and hook snapshots of one tree share entries.
//! * Values are sealed with [`seal`] and opened with [`open`]: an envelope of magic, namespace, schema, key echo,
//!   length and checksum around the payload. A value that does not open exactly is a miss, never a wrong result,
//!   whatever the store returns. The envelope is integrity, not authentication.
//! * [`CacheCounters`] counts the lookups and writes of a run per namespace, so a host or a test can tell a warm run
//!   from a cold one without reading the store.

use std::collections::BTreeMap;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use serde::{Deserialize, Serialize};

/// The layout of keys and envelopes. Raised when either changes, so entries written by another layout are misses.
pub const CACHE_VERSION: u32 = 1;

/// The first bytes of every sealed value.
const MAGIC: [u8; 8] = *b"SKIFCACH";

/// The bytes of a sealed value before the namespace name: magic and cache version.
const PREFIX: usize = MAGIC.len() + 4;

/// A kind of cache entry: a name and the schema of its payload.
///
/// The namespace is a parameter of every store call, so a host can lay entries out and report them per namespace.
/// The schema is part of every key and envelope: raising it makes every entry written under the old one a miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CacheNamespace {
    /// A short, stable name such as `modules`: lower-case ASCII letters, digits, `.` and `-`, at most 64 bytes.
    pub name: &'static str,
    /// The version of the payload's layout.
    pub schema: u32,
}

impl CacheNamespace {
    /// The namespace `name` at `schema`.
    #[must_use]
    pub const fn new(name: &'static str, schema: u32) -> Self {
        Self { name, schema }
    }

    /// Whether `name` is a namespace name a host can use as a directory or file name as it is.
    #[must_use]
    pub fn is_valid_name(name: &str) -> bool {
        !name.is_empty()
            && name.len() <= 64
            && !name.starts_with('.')
            && name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-'))
    }
}

/// A store of cache entries a host owns and gives the linter (ADR 0007).
///
/// Both methods are called from the parallel steps of a run, so an implementation is `Send + Sync`. A lookup that
/// fails for any reason returns `None`, a miss; a write that fails is the host's to log and never reaches the run.
/// Values are opaque: skif seals them before [`put`](Self::put) and verifies them after [`get`](Self::get), so a
/// store may lose, truncate or mix up entries without changing a result.
pub trait CacheStore: Send + Sync {
    /// The value stored under `key` in `namespace`, if any.
    fn get(&self, namespace: CacheNamespace, key: &[u8; 32]) -> Option<Arc<[u8]>>;
    /// Stores `value` under `key` in `namespace`.
    fn put(&self, namespace: CacheNamespace, key: &[u8; 32], value: Arc<[u8]>);
}

/// The key of an entry: 32 bytes.
type Key = [u8; 32];

/// The entries of a [`MemoryStore`], by namespace name and key.
type Entries = BTreeMap<(&'static str, Key), Arc<[u8]>>;

/// A [`CacheStore`] in memory, for tests and for a host that keeps entries only while it runs.
#[derive(Debug, Default)]
pub struct MemoryStore {
    entries: Mutex<Entries>,
}

impl MemoryStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of entries in every namespace.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner).len()
    }

    /// Whether the store holds no entry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every entry, sorted by namespace name and key.
    #[must_use]
    pub fn entries(&self) -> Vec<(&'static str, Key, Arc<[u8]>)> {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|((name, key), value)| (*name, *key, Arc::clone(value)))
            .collect()
    }
}

impl CacheStore for MemoryStore {
    fn get(&self, namespace: CacheNamespace, key: &[u8; 32]) -> Option<Arc<[u8]>> {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(namespace.name, *key))
            .cloned()
    }

    fn put(&self, namespace: CacheNamespace, key: &[u8; 32], value: Arc<[u8]>) {
        self.entries
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert((namespace.name, *key), value);
    }
}

/// The store a linter caches through and the digest of its host's salt, as the run hands them to the analyses.
#[derive(Clone)]
pub struct CacheHandle {
    store: Arc<dyn CacheStore>,
    salt: [u8; 32],
}

impl CacheHandle {
    /// `store`, keyed with the digest of `salt`, the host's opaque salt (see [`salt_digest`]).
    #[must_use]
    pub fn new(store: Arc<dyn CacheStore>, salt: [u8; 32]) -> Self {
        Self { store, salt }
    }

    /// The store.
    #[must_use]
    pub fn store(&self) -> &Arc<dyn CacheStore> {
        &self.store
    }

    /// The digest of the host's salt, an input of every key.
    #[must_use]
    pub const fn salt(&self) -> &[u8; 32] {
        &self.salt
    }
}

impl std::fmt::Debug for CacheHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheHandle").finish_non_exhaustive()
    }
}

/// The digest of a host's opaque salt: no salt and an empty salt are the same.
#[must_use]
pub fn salt_digest(salt: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key("skif cache salt v1");
    hasher.update(salt);
    *hasher.finalize().as_bytes()
}

/// The blake3 digest of `content`, the form in which file contents and configuration enter a key.
#[must_use]
pub fn content_digest(content: &[u8]) -> [u8; 32] {
    *blake3::hash(content).as_bytes()
}

/// Builds the 32-byte key of one cache entry.
///
/// Every key covers [`CACHE_VERSION`], the skif version, the namespace with its schema and the host's salt; the
/// caller adds the inputs the entry depends on. Each field is written with its length first, so two different
/// splits of the same bytes never give the same key.
///
/// ```
/// use std::path::Path;
/// use skif_core::cache::{CacheNamespace, KeyBuilder, salt_digest};
///
/// let modules = CacheNamespace::new("modules", 1);
/// let key = |root: &str, file: &str| {
///     let file = Path::new(file);
///     KeyBuilder::new(modules, &salt_digest(b"host"))
///         .relative_path(file.strip_prefix(root).unwrap_or(file))
///         .content(b"export const a = 1;")
///         .finish()
/// };
/// // The key does not depend on where the tree lives.
/// assert_eq!(key("/one", "/one/src/a.ts"), key("/two", "/two/src/a.ts"));
/// ```
#[derive(Debug, Clone)]
pub struct KeyBuilder {
    hasher: blake3::Hasher,
}

impl KeyBuilder {
    /// A key in `namespace` under the host salt digest `salt`.
    #[must_use]
    pub fn new(namespace: CacheNamespace, salt: &[u8; 32]) -> Self {
        let mut builder = Self {
            hasher: blake3::Hasher::new_derive_key("skif cache key v1"),
        };
        builder.write(&CACHE_VERSION.to_le_bytes());
        builder.write(env!("CARGO_PKG_VERSION").as_bytes());
        builder.write(namespace.name.as_bytes());
        builder.write(&namespace.schema.to_le_bytes());
        builder.write(salt);
        builder
    }

    fn write(&mut self, bytes: &[u8]) {
        self.hasher.update(&(bytes.len() as u64).to_le_bytes());
        self.hasher.update(bytes);
    }

    /// Adds a field of raw bytes, such as a flag or an option.
    #[must_use]
    pub fn bytes(mut self, field: &[u8]) -> Self {
        self.write(field);
        self
    }

    /// Adds the digest of the configuration the entry depends on (see [`content_digest`]).
    #[must_use]
    pub fn config_digest(self, digest: &[u8; 32]) -> Self {
        self.bytes(digest)
    }

    /// Adds `content` by its digest.
    #[must_use]
    pub fn content(self, content: &[u8]) -> Self {
        self.bytes(&content_digest(content))
    }

    /// Adds a path relative to the root the entry belongs to, its components joined with `/` so every platform
    /// spells it alike. A key must never depend on where a tree lives: an absolute path is written with a marker of
    /// its own, so such a key is still correct but shared by no other checkout.
    #[must_use]
    pub fn relative_path(mut self, path: &Path) -> Self {
        let mut spelled = Vec::new();
        let mut absolute = false;
        for component in path.components() {
            match component {
                Component::Normal(part) => {
                    if !spelled.is_empty() {
                        spelled.push(b'/');
                    }
                    spelled.extend_from_slice(part.as_encoded_bytes());
                }
                Component::ParentDir => {
                    if !spelled.is_empty() {
                        spelled.push(b'/');
                    }
                    spelled.extend_from_slice(b"..");
                }
                Component::CurDir => {}
                Component::RootDir | Component::Prefix(_) => absolute = true,
            }
        }
        self.write(&[u8::from(absolute)]);
        if absolute {
            self.write(path.as_os_str().as_encoded_bytes());
        } else {
            self.write(&spelled);
        }
        self
    }

    /// The key.
    #[must_use]
    pub fn finish(&self) -> [u8; 32] {
        *self.hasher.finalize().as_bytes()
    }
}

/// Wraps `payload` in the envelope [`open`] verifies: magic, cache version, namespace name and schema, the key,
/// the payload's length and blake3 checksum, then the payload.
///
/// # Panics
/// Never for a namespace of [`CacheNamespace::is_valid_name`]; a longer name is cut to 255 bytes in the envelope,
/// which [`open`] then rejects.
#[must_use]
pub fn seal(namespace: CacheNamespace, key: &[u8; 32], payload: &[u8]) -> Arc<[u8]> {
    let name = namespace.name.as_bytes();
    let name = &name[..name.len().min(usize::from(u8::MAX))];
    let mut value = Vec::with_capacity(PREFIX + 1 + name.len() + 4 + 32 + 8 + 32 + payload.len());
    value.extend_from_slice(&MAGIC);
    value.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    #[allow(clippy::cast_possible_truncation)]
    value.push(name.len() as u8);
    value.extend_from_slice(name);
    value.extend_from_slice(&namespace.schema.to_le_bytes());
    value.extend_from_slice(key);
    value.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    value.extend_from_slice(&content_digest(payload));
    value.extend_from_slice(payload);
    Arc::from(value)
}

/// The payload of `value` when it is exactly an envelope [`seal`] wrote for `namespace` and `key`: any other magic,
/// version, namespace, schema, key, length or checksum, a truncated value or bytes after the payload give `None`.
#[must_use]
pub fn open<'a>(namespace: CacheNamespace, key: &[u8; 32], value: &'a [u8]) -> Option<&'a [u8]> {
    let mut rest = value;
    let mut take = |length: usize| -> Option<&'a [u8]> {
        if rest.len() < length {
            return None;
        }
        let (head, tail) = rest.split_at(length);
        rest = tail;
        Some(head)
    };
    if take(MAGIC.len())? != MAGIC || take(4)? != CACHE_VERSION.to_le_bytes() {
        return None;
    }
    let length = usize::from(*take(1)?.first()?);
    if take(length)? != namespace.name.as_bytes() || take(4)? != namespace.schema.to_le_bytes() || take(32)? != key {
        return None;
    }
    let mut size = [0; 8];
    size.copy_from_slice(take(8)?);
    let size = usize::try_from(u64::from_le_bytes(size)).ok()?;
    let checksum = take(32)?;
    let payload = take(size)?;
    if !rest.is_empty() || checksum != content_digest(payload) {
        return None;
    }
    Some(payload)
}

/// What a run did with one namespace of the store.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub struct CacheCounters {
    /// Lookups answered by a stored entry that opened.
    pub hits: u64,
    /// Lookups the store did not answer, or answered with a value that did not open.
    pub misses: u64,
    /// Bytes of sealed values handed to the store.
    pub bytes_written: u64,
}

impl CacheCounters {
    /// Counters with these values.
    #[must_use]
    pub const fn new(hits: u64, misses: u64, bytes_written: u64) -> Self {
        Self {
            hits,
            misses,
            bytes_written,
        }
    }

    /// Adds `other` to these counters.
    pub const fn add(&mut self, other: Self) {
        self.hits += other.hits;
        self.misses += other.misses;
        self.bytes_written += other.bytes_written;
    }
}

/// [`CacheCounters`] that the threads of a run update together.
#[derive(Debug, Default)]
pub struct CacheTally {
    hits: AtomicU64,
    misses: AtomicU64,
    bytes_written: AtomicU64,
}

impl CacheTally {
    /// A tally at zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Counts a hit.
    pub fn hit(&self) {
        self.hits.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts a miss.
    pub fn miss(&self) {
        self.misses.fetch_add(1, Ordering::Relaxed);
    }

    /// Counts a hit in place of a miss already counted for the same lookup: the entry arrived between a first
    /// look and a second, so the lookup was served after all.
    pub fn hit_after_miss(&self) {
        self.hit();
        // Never below zero, whatever the caller counted before.
        let mut misses = self.misses.load(Ordering::Relaxed);
        while let Some(fewer) = misses.checked_sub(1) {
            match self
                .misses
                .compare_exchange_weak(misses, fewer, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(current) => misses = current,
            }
        }
    }

    /// Counts `bytes` written.
    pub fn written(&self, bytes: usize) {
        self.bytes_written.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    /// The counts so far.
    #[must_use]
    pub fn snapshot(&self) -> CacheCounters {
        CacheCounters {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            bytes_written: self.bytes_written.load(Ordering::Relaxed),
        }
    }
}

const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    let _ = assert_send_sync::<MemoryStore>;
    let _ = assert_send_sync::<CacheHandle>;
    let _ = assert_send_sync::<CacheTally>;
    let _ = assert_send_sync::<Arc<dyn CacheStore>>;
};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const MODULES: CacheNamespace = CacheNamespace::new("modules", 3);

    fn key(text: &str) -> [u8; 32] {
        KeyBuilder::new(MODULES, &salt_digest(b""))
            .content(text.as_bytes())
            .finish()
    }

    #[test]
    fn a_sealed_value_opens_to_its_payload() {
        let key = key("a");
        let value = seal(MODULES, &key, b"payload");
        assert_eq!(open(MODULES, &key, &value), Some(&b"payload"[..]));
        let empty = seal(MODULES, &key, b"");
        assert_eq!(open(MODULES, &key, &empty), Some(&b""[..]));
    }

    #[test]
    fn a_value_that_is_not_exactly_the_envelope_does_not_open() {
        let key = key("a");
        let value = seal(MODULES, &key, b"payload");
        assert!(open(MODULES, &self::key("b"), &value).is_none(), "another key");
        assert!(
            open(CacheNamespace::new("modules", 4), &key, &value).is_none(),
            "another schema"
        );
        assert!(
            open(CacheNamespace::new("other", 3), &key, &value).is_none(),
            "another namespace"
        );
        assert!(open(MODULES, &key, &value[..value.len() - 1]).is_none(), "truncated");
        assert!(open(MODULES, &key, &[]).is_none(), "empty");
        let mut longer = value.to_vec();
        longer.push(0);
        assert!(open(MODULES, &key, &longer).is_none(), "trailing bytes");
        for at in 0..value.len() {
            let mut flipped = value.to_vec();
            flipped[at] ^= 0x01;
            assert!(open(MODULES, &key, &flipped).is_none(), "flipped byte {at}");
        }
    }

    #[test]
    fn random_bytes_never_open() {
        let key = key("a");
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        for length in 0..512 {
            let bytes: Vec<u8> = (0..length)
                .map(|_| {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    state.to_le_bytes()[0]
                })
                .collect();
            assert!(open(MODULES, &key, &bytes).is_none());
        }
    }

    #[test]
    fn the_key_changes_with_each_input() {
        let salt = salt_digest(b"host");
        let base = KeyBuilder::new(MODULES, &salt)
            .relative_path(Path::new("src/a.ts"))
            .content(b"x")
            .finish();
        let others = [
            KeyBuilder::new(CacheNamespace::new("modules", 4), &salt)
                .relative_path(Path::new("src/a.ts"))
                .content(b"x")
                .finish(),
            KeyBuilder::new(CacheNamespace::new("other", 3), &salt)
                .relative_path(Path::new("src/a.ts"))
                .content(b"x")
                .finish(),
            KeyBuilder::new(MODULES, &salt_digest(b"other host"))
                .relative_path(Path::new("src/a.ts"))
                .content(b"x")
                .finish(),
            KeyBuilder::new(MODULES, &salt)
                .relative_path(Path::new("src/b.ts"))
                .content(b"x")
                .finish(),
            KeyBuilder::new(MODULES, &salt)
                .relative_path(Path::new("src/a.ts"))
                .content(b"y")
                .finish(),
            KeyBuilder::new(MODULES, &salt)
                .relative_path(Path::new("/src/a.ts"))
                .content(b"x")
                .finish(),
        ];
        for other in others {
            assert_ne!(base, other);
        }
        // Length prefixes: the same bytes split differently are different keys.
        assert_ne!(
            KeyBuilder::new(MODULES, &salt).bytes(b"ab").bytes(b"c").finish(),
            KeyBuilder::new(MODULES, &salt).bytes(b"a").bytes(b"bc").finish()
        );
    }

    #[test]
    fn a_relative_path_is_spelled_alike_on_every_platform() {
        let salt = salt_digest(b"");
        let key = |path: &str| KeyBuilder::new(MODULES, &salt).relative_path(Path::new(path)).finish();
        assert_eq!(key("src/a.ts"), key("./src/a.ts"));
        assert_eq!(key("src/a.ts"), key("src//a.ts"));
        assert_ne!(key("../a.ts"), key("a.ts"));
    }

    #[test]
    fn no_salt_and_an_empty_salt_are_the_same() {
        assert_eq!(salt_digest(b""), salt_digest(&[]));
        assert_ne!(salt_digest(b""), salt_digest(b"x"));
    }

    #[test]
    fn namespace_names_are_safe_as_file_names() {
        for good in ["modules", "other.facts", "a-1"] {
            assert!(CacheNamespace::is_valid_name(good), "{good}");
        }
        for bad in ["", ".hidden", "Upper", "a/b", "a\\b", "..", &"x".repeat(65)] {
            assert!(!CacheNamespace::is_valid_name(bad), "{bad}");
        }
    }

    #[test]
    fn a_memory_store_returns_what_was_put_per_namespace() {
        let store = MemoryStore::new();
        let key = key("a");
        assert!(store.get(MODULES, &key).is_none());
        store.put(MODULES, &key, Arc::from(&b"v"[..]));
        assert_eq!(store.get(MODULES, &key).as_deref(), Some(&b"v"[..]));
        assert!(store.get(CacheNamespace::new("other", 1), &key).is_none());
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn a_tally_counts_and_counters_add() {
        let tally = CacheTally::new();
        tally.hit();
        tally.miss();
        tally.miss();
        tally.written(10);
        let mut counters = tally.snapshot();
        assert_eq!(counters, CacheCounters::new(1, 2, 10));
        counters.add(CacheCounters::new(1, 0, 5));
        assert_eq!(counters, CacheCounters::new(2, 2, 15));
    }
}
