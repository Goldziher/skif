//! The files git tracks, read from `.git/index` without running git, and a file system overlay that serves them.
//!
//! Walking a large work tree costs one `opendir`/`getdents` per directory, which dominates a run on a slow file
//! system, while the index already lists every tracked file. [`TrackedFiles`] is that listing for one root, and
//! [`IndexedFs`] answers the listing of a tracked directory from it and hands everything else to the file system it
//! wraps. The JS analysis discovers source files through it, and the rules that list directories themselves (the
//! toolchain scan, the Bazel `BUILD` files of the lockfile consumers) reach it through the lint context.
//!
//! The index knows tracked files only, so by itself it is blind to a file that was created or deleted since the last
//! `git add`. A directory is therefore `stat`ed (no `opendir`) and compared with the modification time of the index
//! file. A directory modified at or after that time (equal counts as modified, the racy-git rule) is listed for real,
//! which finds untracked files and directories and drops deleted files; an older one cannot have gained or lost an
//! entry since the index was written and is served from the index. That is still blind to an untracked file created
//! before the index was last written (any `git add`, `git commit` or refreshing `git status` writes it), which the
//! index does not record, so only `[js] discovery = "git"` uses the overlay (ADR 0095). On a file system without
//! modification times the index alone is trusted.
//! [`TrackedFiles::for_discovery`] applies that choice.
//!
//! Whatever is listed is confirmed when it is used: [`IndexedFs`] answers existence checks from the real file system,
//! so a tracked file that was deleted is never reported as present.

use skif_core::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::SystemTime;

use skif_config::{Config, JsDiscovery};
use skif_core::{DirEntry, FileSystem, PathKind};

const HEADER_LEN: usize = 12;
/// Stat data (40 bytes), object id (20 bytes) and flags (2 bytes).
const FIXED_LEN: usize = 62;
const CHECKSUM_LEN: usize = 20;
const FLAG_EXTENDED: u16 = 0x4000;
const EXTENDED_SKIP_WORKTREE: u16 = 0x4000;
const MODE_TYPE_SHIFT: u32 = 12;
const TYPE_REGULAR: u32 = 0o10;
const TYPE_SYMLINK: u32 = 0o12;

/// The big-endian `u32` of `bytes` at `at`.
fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    Some(u32::from_be_bytes(<[u8; 4]>::try_from(bytes.get(at..end)?).ok()?))
}

fn be16(bytes: &[u8], at: usize) -> Option<u16> {
    let end = at.checked_add(2)?;
    Some(u16::from_be_bytes(<[u8; 2]>::try_from(bytes.get(at..end)?).ok()?))
}

/// The offset encoding of index format 4: seven bits per byte, most significant group first, with `1` added for every
/// continuation. Returns the value and the number of bytes used.
fn offset_varint(bytes: &[u8], at: usize) -> Option<(usize, usize)> {
    let mut byte = *bytes.get(at)?;
    let mut value = usize::from(byte & 0x7f);
    let mut used = 1;
    while byte & 0x80 != 0 {
        byte = *bytes.get(at.checked_add(used)?)?;
        used += 1;
        value = value
            .checked_add(1)?
            .checked_mul(128)?
            .checked_add(usize::from(byte & 0x7f))?;
    }
    Some((value, used))
}

/// The SHA-1 compression function over one 64-byte block.
fn sha1_block(state: &mut [u32; 5], block: &[u8; 64]) {
    let mut words = [0_u32; 80];
    for (word, chunk) in words.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_be_bytes(*chunk);
    }
    for at in 16..80 {
        words[at] = (words[at - 3] ^ words[at - 8] ^ words[at - 14] ^ words[at - 16]).rotate_left(1);
    }
    let [mut va, mut vb, mut vc, mut vd, mut ve] = *state;
    for (round, word) in words.iter().enumerate() {
        let (mix, constant) = match round {
            0..=19 => ((vb & vc) | (!vb & vd), 0x5A82_7999_u32),
            20..=39 => (vb ^ vc ^ vd, 0x6ED9_EBA1),
            40..=59 => ((vb & vc) | (vb & vd) | (vc & vd), 0x8F1B_BCDC),
            _ => (vb ^ vc ^ vd, 0xCA62_C1D6),
        };
        let next = va
            .rotate_left(5)
            .wrapping_add(mix)
            .wrapping_add(ve)
            .wrapping_add(constant)
            .wrapping_add(*word);
        ve = vd;
        vd = vc;
        vc = vb.rotate_left(30);
        vb = va;
        va = next;
    }
    for (slot, value) in state.iter_mut().zip([va, vb, vc, vd, ve]) {
        *slot = slot.wrapping_add(value);
    }
}

/// The SHA-1 digest of `data`, which is what the trailer of an index holds.
fn sha1(data: &[u8]) -> [u8; CHECKSUM_LEN] {
    let mut state = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    let (blocks, remainder) = data.as_chunks::<64>();
    for block in blocks {
        sha1_block(&mut state, block);
    }
    let mut tail = remainder.to_vec();
    tail.push(0x80);
    while tail.len() % 64 != 56 {
        tail.push(0);
    }
    tail.extend((u64::try_from(data.len()).unwrap_or(u64::MAX).wrapping_mul(8)).to_be_bytes());
    for block in tail.as_chunks::<64>().0 {
        sha1_block(&mut state, block);
    }
    let mut digest = [0; CHECKSUM_LEN];
    for (out, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        out.copy_from_slice(&word.to_be_bytes());
    }
    digest
}

/// Whether the trailer of `bytes` is the SHA-1 of what precedes it. An all-zero trailer is accepted: git writes one
/// when `index.skipHash` is on (`feature.manyFiles`), so it says nothing either way. An index of a SHA-256 repository
/// ends in a 32-byte digest and never passes.
fn checksum_matches(bytes: &[u8]) -> bool {
    let Some(split) = bytes.len().checked_sub(CHECKSUM_LEN) else {
        return false;
    };
    let (body, trailer) = bytes.split_at(split);
    trailer.iter().all(|byte| *byte == 0) || sha1(body) == trailer
}

