//! A memoizing [`FileSystem`] so one run reads every directory and configuration file at most once.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::SystemTime;

use crate::fs::{DirEntry, FileSystem, PathKind};

const SHARDS: u64 = 64;

/// A directory is listed, instead of probing its children one by one, once this many of its children were probed
/// without a cached listing: several probes of one directory (every extension of an import) then cost one listing.
const LIST_AFTER_PROBES: u32 = 3;

/// Configuration files above this size are read every time instead of being held.
const MAX_CACHED_TEXT: usize = 128 * 1024;

/// A directory listing, or the kind of error that makes the directory permanently unreadable.
type Listing = Result<Arc<[DirEntry]>, io::ErrorKind>;

/// A path-keyed map split over independently locked shards, so parallel readers rarely contend.
struct Sharded<V> {
    hasher: RandomState,
    shards: Vec<RwLock<HashMap<PathBuf, V>>>,
}

impl<V: Clone> Sharded<V> {
    fn new() -> Self {
        Self {
            hasher: RandomState::new(),
            shards: (0..SHARDS).map(|_| RwLock::new(HashMap::new())).collect(),
        }
    }

    fn shard(&self, key: &Path) -> &RwLock<HashMap<PathBuf, V>> {
        let index = usize::try_from(self.hasher.hash_one(key) % SHARDS).unwrap_or(0);
        &self.shards[index]
    }

    fn get(&self, key: &Path) -> Option<V> {
        let shard = self.shard(key).read().unwrap_or_else(PoisonError::into_inner);
        shard.get(key).cloned()
    }

    fn insert(&self, key: &Path, value: V) {
        let mut shard = self.shard(key).write().unwrap_or_else(PoisonError::into_inner);
        shard.insert(key.to_path_buf(), value);
    }

    fn clear(&self) {
        for shard in &self.shards {
            shard.write().unwrap_or_else(PoisonError::into_inner).clear();
        }
    }
}

/// Whether the file is small, shared configuration that several components read: manifests, `tsconfig`, ignore
/// files and tool configuration. Source files are read once and are not held.
fn is_config_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let extension = Path::new(name).extension().and_then(OsStr::to_str);
    matches!(extension, Some("json" | "jsonc" | "json5" | "yaml" | "yml" | "toml"))
        || matches!(
            name,
            ".gitignore" | ".ignore" | ".npmrc" | ".yarnrc" | ".nvmrc" | ".node-version" | ".prettierignore"
        )
        || name.contains(".config.")
        || (name.starts_with('.') && (name.ends_with("rc") || name.contains("rc.")))
}

/// The listing in path order, which the lookups by binary search rely on; a host file system that returns entries
/// in another order gets them sorted here instead of silently answering "no such file".
fn sorted(entries: Arc<[DirEntry]>) -> Arc<[DirEntry]> {
    if entries.is_sorted_by(|a, b| a.path <= b.path) {
        return entries;
    }
    let mut owned = entries.to_vec();
    owned.sort_by(|a, b| a.path.cmp(&b.path));
    Arc::from(owned)
}

/// Whether a sibling in `entries` is named like `path` but for the case of ASCII letters.
fn differs_by_case_only(entries: &[DirEntry], path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        entries.iter().any(|entry| {
            entry
                .path
                .file_name()
                .is_some_and(|other| other.eq_ignore_ascii_case(name))
        })
    })
}

fn not_found(path: &Path) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("{} not found", path.display()))
}

/// Wraps a [`FileSystem`] and memoizes what a run asks of it repeatedly.
///
/// - Directory listings are kept (shared, see [`FileSystem::read_dir_shared`]), including the absence of a
///   directory.
/// - `is_file`, `is_dir` and `kind` are answered from the listing of the parent directory when it is already cached,
///   so probing many names in one directory costs one listing, not one `stat` per name. Otherwise the inner file
///   system is asked once, for file and directory together, and the answer, a negative one included, is kept. A
///   directory whose children are probed repeatedly (except `node_modules` and scope directories, whose listing
///   costs a `stat` per link) is listed after a few probes and answers the rest from the listing.
/// - Small configuration files (manifests, `tsconfig`, ignore files, tool configuration) are kept after the first
///   read, a missing one included. Other files are read through every time.
///
/// The cache never expires: use one instance for one run, and [`CachedFs::clear`] it after writing files. A name the
/// listing lacks but a sibling spells in another case is asked of the inner file system, so the answer is the one
/// the disk gives (found on a case-insensitive disk, absent on a case-sensitive one) whatever the number of probes
/// before it.
pub struct CachedFs<F> {
    inner: F,
    listings: Sharded<Listing>,
    kinds: Sharded<PathKind>,
    probes: Sharded<u32>,
    texts: Sharded<Option<Arc<str>>>,
}

