//! Read-only file system abstraction so discovery runs against disk, memory or an embedder's buffers.

use std::collections::BTreeMap;
use std::io;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A directory entry returned by [`FileSystem::read_dir`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DirEntry {
    /// Full path of the entry (the listed directory joined with the entry name).
    pub path: PathBuf,
    /// Whether the entry is a directory (symlinks are followed).
    pub is_dir: bool,
    /// Whether the entry is a regular file (symlinks are followed).
    pub is_file: bool,
    /// Whether the entry itself is a symbolic link; `is_dir` and `is_file` describe its target.
    pub is_symlink: bool,
}

impl DirEntry {
    /// An entry that is not a symbolic link.
    #[must_use]
    pub const fn new(path: PathBuf, is_dir: bool, is_file: bool) -> Self {
        Self {
            path,
            is_dir,
            is_file,
            is_symlink: false,
        }
    }

    /// Marks the entry as a symbolic link.
    #[must_use]
    pub const fn symlink(mut self) -> Self {
        self.is_symlink = true;
        self
    }
}

/// Read-only file system access used by discovery.
pub trait FileSystem: Send + Sync {
    /// Reads a UTF-8 file.
    ///
    /// # Errors
    /// Any IO error from reading the file.
    fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>>;
    /// Lists a directory, sorted by path.
    ///
    /// # Errors
    /// Any IO error from listing the directory.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>>;
    /// Lists a directory like [`FileSystem::read_dir`], as a shared slice.
    ///
    /// Caching implementations hand out the listing they hold instead of copying it.
    ///
    /// # Errors
    /// Any IO error from listing the directory.
    fn read_dir_shared(&self, path: &Path) -> io::Result<Arc<[DirEntry]>> {
        self.read_dir(path).map(Arc::from)
    }
    /// Whether `path` is a regular file.
    fn is_file(&self, path: &Path) -> bool;
    /// Whether `path` is a directory.
    fn is_dir(&self, path: &Path) -> bool;
    /// `path` as the file system resolves it: absolute, with every symbolic link followed.
    ///
    /// The default returns `path` unchanged, which is right for a tree that has no links (such as [`MemoryFs`]).
    ///
    /// # Errors
    /// Any IO error from resolving the path, such as it not existing.
    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        Ok(path.to_path_buf())
    }
}

/// The real file system.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsFs;

impl FileSystem for OsFs {
    fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
        std::fs::read_to_string(path).map(Arc::from)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            // `file_type` comes from the directory record; only a symlink needs a `stat` to learn its target.
            let file_type = entry.file_type()?;
            let is_symlink = file_type.is_symlink();
            let (is_dir, is_file) = if is_symlink {
                std::fs::metadata(&path).map_or((false, false), |metadata| (metadata.is_dir(), metadata.is_file()))
            } else {
                (file_type.is_dir(), file_type.is_file())
            };
            let entry = DirEntry::new(path, is_dir, is_file);
            entries.push(if is_symlink { entry.symlink() } else { entry });
        }
        // Entries share one parent, so comparing the raw paths orders them like the component-wise `Path` order
        // without parsing components on every comparison.
        entries.sort_unstable_by(|a, b| a.path.as_os_str().cmp(b.path.as_os_str()));
        Ok(entries)
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        std::fs::canonicalize(path)
    }
}

/// An in-memory file tree for tests and embedders. Directories exist implicitly as parents of files.
///
/// Paths order component-wise, so everything below a directory is one contiguous range of the map; listing and
/// directory checks are range queries rather than scans of the whole tree.
#[derive(Debug, Clone, Default)]
pub struct MemoryFs {
    files: BTreeMap<PathBuf, Arc<str>>,
}

impl MemoryFs {
    /// An empty tree.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file, builder style.
    #[must_use]
    pub fn with_file(mut self, path: impl Into<PathBuf>, content: impl AsRef<str>) -> Self {
        self.insert(path, content);
        self
    }

    /// Adds or replaces a file.
    pub fn insert(&mut self, path: impl Into<PathBuf>, content: impl AsRef<str>) {
        self.files.insert(path.into(), Arc::from(content.as_ref()));
    }

    /// The files strictly below `dir`.
    fn below<'a>(&'a self, dir: &'a Path) -> impl Iterator<Item = &'a PathBuf> + 'a {
        self.files
            .range::<Path, _>((Bound::Included(dir), Bound::Unbounded))
            .map(|(path, _)| path)
            .take_while(move |path| path.starts_with(dir))
            .filter(move |path| path.as_path() != dir)
    }
}

fn not_found(path: &Path) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("{} not found", path.display()))
}