/// Whether the `[extensions]` section of a git configuration selects `objectformat = sha256`.
fn selects_sha256(config: &str) -> bool {
    let mut in_extensions = false;
    for line in config.lines().map(str::trim) {
        if let Some(section) = line.strip_prefix('[') {
            let name = section.split([']', ' ', '"']).next().unwrap_or_default();
            in_extensions = name.eq_ignore_ascii_case("extensions");
        } else if in_extensions && let Some((key, value)) = line.split_once('=') {
            let value = value
                .split(['#', ';'])
                .next()
                .unwrap_or_default()
                .trim()
                .trim_matches('"');
            if key.trim().eq_ignore_ascii_case("objectformat") && value.eq_ignore_ascii_case("sha256") {
                return true;
            }
        }
    }
    false
}

/// The paths of the tracked files and symbolic links of an index, in index order, in one buffer.
#[derive(Debug, Default, PartialEq, Eq)]
struct TrackedPaths {
    names: String,
    /// Where each path ends in `names`; it starts where the previous one ends.
    ends: Vec<PathEnd>,
    /// Whether a path sorts before the one preceding it. Git writes its index in byte order, which a lookup by
    /// directory relies on; an index that is not, is scanned in full.
    unsorted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PathEnd {
    end: usize,
    symlink: bool,
}

impl TrackedPaths {
    /// Appends `path` unless it repeats the previous one (the stages of a conflicted file).
    fn push(&mut self, path: &str, symlink: bool) {
        let last = self.iter().next_back().map(|(last, _)| last);
        if last == Some(path) {
            return;
        }
        if last.is_some_and(|last| last > path) {
            self.unsorted = true;
        }
        self.names.push_str(path);
        self.ends.push(PathEnd {
            end: self.names.len(),
            symlink,
        });
    }

    fn start_of(&self, position: usize) -> usize {
        position.checked_sub(1).map_or(0, |before| self.ends[before].end)
    }

    /// The paths below the directory `prefix` (without a trailing separator; empty for all), relative to it, with
    /// whether each is a symbolic link.
    ///
    /// The paths of one directory are adjacent in a sorted index, so the range is found by bisection, not by testing
    /// every path.
    fn below<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = (&'a str, bool)> {
        let (first, last) = if prefix.is_empty() || self.unsorted {
            (0, self.ends.len())
        } else {
            let at = |position: usize| &self.names[self.start_of(position)..self.ends[position].end];
            let start = format!("{prefix}/");
            let first = partition(self.ends.len(), |position| at(position) < start.as_str());
            let last = first
                + partition(self.ends.len() - first, |position| {
                    at(first + position).starts_with(&start)
                });
            (first, last)
        };
        self.ends[first..last]
            .iter()
            .enumerate()
            .filter_map(move |(offset, path)| {
                let position = first + offset;
                let full = &self.names[self.start_of(position)..path.end];
                let relative = if prefix.is_empty() {
                    full
                } else {
                    full.strip_prefix(prefix)?.strip_prefix('/')?
                };
                Some((relative, path.symlink))
            })
    }

    /// The paths with whether each is a symbolic link.
    fn iter(&self) -> impl DoubleEndedIterator<Item = (&str, bool)> {
        self.ends
            .iter()
            .enumerate()
            .map(|(position, path)| (&self.names[self.start_of(position)..path.end], path.symlink))
    }
}

/// The number of leading positions of `0..len` for which `predicate` holds, which must hold for a prefix only.
fn partition(len: usize, predicate: impl Fn(usize) -> bool) -> usize {
    let (mut low, mut high) = (0, len);
    while low < high {
        let middle = low + (high - low) / 2;
        if predicate(middle) {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    low
}

/// The tracked files and symbolic links of an index, in index order. `None` when the index cannot be used: another
/// format, a truncated file or one whose checksum does not match, or a split or sparse index that does not list every
/// file itself.
///
/// Submodules (`gitlink` entries) and files outside a sparse checkout (`skip-worktree`) are left out, a conflicted
/// file is listed once, and entries whose path is not UTF-8 are skipped.
fn parse(bytes: &[u8]) -> Option<TrackedPaths> {
    if bytes.get(..4)? != b"DIRC" || !checksum_matches(bytes) {
        return None;
    }
    let version = be32(bytes, 4)?;
    if !(2..=4).contains(&version) {
        return None;
    }
    let count = usize::try_from(be32(bytes, 8)?).ok()?;
    let end = bytes.len().checked_sub(CHECKSUM_LEN)?;
    let capacity = count.min(1 << 21);
    let mut tracked = TrackedPaths {
        names: String::with_capacity(capacity.saturating_mul(48)),
        ends: Vec::with_capacity(capacity),
        unsorted: false,
    };
    let mut previous: Vec<u8> = Vec::new();
    let mut pos = HEADER_LEN;
    for _ in 0..count {
        let start = pos;
        let mode = be32(bytes, start.checked_add(24)?)?;
        let flags = be16(bytes, start.checked_add(60)?)?;
        let mut cursor = start.checked_add(FIXED_LEN)?;
        let mut extended = 0;
        if flags & FLAG_EXTENDED != 0 {
            if version < 3 {
                return None;
            }
            extended = be16(bytes, cursor)?;
            cursor += 2;
        }
        let name: &[u8] = if version == 4 {
            let (strip, used) = offset_varint(bytes, cursor)?;
            cursor += used;
            let nul = bytes.get(cursor..end)?.iter().position(|byte| *byte == 0)?;
            previous.truncate(previous.len().checked_sub(strip)?);
            previous.extend_from_slice(&bytes[cursor..cursor + nul]);
            pos = cursor + nul + 1;
            &previous
        } else {
            let nul = bytes.get(cursor..end)?.iter().position(|byte| *byte == 0)?;
            // Entries are NUL padded to a multiple of eight bytes, counted from the start of the entry.
            pos = start.checked_add((cursor - start + nul + 8) & !7)?;
            &bytes[cursor..cursor + nul]
        };
        if pos > end {
            return None;
        }
        let kind = mode >> MODE_TYPE_SHIFT;
        let wanted = (kind == TYPE_REGULAR || kind == TYPE_SYMLINK) && extended & EXTENDED_SKIP_WORKTREE == 0;
        if wanted && let Ok(path) = std::str::from_utf8(name) {
            tracked.push(path, kind == TYPE_SYMLINK);
        }
    }
    // Extensions follow the entries; a lowercase signature marks one the reader must understand (`link` for a split
    // index, `sdir` for a sparse one), and neither is supported.
    while pos + 8 <= end {
        if bytes.get(pos)?.is_ascii_lowercase() {
            return None;
        }
        let size = usize::try_from(be32(bytes, pos + 4)?).ok()?;
        pos = pos.checked_add(8)?.checked_add(size)?;
    }
    Some(tracked)
}

/// The parsed indexes of a run, so that analysing several roots of one repository, or one root by several providers,
/// reads and parses its index once.
///
/// An entry is reused while the index file keeps its modification time; a file system that does not report one is
/// never cached.
#[derive(Default)]
pub struct IndexCache {
    entries: Mutex<HashMap<PathBuf, CachedIndex>>,
}

impl std::fmt::Debug for IndexCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexCache").finish_non_exhaustive()
    }
}

struct CachedIndex {
    modified: SystemTime,
    /// `None` for an index that cannot be used, so it is not read again either.
    paths: Option<Arc<TrackedPaths>>,
}

impl IndexCache {
    /// The parsed index at `index`. The lock is not held while the index is read and parsed, so a provider never waits
    /// for another's parse (two concurrent first reads both parse, and store the same result).
    fn get(&self, fs: &dyn FileSystem, index: &Path, modified: Option<SystemTime>) -> Option<Arc<TrackedPaths>> {
        if let Some(modified) = modified
            && let Some(cached) = self
                .entries
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(index)
                .filter(|cached| cached.modified == modified)
        {
            return cached.paths.clone();
        }
        let paths = fs.read_bytes(index).ok().and_then(|bytes| parse(&bytes)).map(Arc::new);
        if let Some(modified) = modified {
            self.entries.lock().unwrap_or_else(PoisonError::into_inner).insert(
                index.to_path_buf(),
                CachedIndex {
                    modified,
                    paths: paths.clone(),
                },
            );
        }
        paths
    }
}

/// The directories of the tracked tree under a root, with the entries of each.
#[derive(Debug, Default)]
struct TrackedTree {
    dirs: HashMap<PathBuf, Arc<[DirEntry]>>,
}

fn join(root: &Path, relative: &str) -> PathBuf {
    if relative.is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    }
}