impl<F: FileSystem> CachedFs<F> {
    /// An empty cache over `inner`.
    #[must_use]
    pub fn new(inner: F) -> Self {
        Self {
            inner,
            listings: Sharded::new(),
            kinds: Sharded::new(),
            probes: Sharded::new(),
            texts: Sharded::new(),
        }
    }

    /// The wrapped file system.
    #[must_use]
    pub const fn inner(&self) -> &F {
        &self.inner
    }

    /// Forgets everything, so the next query observes the file system as it is now.
    pub fn clear(&self) {
        self.listings.clear();
        self.kinds.clear();
        self.probes.clear();
        self.texts.clear();
    }

    /// The kind of `path` according to the cached listing of its parent; `None` when that is not cached.
    fn kind_in_parent(&self, path: &Path) -> Option<PathKind> {
        let listing = self.listings.get(path.parent()?)?;
        let Ok(entries) = listing else {
            return Some(PathKind::NONE);
        };
        match entries.binary_search_by(|entry| entry.path.as_path().cmp(path)) {
            Ok(index) => Some(PathKind {
                is_file: entries[index].is_file,
                is_dir: entries[index].is_dir,
            }),
            // A case-insensitive disk finds `Package.json` for `package.json`, which the listing does not spell.
            Err(_) if differs_by_case_only(&entries, path) => None,
            Err(_) => Some(PathKind::NONE),
        }
    }

    /// Counts a probe of a child of `path`'s parent and lists the parent once it was probed often enough. Whether the
    /// listing is now cached is not reported: [`CachedFs::kind_in_parent`] tells.
    fn list_parent_when_probed(&self, path: &Path) {
        let Some(parent) = path.parent() else { return };
        let name = parent.file_name().and_then(OsStr::to_str).unwrap_or_default();
        if name == "node_modules" || name.starts_with('@') {
            return;
        }
        let probed = self.probes.get(parent).unwrap_or(0).saturating_add(1);
        self.probes.insert(parent, probed);
        if probed >= LIST_AFTER_PROBES {
            let _ = self.read_dir_shared(parent);
        }
    }
}

impl<F: FileSystem + std::fmt::Debug> std::fmt::Debug for CachedFs<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedFs")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl<F: FileSystem + Default> Default for CachedFs<F> {
    fn default() -> Self {
        Self::new(F::default())
    }
}

