//! The files git tracks, read from `.git/index` without running git, and a file system overlay that serves them.
//!
//! Walking a large work tree costs one `opendir`/`getdents` per directory, which dominates the analysis on a slow
//! file system, while the index already lists every tracked file. [`TrackedTree::load`] reads it (formats 2 to 4);
//! [`IndexedFs`] answers listings and existence checks of tracked paths from memory and hands everything else to the
//! file system it wraps.
//!
//! Only tracked files are listed. A file that was never `git add`ed is invisible to listings; a path outside the
//! tracked tree (`node_modules`, build output, untracked and ignored files) is still answered by the real file system.
//! A tracked file that was deleted from disk but not yet staged is listed until the index is updated.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use skif_core::{DirEntry, FileSystem, PathKind};

const HEADER_LEN: usize = 12;
/// Stat data (40 bytes), object id (20 bytes) and flags (2 bytes).
const FIXED_LEN: usize = 62;
const CHECKSUM_LEN: usize = 20;
const FLAG_EXTENDED: u16 = 0x4000;
const FLAG_STAGE: u16 = 0x3000;
const EXTENDED_SKIP_WORKTREE: u16 = 0x4000;
const MODE_TYPE_SHIFT: u32 = 12;
const TYPE_REGULAR: u32 = 0o10;
const TYPE_SYMLINK: u32 = 0o12;

/// A tracked path of the index.
#[derive(Debug, PartialEq, Eq)]
struct Tracked {
    /// Relative to the work tree root, `/`-separated.
    path: String,
    symlink: bool,
}

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
        value = value.checked_add(1)?.checked_mul(128)?.checked_add(usize::from(byte & 0x7f))?;
    }
    Some((value, used))
}