/// The directories being collected: an id per directory path, so that adding a file to the current directory costs
/// no hash lookup.
struct Collector<'a> {
    root: &'a Path,
    ids: HashMap<PathBuf, usize>,
    lists: Vec<Vec<DirEntry>>,
}

impl Collector<'_> {
    /// The id of `dir` (relative to the root), making it and its ancestors known, each listed in its parent.
    fn directory(&mut self, dir: &str) -> usize {
        let mut missing = Vec::new();
        let mut current = dir;
        let known = loop {
            if let Some(id) = self.ids.get(&join(self.root, current)) {
                break Some(*id);
            }
            missing.push(current);
            if current.is_empty() {
                break None;
            }
            current = current.rsplit_once('/').map_or("", |(parent, _)| parent);
        };
        let mut parent = known;
        for added in missing.into_iter().rev() {
            let path = join(self.root, added);
            let id = self.lists.len();
            self.lists.push(Vec::new());
            self.ids.insert(path.clone(), id);
            if let Some(parent) = parent {
                self.lists[parent].push(DirEntry::new(path, true, false));
            }
            parent = Some(id);
        }
        parent.unwrap_or_default()
    }
}

impl TrackedTree {
    /// The entries below `base`, the root's path from the work tree root.
    fn build(root: &Path, base: &Path, tracked: &TrackedPaths) -> Self {
        let prefix = base.to_string_lossy().replace('\\', "/");
        let prefix = prefix.trim_matches('/');
        let mut collector = Collector {
            root,
            ids: HashMap::default(),
            lists: Vec::new(),
        };
        let mut last_dir = "";
        let mut current: Option<usize> = None;
        for (relative, symlink) in tracked.below(prefix) {
            let dir = relative.rsplit_once('/').map_or("", |(dir, _)| dir);
            let id = match current {
                Some(id) if last_dir == dir => id,
                _ => {
                    let id = collector.directory(dir);
                    last_dir = dir;
                    current = Some(id);
                    id
                }
            };
            let file = DirEntry::new(join(root, relative), false, !symlink);
            collector.lists[id].push(if symlink { file.symlink() } else { file });
        }
        let Collector { ids, mut lists, .. } = collector;
        let dirs = ids
            .into_iter()
            .map(|(dir, id)| {
                let mut entries = std::mem::take(&mut lists[id]);
                // Entries share one parent, so raw path order is the component order the listings promise.
                entries.sort_unstable_by(|a, b| a.path.as_os_str().cmp(b.path.as_os_str()));
                entries.dedup_by(|a, b| a.path == b.path);
                (dir, Arc::from(entries))
            })
            .collect();
        Self { dirs }
    }

    /// The tracked entries of `dir`, if it is a tracked directory.
    fn listing(&self, dir: &Path) -> Option<&Arc<[DirEntry]>> {
        self.dirs.get(dir)
    }
}

/// The work tree root and the git directory that belong to `root`: the closest ancestor with a `.git` directory, or
/// a `.git` file pointing at one (linked work trees and submodules). `None` outside a git work tree.
#[must_use]
pub fn git_dirs(fs: &dyn FileSystem, root: &Path) -> Option<(PathBuf, PathBuf)> {
    for dir in root.ancestors() {
        let dot_git = dir.join(".git");
        let kind = fs.kind(&dot_git);
        if kind.is_dir {
            return Some((dir.to_path_buf(), dot_git));
        }
        if kind.is_file {
            let text = fs.read_to_string(&dot_git).ok()?;
            let target = text.lines().find_map(|line| line.strip_prefix("gitdir:"))?.trim();
            return Some((dir.to_path_buf(), dir.join(target)));
        }
    }
    None
}

/// The directory of the repository `git_dir` belongs to that holds what its work trees share (`config`,
/// `info/exclude`): a linked work tree names it in `commondir`, any other git directory is its own.
#[must_use]
pub fn git_common_dir(fs: &dyn FileSystem, git_dir: &Path) -> PathBuf {
    fs.read_to_string(&git_dir.join("commondir"))
        .ok()
        .map_or_else(|| git_dir.to_path_buf(), |text| git_dir.join(text.trim()))
}

/// The configuration file of the repository `git_dir` belongs to; a linked work tree keeps it in the common directory.
fn config_path(fs: &dyn FileSystem, git_dir: &Path) -> PathBuf {
    git_common_dir(fs, git_dir).join("config")
}