impl<F: FileSystem> FileSystem for CachedFs<F> {
    fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
        if !is_config_file(path) {
            return self.inner.read_to_string(path);
        }
        if let Some(cached) = self.texts.get(path) {
            return cached.ok_or_else(|| not_found(path));
        }
        if self.kind_in_parent(path).is_some_and(|kind| !kind.is_file) {
            return Err(not_found(path));
        }
        match self.inner.read_to_string(path) {
            Ok(text) => {
                if text.len() <= MAX_CACHED_TEXT {
                    self.texts.insert(path, Some(Arc::clone(&text)));
                }
                Ok(text)
            }
            Err(error) => {
                if error.kind() == io::ErrorKind::NotFound {
                    self.texts.insert(path, None);
                }
                Err(error)
            }
        }
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.read_dir_shared(path).map(|entries| entries.to_vec())
    }

    fn read_dir_shared(&self, path: &Path) -> io::Result<Arc<[DirEntry]>> {
        if let Some(listing) = self.listings.get(path) {
            return listing.map_err(io::Error::from);
        }
        match self.inner.read_dir_shared(path) {
            Ok(entries) => {
                let entries = sorted(entries);
                self.listings.insert(path, Ok(Arc::clone(&entries)));
                Ok(entries)
            }
            Err(error) => {
                if matches!(error.kind(), io::ErrorKind::NotFound | io::ErrorKind::NotADirectory) {
                    self.listings.insert(path, Err(error.kind()));
                }
                Err(error)
            }
        }
    }

    fn is_file(&self, path: &Path) -> bool {
        self.kind(path).is_file
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.kind(path).is_dir
    }

    fn kind(&self, path: &Path) -> PathKind {
        if let Some(kind) = self.kind_in_parent(path) {
            return kind;
        }
        if let Some(kind) = self.kinds.get(path) {
            return kind;
        }
        self.list_parent_when_probed(path);
        if let Some(kind) = self.kind_in_parent(path) {
            return kind;
        }
        let kind = self.inner.kind(path);
        self.kinds.insert(path, kind);
        kind
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.inner.canonicalize(path)
    }

    fn read_bytes(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read_bytes(path)
    }

    fn modified(&self, path: &Path) -> Option<SystemTime> {
        self.inner.modified(path)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::fs::{MemoryFs, OsFs};

    use super::*;

    /// A [`MemoryFs`] that counts what reaches it.
    struct Counting {
        inner: MemoryFs,
        listings: AtomicUsize,
        stats: AtomicUsize,
        reads: AtomicUsize,
    }

    impl Counting {
        fn new(inner: MemoryFs) -> Self {
            Self {
                inner,
                listings: AtomicUsize::new(0),
                stats: AtomicUsize::new(0),
                reads: AtomicUsize::new(0),
            }
        }
    }

    fn count(counter: &AtomicUsize) -> usize {
        counter.load(Ordering::SeqCst)
    }

    impl FileSystem for Counting {
        fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.inner.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
            self.listings.fetch_add(1, Ordering::SeqCst);
            self.inner.read_dir(path)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.stats.fetch_add(1, Ordering::SeqCst);
            self.inner.is_file(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.stats.fetch_add(1, Ordering::SeqCst);
            self.inner.is_dir(path)
        }
    }

    /// Like [`Counting`], but a `kind` is one lookup, as it is on a real file system.
    struct OneStatKind(Counting);

    impl FileSystem for OneStatKind {
        fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
            self.0.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
            self.0.read_dir(path)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.0.is_file(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.0.is_dir(path)
        }

        fn kind(&self, path: &Path) -> PathKind {
            self.0.stats.fetch_add(1, Ordering::SeqCst);
            self.0.inner.kind(path)
        }
    }

    fn cached() -> CachedFs<Counting> {
        CachedFs::new(Counting::new(
            MemoryFs::new()
                .with_file("/ws/package.json", "{}")
                .with_file("/ws/src/index.ts", "export {}")
                .with_file("/ws/src/deep/more.ts", ""),
        ))
    }

    #[test]
    fn listings_are_computed_once_and_shared() {
        let fs = cached();
        let first = fs.read_dir_shared(Path::new("/ws")).unwrap();
        let second = fs.read_dir_shared(Path::new("/ws")).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(fs.read_dir(Path::new("/ws")).unwrap(), first.to_vec());
        assert_eq!(count(&fs.inner().listings), 1);
    }

    #[test]
    fn missing_directories_are_remembered() {
        let fs = cached();
        for _ in 0..3 {
            assert_eq!(
                fs.read_dir(Path::new("/nope")).unwrap_err().kind(),
                io::ErrorKind::NotFound
            );
        }
        assert_eq!(count(&fs.inner().listings), 1);
    }

    /// A file system whose lookups ignore the case of names, as macOS and Windows volumes do; listings spell names as
    /// stored.
    struct CaseInsensitive(MemoryFs);

    impl CaseInsensitive {
        fn find(&self, path: &Path) -> Option<DirEntry> {
            let name = path.file_name()?;
            self.0.read_dir(path.parent()?).ok()?.into_iter().find(|entry| {
                entry
                    .path
                    .file_name()
                    .is_some_and(|other| other.eq_ignore_ascii_case(name))
            })
        }
    }

    impl FileSystem for CaseInsensitive {
        fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
            self.0.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
            self.0.read_dir(path)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.find(path).is_some_and(|entry| entry.is_file)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.find(path).is_some_and(|entry| entry.is_dir)
        }
    }

    #[test]
    fn a_probe_in_another_case_gets_the_answer_of_the_disk_whatever_was_probed_before() {
        let files = || {
            CaseInsensitive(
                MemoryFs::new()
                    .with_file("/ws/package.json", "{}")
                    .with_file("/ws/a.ts", "")
                    .with_file("/ws/b.ts", "")
                    .with_file("/ws/c.ts", ""),
            )
        };
        let probe = Path::new("/ws/Package.json");
        let cold = CachedFs::new(files());
        assert!(cold.is_file(probe));
        let warm = CachedFs::new(files());
        for sibling in ["/ws/a.ts", "/ws/b.ts", "/ws/c.ts", "/ws/d.ts"] {
            let _ = warm.is_file(Path::new(sibling));
        }
        assert!(warm.read_dir_shared(Path::new("/ws")).is_ok());
        assert!(
            warm.is_file(probe),
            "answered from the listing, which spells the name differently"
        );
        assert!(!warm.is_file(Path::new("/ws/other.json")));
    }

    #[test]
    fn kinds_come_from_a_cached_parent_listing_without_stats() {
        let fs = cached();
        fs.read_dir(Path::new("/ws")).unwrap();
        assert!(fs.is_file(Path::new("/ws/package.json")));
        assert!(!fs.is_file(Path::new("/ws/src")));
        assert!(fs.is_dir(Path::new("/ws/src")));
        assert!(!fs.is_dir(Path::new("/ws/package.json")));
        assert!(!fs.is_file(Path::new("/ws/tsconfig.json")));
        assert!(!fs.is_dir(Path::new("/ws/missing")));
        assert_eq!(count(&fs.inner().stats), 0);
    }

    #[test]
    fn a_missing_parent_listing_makes_every_child_absent() {
        let fs = cached();
        let _ = fs.read_dir(Path::new("/gone"));
        assert!(!fs.is_file(Path::new("/gone/package.json")));
        assert!(!fs.is_dir(Path::new("/gone/src")));
        assert_eq!(count(&fs.inner().stats), 0);
    }

    #[test]
    fn stats_without_a_listing_are_cached_including_negatives() {
        let fs = cached();
        for _ in 0..3 {
            assert!(fs.is_file(Path::new("/ws/package.json")));
            assert!(!fs.is_file(Path::new("/ws/src/nope.json")));
            assert!(fs.is_dir(Path::new("/ws/src")));
            assert!(!fs.is_dir(Path::new("/ws/nope/dir")));
        }
        // A file costs one stat; the three other paths are asked "file?" and "directory?" once each.
        assert_eq!(count(&fs.inner().stats), 1 + 2 + 2 + 2);
        assert_eq!(count(&fs.inner().listings), 0);
    }

    #[test]
    fn a_directory_probed_repeatedly_is_listed_once() {
        let fs = cached();
        for name in ["a.ts", "b.ts", "c.ts", "d.ts", "index.ts"] {
            assert_eq!(fs.is_file(&Path::new("/ws/src").join(name)), name == "index.ts");
        }
        assert!(fs.is_dir(Path::new("/ws/src/deep")));
        assert_eq!(count(&fs.inner().listings), 1);
        // Two probes were stats; the third listed the directory and the rest were answered from the listing.
        assert_eq!(count(&fs.inner().stats), 2 * 2);
    }

    #[test]
    fn module_directories_are_never_listed_by_probing() {
        let fs = CachedFs::new(Counting::new(
            MemoryFs::new().with_file("/ws/node_modules/a/package.json", "{}"),
        ));
        for name in ["a", "b", "c", "d", "e"] {
            let _ = fs.is_dir(&Path::new("/ws/node_modules").join(name));
        }
        assert_eq!(count(&fs.inner().listings), 0);
    }

    #[test]
    fn kind_reports_file_directory_and_nothing_with_one_lookup_each() {
        let fs = CachedFs::new(OneStatKind(Counting::new(
            MemoryFs::new()
                .with_file("/ws/package.json", "{}")
                .with_file("/ws/src/index.ts", "export {}"),
        )));
        assert_eq!(fs.kind(Path::new("/ws/package.json")), PathKind::FILE);
        assert_eq!(count(&fs.inner().0.stats), 1);
        assert_eq!(fs.kind(Path::new("/ws/src")), PathKind::DIR);
        assert_eq!(count(&fs.inner().0.stats), 2);
        assert_eq!(fs.kind(Path::new("/other/x")), PathKind::NONE);
        assert_eq!(count(&fs.inner().0.stats), 3);
        // Answered again from memory, whichever way it is asked.
        assert_eq!(fs.kind(Path::new("/ws/package.json")), PathKind::FILE);
        assert!(fs.is_file(Path::new("/ws/package.json")) && !fs.is_dir(Path::new("/ws/package.json")));
        assert!(fs.is_dir(Path::new("/ws/src")) && !fs.is_file(Path::new("/ws/src")));
        assert_eq!(count(&fs.inner().0.stats), 3);
    }

    #[test]
    fn a_file_system_without_a_kind_lookup_costs_a_file_and_a_directory_check() {
        let fs = cached();
        assert_eq!(fs.kind(Path::new("/ws/package.json")), PathKind::FILE);
        assert_eq!(fs.kind(Path::new("/ws/src")), PathKind::DIR);
        assert_eq!(fs.kind(Path::new("/other/x")), PathKind::NONE);
        assert_eq!(fs.kind(Path::new("/ws/package.json")), PathKind::FILE);
        assert_eq!(count(&fs.inner().stats), 1 + 2 + 2);
    }

    #[test]
    fn a_file_is_known_not_to_be_a_directory_after_one_stat() {
        let fs = cached();
        assert!(fs.is_file(Path::new("/ws/package.json")));
        assert!(!fs.is_dir(Path::new("/ws/package.json")));
        assert_eq!(count(&fs.inner().stats), 1);
    }

    #[test]
    fn configuration_files_are_read_once_and_sources_are_not_held() {
        let fs = cached();
        for _ in 0..3 {
            assert_eq!(&*fs.read_to_string(Path::new("/ws/package.json")).unwrap(), "{}");
            assert_eq!(&*fs.read_to_string(Path::new("/ws/src/index.ts")).unwrap(), "export {}");
            assert!(fs.read_to_string(Path::new("/ws/tsconfig.json")).is_err());
        }
        assert_eq!(count(&fs.inner().reads), 1 + 3 + 1);
    }

    #[test]
    fn a_configuration_file_absent_from_the_cached_listing_is_not_read() {
        let fs = cached();
        fs.read_dir(Path::new("/ws")).unwrap();
        let error = fs.read_to_string(Path::new("/ws/tsconfig.json")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(count(&fs.inner().reads), 0);
    }

    #[test]
    fn clear_observes_the_file_system_again() {
        let fs = cached();
        fs.read_dir(Path::new("/ws")).unwrap();
        fs.read_to_string(Path::new("/ws/package.json")).unwrap();
        fs.clear();
        fs.read_dir(Path::new("/ws")).unwrap();
        fs.read_to_string(Path::new("/ws/package.json")).unwrap();
        assert_eq!(count(&fs.inner().listings), 2);
        assert_eq!(count(&fs.inner().reads), 2);
    }

    #[test]
    fn configuration_file_names() {
        for name in [
            "package.json",
            "tsconfig.base.json",
            ".gitignore",
            "pnpm-workspace.yaml",
            "vite.config.ts",
            ".eslintrc.js",
            ".babelrc",
        ] {
            assert!(is_config_file(Path::new(name)), "{name}");
        }
        for name in ["index.ts", "app.tsx", "styles.css", "README.md"] {
            assert!(!is_config_file(Path::new(name)), "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_answered_by_their_target() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::fs::write(dir.path().join("file.txt"), "").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("dir-link")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("file.txt"), dir.path().join("file-link")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("gone"), dir.path().join("dangling")).unwrap();
        let fs = CachedFs::new(OsFs);
        let entries = fs.read_dir(dir.path()).unwrap();
        assert!(entries.iter().any(|entry| entry.is_symlink));
        assert!(fs.is_dir(&dir.path().join("dir-link")) && !fs.is_file(&dir.path().join("dir-link")));
        assert!(fs.is_file(&dir.path().join("file-link")) && !fs.is_dir(&dir.path().join("file-link")));
        assert!(!fs.is_file(&dir.path().join("dangling")) && !fs.is_dir(&dir.path().join("dangling")));
    }

    #[test]
    fn disk_changes_are_invisible_until_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let fs = CachedFs::new(OsFs);
        assert_eq!(fs.read_dir(dir.path()).unwrap(), Vec::<DirEntry>::new());
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert!(!fs.is_file(&dir.path().join("package.json")));
        fs.clear();
        assert!(fs.is_file(&dir.path().join("package.json")));
    }

    /// A host file system whose listings are not in path order.
    struct Unsorted(MemoryFs);

    impl FileSystem for Unsorted {
        fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
            self.0.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
            let mut entries = self.0.read_dir(path)?;
            entries.reverse();
            Ok(entries)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.0.is_file(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.0.is_dir(path)
        }
    }

    #[test]
    fn unsorted_host_listings_are_sorted_before_lookups() {
        let fs = CachedFs::new(Unsorted(
            MemoryFs::new()
                .with_file("/ws/a.json", "")
                .with_file("/ws/b.json", "")
                .with_file("/ws/c.json", ""),
        ));
        let entries = fs.read_dir_shared(Path::new("/ws")).unwrap();
        assert!(entries.windows(2).all(|pair| pair[0].path < pair[1].path));
        for name in ["a", "b", "c"] {
            assert!(fs.is_file(&Path::new("/ws").join(format!("{name}.json"))), "{name}");
        }
    }
}