impl FileSystem for MemoryFs {
    fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
        self.files.get(path).cloned().ok_or_else(|| not_found(path))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let mut children: BTreeMap<PathBuf, (bool, bool)> = BTreeMap::new();
        for file in self.below(path) {
            let Ok(rest) = file.strip_prefix(path) else { continue };
            let mut components = rest.components();
            let Some(first) = components.next() else { continue };
            let is_file = components.next().is_none();
            let flags = children.entry(path.join(first.as_os_str())).or_default();
            if is_file {
                flags.1 = true;
            } else {
                flags.0 = true;
            }
        }
        if children.is_empty() {
            return Err(not_found(path));
        }
        Ok(children
            .into_iter()
            .map(|(path, (is_dir, is_file))| DirEntry::new(path, is_dir, is_file))
            .collect())
    }

    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.below(path).next().is_some()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn tree() -> MemoryFs {
        MemoryFs::new()
            .with_file("/ws/a/package.json", "{}")
            .with_file("/ws/a/src/index.js", "")
            .with_file("/ws/a-b/package.json", "{}")
            .with_file("/ws/ab", "file")
            .with_file("/ws/z/package.json", "{}")
    }

    fn names(entries: &[DirEntry]) -> Vec<(String, bool, bool)> {
        entries
            .iter()
            .map(|e| {
                (
                    e.path.file_name().unwrap().to_string_lossy().into_owned(),
                    e.is_dir,
                    e.is_file,
                )
            })
            .collect()
    }

    #[test]
    fn memory_listing_is_a_sorted_range_that_ignores_prefix_siblings() {
        let fs = tree();
        assert_eq!(
            names(&fs.read_dir(Path::new("/ws")).unwrap()),
            [
                ("a".into(), true, false),
                ("a-b".into(), true, false),
                ("ab".into(), false, true),
                ("z".into(), true, false)
            ]
        );
        assert_eq!(
            names(&fs.read_dir(Path::new("/ws/a")).unwrap()),
            [("package.json".into(), false, true), ("src".into(), true, false)]
        );
    }

    #[test]
    fn memory_missing_and_file_paths_are_not_directories() {
        let fs = tree();
        assert!(fs.read_dir(Path::new("/ws/missing")).is_err());
        assert!(fs.read_dir(Path::new("/ws/ab")).is_err());
        assert!(fs.is_dir(Path::new("/ws/a")) && fs.is_dir(Path::new("/ws/a/src")) && fs.is_dir(Path::new("/ws")));
        assert!(!fs.is_dir(Path::new("/ws/ab")) && !fs.is_dir(Path::new("/ws/a/sr")) && !fs.is_dir(Path::new("/x")));
        assert!(fs.is_file(Path::new("/ws/ab")) && !fs.is_file(Path::new("/ws/a")));
    }

    #[test]
    fn memory_reads_share_one_allocation() {
        let fs = tree();
        let first = fs.read_to_string(Path::new("/ws/ab")).unwrap();
        let second = fs.read_to_string(Path::new("/ws/ab")).unwrap();
        assert_eq!(&*first, "file");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(
            fs.read_to_string(Path::new("/nope")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn os_listing_reports_kinds_sorted() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("b.txt"), "hello").unwrap();
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        let entries = OsFs.read_dir(dir.path()).unwrap();
        assert_eq!(
            names(&entries),
            [
                ("a.txt".into(), false, true),
                ("b.txt".into(), false, true),
                ("sub".into(), true, false)
            ]
        );
        assert_eq!(&*OsFs.read_to_string(&dir.path().join("b.txt")).unwrap(), "hello");
        assert!(OsFs.is_dir(dir.path()) && OsFs.is_file(&dir.path().join("a.txt")));
    }

    #[cfg(unix)]
    #[test]
    fn os_listing_marks_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
        let flags: Vec<(String, bool)> = OsFs
            .read_dir(dir.path())
            .unwrap()
            .iter()
            .map(|e| (e.path.file_name().unwrap().to_string_lossy().into_owned(), e.is_symlink))
            .collect();
        assert_eq!(flags, [("link".to_owned(), true), ("real".to_owned(), false)]);
    }

    #[test]
    fn memory_entries_are_not_symlinks() {
        let entries = tree().read_dir(Path::new("/ws")).unwrap();
        assert!(entries.iter().all(|entry| !entry.is_symlink));
    }

    #[cfg(unix)]
    #[test]
    fn os_listing_follows_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("real")).unwrap();
        std::fs::write(dir.path().join("file.txt"), "").unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("dir-link")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("file.txt"), dir.path().join("file-link")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("gone"), dir.path().join("dangling")).unwrap();
        let entries = OsFs.read_dir(dir.path()).unwrap();
        assert_eq!(
            names(&entries),
            [
                ("dangling".into(), false, false),
                ("dir-link".into(), true, false),
                ("file-link".into(), false, true),
                ("file.txt".into(), false, true),
                ("real".into(), true, false)
            ]
        );
    }
}