/// The files git tracks below one root, by directory, with when the listing of a directory may be used: the listing
/// [`IndexedFs`] serves.
#[derive(Debug)]
pub struct TrackedFiles {
    tree: TrackedTree,
    trust: Trust,
}

impl TrackedFiles {
    /// The files tracked below `root`, read through `fs` and `cache`, or `None` when `root` is not in a git work
    /// tree, the index cannot be read, verified or understood (another format or hash width, a split or sparse
    /// index) or nothing below `root` is tracked. A directory modified since the index was written is listed from
    /// disk; when `fs` reports no modification time for the index, every tracked directory is listed from the index.
    #[must_use]
    pub fn read(fs: &dyn FileSystem, root: &Path, cache: &IndexCache) -> Option<Self> {
        let (work_tree, git_dir) = git_dirs(fs, root)?;
        if fs
            .read_to_string(&config_path(fs, &git_dir))
            .is_ok_and(|config| selects_sha256(&config))
        {
            return None;
        }
        let index = git_dir.join("index");
        let modified = fs.modified(&index);
        let tracked = cache.get(fs, &index, modified)?;
        let tree = TrackedTree::build(root, root.strip_prefix(&work_tree).ok()?, &tracked);
        tree.dirs.contains_key(root).then_some(())?;
        let trust = modified.map_or(Trust::Always, Trust::ChangedSince);
        Some(Self { tree, trust })
    }

    /// [`read`](Self::read) when `config` lists files from the git index: `[js] discovery = "git"` without
    /// `no_gitignore`. `None` for `auto` and `walk`, which walk the directories, because the index cannot tell
    /// whether an unchanged directory holds an untracked file created before it was last written (ADR 0095), and
    /// with `no_gitignore`, because the index lists no ignored file.
    #[must_use]
    pub fn for_discovery(fs: &dyn FileSystem, root: &Path, config: &Config, cache: &IndexCache) -> Option<Self> {
        if config.js.no_gitignore || config.js.discovery != JsDiscovery::Git {
            return None;
        }
        Self::read(fs, root, cache)
    }
}

/// When the index listing of a directory may be used.
#[derive(Debug, Clone, Copy)]
enum Trust {
    Always,
    /// Unless the directory was modified at or after this time.
    ChangedSince(SystemTime),
}

/// A file system that lists tracked directories from [`TrackedFiles`] and defers everything else to `inner`.
///
/// A path the index does not list, a directory that changed since the index was written and every existence check is
/// asked of `inner`, so untracked and ignored files stay reachable by path (`node_modules`, generated sources) and a
/// file deleted from disk is never reported as present. A tracked symbolic link is listed the way a directory listing
/// of `inner` reports it, as a file or a directory according to its target. `inner` is any file system, owned
/// (`Arc<dyn FileSystem>`) or borrowed (`&dyn FileSystem`).
pub struct IndexedFs<F> {
    inner: F,
    tracked: Arc<TrackedFiles>,
    /// The listing served for each tracked directory, decided once. Read far more often than written: every listing
    /// of a tracked directory after the first is a read.
    served: RwLock<HashMap<PathBuf, Arc<[DirEntry]>>>,
}

impl<F: FileSystem> IndexedFs<F> {
    /// `inner` with the directories of `tracked` listed from the index.
    #[must_use]
    pub fn new(inner: F, tracked: Arc<TrackedFiles>) -> Self {
        Self {
            inner,
            tracked,
            served: RwLock::new(HashMap::default()),
        }
    }

    /// The entries of the tracked directory `dir`, whose index listing is `indexed`.
    fn serve(&self, dir: &Path, indexed: &Arc<[DirEntry]>) -> io::Result<Arc<[DirEntry]>> {
        if let Trust::ChangedSince(index_modified) = self.tracked.trust
            && self
                .inner
                .modified(dir)
                .is_none_or(|modified| modified >= index_modified)
        {
            return self.inner.read_dir_shared(dir);
        }
        if !indexed.iter().any(|entry| entry.is_symlink) {
            return Ok(Arc::clone(indexed));
        }
        Ok(indexed
            .iter()
            .map(|entry| {
                if !entry.is_symlink {
                    return entry.clone();
                }
                let kind = self.inner.kind(&entry.path);
                DirEntry::new(entry.path.clone(), kind.is_dir, kind.is_file).symlink()
            })
            .collect())
    }
}

impl<F: FileSystem> FileSystem for IndexedFs<F> {
    fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
        self.inner.read_to_string(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.read_dir_shared(path).map(|entries| entries.to_vec())
    }

    fn read_dir_shared(&self, path: &Path) -> io::Result<Arc<[DirEntry]>> {
        let Some(indexed) = self.tracked.tree.listing(path) else {
            return self.inner.read_dir_shared(path);
        };
        if let Some(served) = self.served.read().unwrap_or_else(PoisonError::into_inner).get(path) {
            return Ok(Arc::clone(served));
        }
        let listing = self.serve(path, indexed)?;
        let mut served = self.served.write().unwrap_or_else(PoisonError::into_inner);
        Ok(Arc::clone(served.entry(path.to_path_buf()).or_insert(listing)))
    }