/// The tracked files and symbolic links of an index, in index order. `None` when the index cannot be used: another
/// format, a truncated file, or a split or sparse index that does not list every file itself.
///
/// Submodules (`gitlink` entries), conflicted duplicates and files outside a sparse checkout (`skip-worktree`) are
/// left out; entries whose path is not UTF-8 are skipped.
fn parse(bytes: &[u8]) -> Option<Vec<Tracked>> {
    if bytes.get(..4)? != b"DIRC" {
        return None;
    }
    let version = be32(bytes, 4)?;
    if !(2..=4).contains(&version) {
        return None;
    }
    let count = usize::try_from(be32(bytes, 8)?).ok()?;
    let end = bytes.len().checked_sub(CHECKSUM_LEN)?;
    let mut tracked = Vec::with_capacity(count.min(1 << 21));
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
        let wanted = (kind == TYPE_REGULAR || kind == TYPE_SYMLINK)
            && extended & EXTENDED_SKIP_WORKTREE == 0
            && flags & FLAG_STAGE == 0;
        if wanted && let Ok(path) = std::str::from_utf8(name) {
            tracked.push(Tracked {
                path: path.to_owned(),
                symlink: kind == TYPE_SYMLINK,
            });
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

/// The directories of the tracked tree under a root, with the entries of each.
#[derive(Debug, Default)]
pub(crate) struct TrackedTree {
    dirs: HashMap<PathBuf, Arc<[DirEntry]>>,
}

fn join(root: &Path, relative: &str) -> PathBuf {
    if relative.is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    }
}

impl TrackedTree {
    /// The tracked tree below `root`, or `None` when `root` is not in a git work tree, its index cannot be read, or
    /// nothing below `root` is tracked.
    pub(crate) fn load(fs: &dyn FileSystem, root: &Path) -> Option<Self> {
        let (work_tree, git_dir) = locate(fs, root)?;
        let bytes = fs.read_bytes(&git_dir.join("index")).ok()?;
        let tracked = parse(&bytes)?;
        let tree = Self::build(root, root.strip_prefix(&work_tree).ok()?, &tracked);
        tree.dirs.contains_key(root).then_some(tree)
    }

    /// The entries below `base`, the root's path from the work tree root.
    fn build(root: &Path, base: &Path, tracked: &[Tracked]) -> Self {
        let prefix = base.to_string_lossy().replace('\\', "/");
        let prefix = prefix.trim_matches('/');
        let mut dirs: HashMap<PathBuf, Vec<DirEntry>> = HashMap::new();
        let mut last_dir = String::new();
        let mut have_last = false;
        for entry in tracked {
            let relative = if prefix.is_empty() {
                entry.path.as_str()
            } else {
                let Some(rest) = entry.path.strip_prefix(prefix).and_then(|rest| rest.strip_prefix('/')) else {
                    continue;
                };
                rest
            };
            let dir = relative.rsplit_once('/').map_or("", |(dir, _)| dir);
            if !(have_last && last_dir == dir) {
                register(&mut dirs, root, dir);
                dir.clone_into(&mut last_dir);
                have_last = true;
            }
            let file = DirEntry::new(join(root, relative), false, !entry.symlink);
            let file = if entry.symlink { file.symlink() } else { file };
            if let Some(list) = dirs.get_mut(&join(root, dir)) {
                list.push(file);
            }
        }
        let dirs: HashMap<PathBuf, Arc<[DirEntry]>> = dirs
            .into_iter()
            .map(|(dir, mut entries)| {
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

    /// What `path` is according to the index; `None` for a path the index does not list and for symbolic links, whose
    /// target the index does not know.
    fn kind(&self, path: &Path) -> Option<PathKind> {
        if self.dirs.contains_key(path) {
            return Some(PathKind::DIR);
        }
        let entries = self.dirs.get(path.parent()?)?;
        let found = entries
            .binary_search_by(|entry| entry.path.as_path().cmp(path))
            .ok()
            .map(|index| &entries[index])?;
        (!found.is_symlink).then_some(PathKind {
            is_file: found.is_file,
            is_dir: found.is_dir,
        })
    }
}

/// Makes `dir` (relative to `root`) and its ancestors known, each listed in its parent.
fn register(dirs: &mut HashMap<PathBuf, Vec<DirEntry>>, root: &Path, dir: &str) {
    let mut missing = Vec::new();
    let mut current = dir;
    while !dirs.contains_key(&join(root, current)) {
        missing.push(current);
        if current.is_empty() {
            break;
        }
        current = current.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
    for added in missing.into_iter().rev() {
        let path = join(root, added);
        dirs.insert(path.clone(), Vec::new());
        if !added.is_empty() {
            let parent = added.rsplit_once('/').map_or("", |(parent, _)| parent);
            if let Some(list) = dirs.get_mut(&join(root, parent)) {
                list.push(DirEntry::new(path, true, false));
            }
        }
    }
}

/// The work tree root and the git directory that belong to `root`: the closest ancestor with a `.git` directory, or
/// a `.git` file pointing at one (linked work trees and submodules).
fn locate(fs: &dyn FileSystem, root: &Path) -> Option<(PathBuf, PathBuf)> {
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

/// A file system that lists and checks tracked paths from a [`TrackedTree`] and defers everything else to `inner`.
///
/// A listing of a tracked directory holds its tracked entries only. A path the index does not list is asked of
/// `inner`, so untracked and ignored files stay reachable by path (`node_modules`, generated sources).
pub(crate) struct IndexedFs {
    inner: Arc<dyn FileSystem>,
    tree: TrackedTree,
}

impl IndexedFs {
    pub(crate) fn new(inner: Arc<dyn FileSystem>, tree: TrackedTree) -> Self {
        Self { inner, tree }
    }
}

impl FileSystem for IndexedFs {
    fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
        self.inner.read_to_string(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.read_dir_shared(path).map(|entries| entries.to_vec())
    }

    fn read_dir_shared(&self, path: &Path) -> io::Result<Arc<[DirEntry]>> {
        self.tree
            .listing(path)
            .map_or_else(|| self.inner.read_dir_shared(path), |entries| Ok(Arc::clone(entries)))
    }

    fn is_file(&self, path: &Path) -> bool {
        self.kind(path).is_file
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.kind(path).is_dir
    }

    fn kind(&self, path: &Path) -> PathKind {
        self.tree.kind(path).unwrap_or_else(|| self.inner.kind(path))
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

    /// An index of `version` holding `items`, followed by `extensions` and a dummy checksum.
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
        out.extend([7; CHECKSUM_LEN]);
        out
    }

    fn paths(tracked: &[Tracked]) -> Vec<&str> {
        tracked.iter().map(|entry| entry.path.as_str()).collect()
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
        assert!(tracked[2].symlink && !tracked[1].symlink);
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
    fn conflicted_and_sparse_entries_are_left_out() {
        let mut conflicted = item("merge.ts", REGULAR);
        conflicted.stage = 2;
        let mut sparse = item("outside.ts", REGULAR);
        sparse.skip_worktree = true;
        let bytes = index(3, &[conflicted, item("ok.ts", REGULAR), sparse], b"");
        assert_eq!(paths(&parse(&bytes).unwrap()), ["ok.ts"]);
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

    fn tracked(paths: &[&str]) -> Vec<Tracked> {
        paths
            .iter()
            .map(|path| Tracked {
                path: (*path).to_owned(),
                symlink: false,
            })
            .collect()
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
            [("a".into(), true), ("a-b".into(), true), ("a.ts".into(), false), ("top.ts".into(), false)]
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
    fn the_overlay_answers_tracked_paths_and_defers_the_rest() {
        let disk = MemoryFs::new()
            .with_file("/ws/src/a.ts", "")
            .with_file("/ws/src/untracked.ts", "")
            .with_file("/ws/node_modules/x/index.js", "");
        let tree = TrackedTree::build(Path::new("/ws"), Path::new(""), &tracked(&["src/a.ts", "gone.ts"]));
        let fs = IndexedFs::new(Arc::new(disk), tree);
        assert!(fs.is_file(Path::new("/ws/src/a.ts")));
        assert!(fs.is_file(Path::new("/ws/gone.ts")));
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

    #[test]
    fn symbolic_links_are_left_to_the_real_file_system() {
        let files = vec![Tracked {
            path: "link.ts".to_owned(),
            symlink: true,
        }];
        let tree = TrackedTree::build(Path::new("/ws"), Path::new(""), &files);
        let listing = tree.listing(Path::new("/ws")).unwrap();
        assert!(listing[0].is_symlink && !listing[0].is_file && !listing[0].is_dir);
        let fs = IndexedFs::new(Arc::new(MemoryFs::new().with_file("/ws/link.ts", "")), tree);
        assert!(fs.is_file(Path::new("/ws/link.ts")));
    }

    /// A tree whose `.git/index` holds raw bytes.
    struct WithIndex {
        files: MemoryFs,
        index: Vec<u8>,
    }

    impl FileSystem for WithIndex {
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
                Ok(self.index.clone())
            } else {
                self.files.read_bytes(path)
            }
        }
    }

    fn repository(items: &[Item]) -> WithIndex {
        WithIndex {
            files: MemoryFs::new()
                .with_file("/repo/.git/HEAD", "ref: refs/heads/main")
                .with_file("/repo/app/package.json", "{}"),
            index: index(2, items, b""),
        }
    }

    #[test]
    fn load_finds_the_git_directory_above_the_root() {
        let fs = repository(&[item("app/package.json", REGULAR), item("app/src/a.ts", REGULAR)]);
        let tree = TrackedTree::load(&fs, Path::new("/repo/app")).unwrap();
        assert_eq!(
            names(tree.listing(Path::new("/repo/app")).unwrap()),
            [("package.json".into(), false), ("src".into(), true)]
        );
    }

    #[test]
    fn load_refuses_a_root_without_tracked_files_or_a_repository() {
        let fs = repository(&[item("other/a.ts", REGULAR)]);
        assert!(TrackedTree::load(&fs, Path::new("/repo/app")).is_none());
        assert!(TrackedTree::load(&MemoryFs::new().with_file("/ws/a.ts", ""), Path::new("/ws")).is_none());
    }

    #[test]
    fn a_git_file_points_at_the_git_directory() {
        let mut fs = repository(&[item("app/a.ts", REGULAR)]);
        fs.files = MemoryFs::new()
            .with_file("/repo/.git", "gitdir: /elsewhere/worktrees/w\n")
            .with_file("/repo/app/package.json", "{}");
        assert!(TrackedTree::load(&fs, Path::new("/repo/app")).is_some());
    }
}