    fn is_file(&self, path: &Path) -> bool {
        self.inner.is_file(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.inner.is_dir(path)
    }

    fn kind(&self, path: &Path) -> PathKind {
        self.inner.kind(path)
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
    use std::time::{Duration, UNIX_EPOCH};

    use skif_core::MemoryFs;

    use super::*;

    const REGULAR: u32 = 0o100_644;
    const SYMLINK: u32 = 0o120_000;
    const GITLINK: u32 = 0o160_000;

    struct Item {
        path: &'static str,
        mode: u32,
        stage: u16,
        skip_worktree: bool,
    }

    fn item(path: &'static str, mode: u32) -> Item {
        Item {
            path,
            mode,
            stage: 0,
            skip_worktree: false,
        }
    }

    fn entry_head(item: &Item, version: u32) -> Vec<u8> {
        let mut out = vec![0; 24];
        out.extend(item.mode.to_be_bytes());
        out.extend([0; 12 + 20]);
        let mut flags = item.stage << 12;
        let extended = item.skip_worktree;
        if extended {
            flags |= FLAG_EXTENDED;
        }
        if version != 4 {
            flags |= u16::try_from(item.path.len().min(0xfff)).unwrap();
        }
        out.extend(flags.to_be_bytes());
        if extended {
            out.extend(EXTENDED_SKIP_WORKTREE.to_be_bytes());
        }
        out
    }

    /// An index of `version` holding `items`, followed by `extensions` and the SHA-1 of everything before.
    fn index(version: u32, items: &[Item], extensions: &[u8]) -> Vec<u8> {
        let mut out = b"DIRC".to_vec();
        out.extend(version.to_be_bytes());
        out.extend(u32::try_from(items.len()).unwrap().to_be_bytes());
        let mut previous = "";
        for item in items {
            let start = out.len();
            out.extend(entry_head(item, version));
            if version == 4 {
                let common = previous
                    .bytes()
                    .zip(item.path.bytes())
                    .take_while(|(a, b)| a == b)
                    .count();
                let strip = previous.len() - common;
                assert!(strip < 128);
                out.push(u8::try_from(strip).unwrap());
                out.extend(&item.path.as_bytes()[common..]);
                out.push(0);
                previous = item.path;
            } else {
                out.extend(item.path.as_bytes());
                let size = (out.len() - start + 8) & !7;
                let padded = start + size;
                out.resize(padded, 0);
            }
        }
        out.extend(extensions);
        let checksum = sha1(&out);
        out.extend(checksum);
        out
    }

    fn with_trailer(mut bytes: Vec<u8>, trailer: &[u8]) -> Vec<u8> {
        bytes.truncate(bytes.len() - CHECKSUM_LEN);
        bytes.extend(trailer);
        bytes
    }

    fn hex(digest: &[u8]) -> String {
        use std::fmt::Write as _;
        digest.iter().fold(String::new(), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
    }

    fn paths(tracked: &TrackedPaths) -> Vec<&str> {
        tracked.iter().map(|(path, _)| path).collect()
    }

    fn tracked(paths: &[&str]) -> TrackedPaths {
        let mut out = TrackedPaths::default();
        for path in paths {
            out.push(path, false);
        }
        out
    }

    fn below<'a>(tracked: &'a TrackedPaths, prefix: &'a str) -> Vec<&'a str> {
        tracked.below(prefix).map(|(path, _)| path).collect()
    }

    /// `fs` with the files tracked below `root` listed from the index, as the JS analysis builds it.
    fn indexed_fs(fs: &Arc<dyn FileSystem>, root: &Path, cache: &IndexCache) -> Option<Arc<dyn FileSystem>> {
        let tracked = TrackedFiles::read(fs.as_ref(), root, cache)?;
        Some(Arc::new(IndexedFs::new(Arc::clone(fs), Arc::new(tracked))))
    }

    #[test]
    fn the_paths_of_one_directory_are_found_by_bisection() {
        let files = tracked(&["a.ts", "app-2/q.ts", "app.ts", "app/x.ts", "app/y/z.ts", "b/c.ts"]);
        assert!(!files.unsorted);
        assert_eq!(below(&files, "app"), ["x.ts", "y/z.ts"]);
        assert_eq!(below(&files, "app/y"), ["z.ts"]);
        assert_eq!(below(&files, "b"), ["c.ts"]);
        assert_eq!(below(&files, "missing"), Vec::<&str>::new());
        assert_eq!(below(&files, "ap"), Vec::<&str>::new());
        assert_eq!(below(&files, "").len(), 6);
    }

    #[test]
    fn an_index_that_is_not_in_byte_order_is_scanned_in_full() {
        let files = tracked(&["b/x.ts", "a/y.ts", "b/z.ts"]);
        assert!(files.unsorted);
        assert_eq!(below(&files, "b"), ["x.ts", "z.ts"]);
    }

    #[test]
    fn sha1_matches_the_published_vectors() {
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            hex(&sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        assert_eq!(
            hex(&sha1("a".repeat(1_000_000).as_bytes())),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
    }

    #[test]
    fn version_two_entries_are_read_with_their_padding() {
        let bytes = index(
            2,
            &[
                item("a.ts", REGULAR),
                item("src/index.ts", REGULAR),
                item("src/link.ts", SYMLINK),
                item("vendor/sub", GITLINK),
                item("z", REGULAR),
            ],
            b"",
        );
        let tracked = parse(&bytes).unwrap();
        assert_eq!(paths(&tracked), ["a.ts", "src/index.ts", "src/link.ts", "z"]);
        let symlinks: Vec<bool> = tracked.iter().map(|(_, symlink)| symlink).collect();
        assert_eq!(symlinks, [false, false, true, false]);
    }

    #[test]
    fn version_four_prefix_compression_is_expanded() {
        let items = [
            item("a/b/one.ts", REGULAR),
            item("a/b/two.ts", REGULAR),
            item("a/c.ts", REGULAR),
            item("d.ts", REGULAR),
        ];
        let tracked = parse(&index(4, &items, b"")).unwrap();
        assert_eq!(paths(&tracked), ["a/b/one.ts", "a/b/two.ts", "a/c.ts", "d.ts"]);
    }

    #[test]
    fn a_conflicted_file_is_listed_once_and_sparse_entries_are_left_out() {
        let staged = |stage| Item {
            stage,
            ..item("merge.ts", REGULAR)
        };
        let mut sparse = item("outside.ts", REGULAR);
        sparse.skip_worktree = true;
        let bytes = index(
            3,
            &[staged(1), staged(2), staged(3), item("ok.ts", REGULAR), sparse],
            b"",
        );
        assert_eq!(paths(&parse(&bytes).unwrap()), ["merge.ts", "ok.ts"]);
    }

    #[test]
    fn optional_extensions_are_skipped_and_required_ones_refuse_the_index() {
        let mut tree = b"TREE".to_vec();
        tree.extend(3_u32.to_be_bytes());
        tree.extend(b"abc");
        let items = [item("a.ts", REGULAR)];
        assert_eq!(paths(&parse(&index(2, &items, &tree)).unwrap()), ["a.ts"]);

        let mut split = b"link".to_vec();
        split.extend(0_u32.to_be_bytes());
        assert!(parse(&index(2, &items, &split)).is_none());
        let mut sparse = b"sdir".to_vec();
        sparse.extend(0_u32.to_be_bytes());
        assert!(parse(&index(2, &items, &sparse)).is_none());
    }

    #[test]
    fn unusable_indexes_are_refused() {
        let good = index(2, &[item("a.ts", REGULAR)], b"");
        assert!(parse(&good).is_some());
        assert!(parse(&good[..good.len() - 30]).is_none());
        assert!(parse(b"").is_none());
        let mut wrong_signature = good.clone();
        wrong_signature[0] = b'X';
        assert!(parse(&wrong_signature).is_none());
        let mut wrong_version = good;
        wrong_version[7] = 9;
        assert!(parse(&wrong_version).is_none());
    }

    #[test]
    fn the_checksum_must_match_unless_the_hash_was_skipped() {
        let good = index(2, &[item("a.ts", REGULAR), item("b.ts", REGULAR)], b"");
        let mut torn = good.clone();
        torn[HEADER_LEN + 3] ^= 1;
        assert!(parse(&torn).is_none());
        assert!(parse(&with_trailer(good.clone(), &[9; CHECKSUM_LEN])).is_none());
        assert_eq!(
            paths(&parse(&with_trailer(good.clone(), &[0; CHECKSUM_LEN])).unwrap()),
            ["a.ts", "b.ts"]
        );
        // A SHA-256 repository ends its index in a 32-byte digest.
        let sha256 = {
            let mut bytes = good[..good.len() - CHECKSUM_LEN].to_vec();
            bytes.extend([0xab; 32]);
            bytes
        };
        assert!(parse(&sha256).is_none());
    }

    #[test]
    fn the_object_format_is_read_from_the_extensions_section() {
        assert!(selects_sha256(
            "[core]\n\tbare = false\n[extensions]\n\tobjectformat = sha256\n"
        ));
        assert!(selects_sha256("[extensions]\nObjectFormat=\"SHA256\" # new\n"));
        assert!(!selects_sha256("[extensions]\n\tobjectformat = sha1\n"));
        assert!(!selects_sha256("[core]\n\tobjectformat = sha256\n"));
        assert!(!selects_sha256("[extensions]\n[core]\n\tobjectformat = sha256\n"));
        assert!(!selects_sha256(""));
    }

    fn names(entries: &[DirEntry]) -> Vec<(String, bool)> {
        entries
            .iter()
            .map(|entry| {
                (
                    entry.path.file_name().unwrap().to_string_lossy().into_owned(),
                    entry.is_dir,
                )
            })
            .collect()
    }

    #[test]
    fn the_tree_lists_directories_with_sorted_entries() {
        let files = tracked(&["a-b/x.ts", "a/y.ts", "a/z/deep.ts", "top.ts", "a.ts"]);
        let tree = TrackedTree::build(Path::new("/ws"), Path::new(""), &files);
        assert_eq!(
            names(tree.listing(Path::new("/ws")).unwrap()),
            [
                ("a".into(), true),
                ("a-b".into(), true),
                ("a.ts".into(), false),
                ("top.ts".into(), false)
            ]
        );
        assert_eq!(
            names(tree.listing(Path::new("/ws/a")).unwrap()),
            [("y.ts".into(), false), ("z".into(), true)]
        );
        assert!(tree.listing(Path::new("/ws/missing")).is_none());
    }

    #[test]
    fn a_root_below_the_work_tree_sees_only_its_own_files() {
        let files = tracked(&["README.md", "app/package.json", "app/src/a.ts", "app-two/b.ts"]);
        let tree = TrackedTree::build(Path::new("/repo/app"), Path::new("app"), &files);
        assert_eq!(
            names(tree.listing(Path::new("/repo/app")).unwrap()),
            [("package.json".into(), false), ("src".into(), true)]
        );
        assert_eq!(tree.dirs.len(), 2);
    }

    #[test]
    fn duplicate_paths_are_listed_once() {
        let files = tracked(&["a.ts", "a.ts", "b/c.ts", "b/c.ts"]);
        let tree = TrackedTree::build(Path::new("/ws"), Path::new(""), &files);
        assert_eq!(tree.listing(Path::new("/ws")).unwrap().len(), 2);
        assert_eq!(tree.listing(Path::new("/ws/b")).unwrap().len(), 1);
    }

    #[test]
    fn the_overlay_answers_listings_from_the_index_and_everything_else_from_disk() {
        let disk = MemoryFs::new()
            .with_file("/ws/src/a.ts", "")
            .with_file("/ws/src/untracked.ts", "")
            .with_file("/ws/node_modules/x/index.js", "");
        let tree = TrackedTree::build(Path::new("/ws"), Path::new(""), &tracked(&["src/a.ts", "gone.ts"]));
        let fs = IndexedFs::new(
            disk,
            Arc::new(TrackedFiles {
                tree,
                trust: Trust::Always,
            }),
        );
        assert!(fs.is_file(Path::new("/ws/src/a.ts")));
        assert!(
            !fs.is_file(Path::new("/ws/gone.ts")),
            "a tracked file missing on disk is absent"
        );
        assert_eq!(fs.kind(Path::new("/ws/gone.ts")), PathKind::NONE);
        assert!(fs.is_dir(Path::new("/ws/src")) && !fs.is_file(Path::new("/ws/src")));
        assert!(fs.is_file(Path::new("/ws/src/untracked.ts")));
        assert!(fs.is_dir(Path::new("/ws/node_modules/x")));
        assert!(!fs.is_file(Path::new("/ws/src/nope.ts")));
        let listing = fs.read_dir(Path::new("/ws/src")).unwrap();
        assert_eq!(names(&listing), [("a.ts".into(), false)]);
        assert!(fs.read_dir(Path::new("/ws/node_modules")).is_ok());
        assert!(Arc::ptr_eq(
            &fs.read_dir_shared(Path::new("/ws/src")).unwrap(),
            &fs.read_dir_shared(Path::new("/ws/src")).unwrap()
        ));
    }

    /// A work tree on an in-memory disk with a given `.git/index` and modification times.
    struct Disk {
        files: MemoryFs,
        index: Vec<u8>,
        times: HashMap<PathBuf, SystemTime>,
        index_reads: AtomicUsize,
    }

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    impl FileSystem for Disk {
        fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
            self.files.read_to_string(path)
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
            self.files.read_dir(path)
        }

        fn is_file(&self, path: &Path) -> bool {
            self.files.is_file(path)
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.files.is_dir(path)
        }

        fn read_bytes(&self, path: &Path) -> io::Result<Vec<u8>> {
            if path.ends_with("index") {
                self.index_reads.fetch_add(1, Ordering::SeqCst);
                Ok(self.index.clone())
            } else {
                self.files.read_bytes(path)
            }
        }

        fn modified(&self, path: &Path) -> Option<SystemTime> {
            self.times.get(path).copied()
        }
    }

    const INDEX_TIME: u64 = 100;

    /// `/ws` with `.git`, the files in `on_disk`, an index listing `in_index` written at [`INDEX_TIME`] and the
    /// modification times in `times`, keyed by path below `/ws`.
    fn work_tree(on_disk: &[&str], in_index: &[&'static str], times: &[(&str, u64)]) -> Arc<Disk> {
        let files = on_disk.iter().fold(
            MemoryFs::new().with_file("/ws/.git/HEAD", "ref: refs/heads/main"),
            |files, path| files.with_file(Path::new("/ws").join(path), ""),
        );
        let items: Vec<Item> = in_index.iter().map(|path| item(path, REGULAR)).collect();
        let mut times: HashMap<PathBuf, SystemTime> = times
            .iter()
            .map(|(path, seconds)| (Path::new("/ws").join(path), at(*seconds)))
            .collect();
        times.insert(PathBuf::from("/ws/.git/index"), at(INDEX_TIME));
        Arc::new(Disk {
            files,
            index: index(2, &items, b""),
            times,
            index_reads: AtomicUsize::new(0),
        })
    }

    fn overlay(disk: &Arc<Disk>) -> Option<Arc<dyn FileSystem>> {
        let fs: Arc<dyn FileSystem> = disk.clone();
        indexed_fs(&fs, Path::new("/ws"), &IndexCache::default())
    }

    fn listed(fs: &dyn FileSystem, dir: &str) -> Vec<String> {
        fs.read_dir(Path::new(dir))
            .unwrap()
            .iter()
            .map(|entry| entry.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn a_file_deleted_but_not_staged_is_not_listed() {
        let disk = work_tree(&["src/a.ts"], &["src/a.ts", "src/gone.ts"], &[("src", 200)]);
        let fs = overlay(&disk).unwrap();
        assert_eq!(listed(fs.as_ref(), "/ws/src"), ["a.ts"]);
        assert!(!fs.is_file(Path::new("/ws/src/gone.ts")));
        assert_eq!(fs.kind(Path::new("/ws/src/gone.ts")), PathKind::NONE);
    }

    #[test]
    fn a_deleted_file_in_an_unchanged_directory_is_still_absent_when_looked_up() {
        // The directory is older than the index (the index was refreshed after the deletion), so the listing is the
        // index's; the existence check still sees the file is gone.
        let disk = work_tree(&["src/a.ts"], &["src/a.ts", "src/gone.ts"], &[("src", 50)]);
        let fs = overlay(&disk).unwrap();
        assert_eq!(listed(fs.as_ref(), "/ws/src"), ["a.ts", "gone.ts"]);
        assert_eq!(fs.kind(Path::new("/ws/src/gone.ts")), PathKind::NONE);
        assert_eq!(fs.kind(Path::new("/ws/src/a.ts")), PathKind::FILE);
    }

    #[test]
    fn a_new_untracked_file_is_listed_when_its_directory_changed() {
        let disk = work_tree(&["src/a.ts", "src/new.ts"], &["src/a.ts"], &[("src", 200)]);
        let fs = overlay(&disk).unwrap();
        assert_eq!(listed(fs.as_ref(), "/ws/src"), ["a.ts", "new.ts"]);
    }

    #[test]
    fn a_new_untracked_directory_is_listed_and_walked() {
        let disk = work_tree(
            &["src/a.ts", "fresh/deep/x.ts"],
            &["src/a.ts"],
            &[("", 200), ("src", 50)],
        );
        let fs = overlay(&disk).unwrap();
        assert_eq!(listed(fs.as_ref(), "/ws"), [".git", "fresh", "src"]);
        assert_eq!(listed(fs.as_ref(), "/ws/fresh"), ["deep"]);
        assert_eq!(listed(fs.as_ref(), "/ws/fresh/deep"), ["x.ts"]);
        assert_eq!(listed(fs.as_ref(), "/ws/src"), ["a.ts"]);
    }

    #[test]
    fn a_directory_modified_at_the_index_time_counts_as_changed() {
        let equal = work_tree(&["src/a.ts", "src/new.ts"], &["src/a.ts"], &[("src", INDEX_TIME)]);
        let fs = overlay(&equal).unwrap();
        assert_eq!(listed(fs.as_ref(), "/ws/src"), ["a.ts", "new.ts"]);
        let older = work_tree(&["src/a.ts", "src/new.ts"], &["src/a.ts"], &[("src", INDEX_TIME - 1)]);
        let fs = overlay(&older).unwrap();
        assert_eq!(listed(fs.as_ref(), "/ws/src"), ["a.ts"]);
    }

    #[test]
    fn a_directory_without_a_modification_time_is_listed_from_disk() {
        let disk = work_tree(&["src/a.ts", "src/new.ts"], &["src/a.ts"], &[]);
        let fs = overlay(&disk).unwrap();
        assert_eq!(listed(fs.as_ref(), "/ws/src"), ["a.ts", "new.ts"]);
    }

    #[test]
    fn without_modification_times_the_index_alone_is_trusted() {
        let disk = work_tree(
            &["src/a.ts", "src/new.ts"],
            &["src/a.ts", "src/gone.ts"],
            &[("src", 200)],
        );
        let bare = Arc::new(Disk {
            times: HashMap::default(),
            files: disk.files.clone(),
            index: disk.index.clone(),
            index_reads: AtomicUsize::new(0),
        });
        let fs = overlay(&bare).unwrap();
        assert_eq!(listed(fs.as_ref(), "/ws/src"), ["a.ts", "gone.ts"]);
        assert_eq!(fs.kind(Path::new("/ws/src/gone.ts")), PathKind::NONE);
        // With the times, the changed directory is listed from disk.
        let fs = overlay(&disk).unwrap();
        assert_eq!(listed(fs.as_ref(), "/ws/src"), ["a.ts", "new.ts"]);
    }

    #[test]
    fn a_later_pass_sees_the_files_the_fix_deleted() {
        let cache = IndexCache::default();
        let before = work_tree(
            &["src/a.ts", "src/dead.ts"],
            &["src/a.ts", "src/dead.ts"],
            &[("src", 50)],
        );
        let first: Arc<dyn FileSystem> = before.clone();
        let fs = indexed_fs(&first, Path::new("/ws"), &cache).unwrap();
        assert_eq!(listed(fs.as_ref(), "/ws/src"), ["a.ts", "dead.ts"]);

        let after = work_tree(&["src/a.ts"], &["src/a.ts", "src/dead.ts"], &[("src", 300)]);
        let second: Arc<dyn FileSystem> = after.clone();
        let fs = indexed_fs(&second, Path::new("/ws"), &cache).unwrap();
        assert_eq!(listed(fs.as_ref(), "/ws/src"), ["a.ts"]);
    }

    #[test]
    fn the_index_is_read_and_parsed_once_per_cache() {
        let disk = work_tree(&["src/a.ts"], &["src/a.ts"], &[("src", 50)]);
        let fs: Arc<dyn FileSystem> = disk.clone();
        let cache = IndexCache::default();
        for _ in 0..3 {
            assert!(indexed_fs(&fs, Path::new("/ws"), &cache).is_some());
        }
        assert_eq!(disk.index_reads.load(Ordering::SeqCst), 1);
        assert!(indexed_fs(&fs, Path::new("/ws"), &IndexCache::default()).is_some());
        assert_eq!(disk.index_reads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_rewritten_index_is_read_again() {
        let cache = IndexCache::default();
        let old = work_tree(&["a.ts"], &["a.ts"], &[]);
        let fs: Arc<dyn FileSystem> = old.clone();
        assert!(indexed_fs(&fs, Path::new("/ws"), &cache).is_some());
        let mut newer = work_tree(&["a.ts"], &["a.ts"], &[]);
        Arc::get_mut(&mut newer)
            .unwrap()
            .times
            .insert(PathBuf::from("/ws/.git/index"), at(INDEX_TIME + 5));
        let fs: Arc<dyn FileSystem> = newer.clone();
        assert!(indexed_fs(&fs, Path::new("/ws"), &cache).is_some());
        assert_eq!(newer.index_reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_sha256_repository_is_refused() {
        let mut disk = work_tree(&["a.ts"], &["a.ts"], &[]);
        {
            let disk = Arc::get_mut(&mut disk).unwrap();
            disk.files = disk.files.clone().with_file(
                "/ws/.git/config",
                "[core]\n\tbare = false\n[extensions]\n\tobjectformat = sha256\n",
            );
        }
        assert!(overlay(&disk).is_none());
        let mut sha1_repository = work_tree(&["a.ts"], &["a.ts"], &[]);
        {
            let disk = Arc::get_mut(&mut sha1_repository).unwrap();
            disk.files = disk
                .files
                .clone()
                .with_file("/ws/.git/config", "[core]\n\tbare = false\n");
        }
        assert!(overlay(&sha1_repository).is_some());
    }

    #[test]
    fn a_linked_work_tree_reads_the_configuration_of_the_common_directory() {
        let files = MemoryFs::new()
            .with_file("/ws/.git", "gitdir: /main/.git/worktrees/w\n")
            .with_file("/main/.git/worktrees/w/commondir", "/main/.git\n")
            .with_file("/main/.git/config", "[extensions]\n\tobjectformat = sha256\n")
            .with_file("/ws/a.ts", "");
        let disk = Arc::new(Disk {
            files,
            index: index(2, &[item("a.ts", REGULAR)], b""),
            times: [(PathBuf::from("/main/.git/worktrees/w/index"), at(INDEX_TIME))]
                .into_iter()
                .collect(),
            index_reads: AtomicUsize::new(0),
        });
        assert!(overlay(&disk).is_none());
    }

    #[test]
    fn a_root_without_tracked_files_or_a_repository_is_not_overlaid() {
        let disk = work_tree(&["a.ts"], &["other/a.ts"], &[]);
        let fs: Arc<dyn FileSystem> = disk.clone();
        let cache = IndexCache::default();
        assert!(indexed_fs(&fs, Path::new("/ws/app"), &cache).is_none());
        let plain: Arc<dyn FileSystem> = Arc::new(MemoryFs::new().with_file("/ws/a.ts", ""));
        assert!(indexed_fs(&plain, Path::new("/ws"), &cache).is_none());
    }

    #[test]
    fn a_git_file_points_at_the_git_directory() {
        let files = MemoryFs::new()
            .with_file("/repo/.git", "gitdir: /elsewhere/worktrees/w\n")
            .with_file("/repo/app/package.json", "{}");
        let disk = Arc::new(Disk {
            files,
            index: index(2, &[item("app/a.ts", REGULAR)], b""),
            times: HashMap::default(),
            index_reads: AtomicUsize::new(0),
        });
        let fs: Arc<dyn FileSystem> = disk;
        assert!(indexed_fs(&fs, Path::new("/repo/app"), &IndexCache::default()).is_some());
    }

    /// A directory below the system's temporary directory, removed when dropped.
    #[cfg(unix)]
    struct TempDir(PathBuf);

    #[cfg(unix)]
    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("skif-workspace-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    #[cfg(unix)]
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn tracked_symbolic_links_are_listed_like_the_disk_lists_them() {
        let dir = TempDir::new("tracked-symbolic-links");
        let root = dir.0.canonicalize().unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("shared")).unwrap();
        std::fs::write(root.join("src/real.ts"), "").unwrap();
        std::fs::write(root.join("shared/x.ts"), "").unwrap();
        std::os::unix::fs::symlink("real.ts", root.join("src/link.ts")).unwrap();
        std::os::unix::fs::symlink("../shared", root.join("src/dirlink")).unwrap();
        std::os::unix::fs::symlink("missing.ts", root.join("src/dangling.ts")).unwrap();
        let items = [
            item("shared/x.ts", REGULAR),
            item("src/dangling.ts", SYMLINK),
            item("src/dirlink", SYMLINK),
            item("src/link.ts", SYMLINK),
            item("src/real.ts", REGULAR),
        ];
        std::fs::write(root.join(".git/index"), index(2, &items, b"")).unwrap();
        let fs: Arc<dyn FileSystem> = Arc::new(skif_core::OsFs);
        let overlay = indexed_fs(&fs, &root, &IndexCache::default()).unwrap();
        for sub in ["src", "shared"] {
            assert_eq!(
                overlay.read_dir(&root.join(sub)).unwrap(),
                fs.read_dir(&root.join(sub)).unwrap(),
                "{sub}"
            );
        }
        let link = overlay
            .read_dir(&root.join("src"))
            .unwrap()
            .into_iter()
            .find(|entry| entry.path.ends_with("link.ts"))
            .unwrap();
        assert!(link.is_file && link.is_symlink);
    }
}
