//! File system abstraction so discovery runs against disk, memory or an embedder's buffers.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// A directory entry returned by [`FileSystem::read_dir`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    /// Full path of the entry (the listed directory joined with the entry name).
    pub path: PathBuf,
    /// Whether the entry is a directory (symlinks are followed).
    pub is_dir: bool,
    /// Whether the entry is a regular file (symlinks are followed).
    pub is_file: bool,
}

/// Read-only file system access used by discovery.
pub trait FileSystem: Send + Sync {
    /// Reads a UTF-8 file.
    ///
    /// # Errors
    /// Any IO error from reading the file.
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    /// Lists a directory, sorted by path.
    ///
    /// # Errors
    /// Any IO error from listing the directory.
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>>;
    /// Whether `path` is a regular file.
    fn is_file(&self, path: &Path) -> bool;
    /// Whether `path` is a directory.
    fn is_dir(&self, path: &Path) -> bool;
}

/// The real file system.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsFs;

impl FileSystem for OsFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            let (is_dir, is_file) = std::fs::metadata(&path)
                .or_else(|_| entry.metadata())
                .map_or((false, false), |metadata| (metadata.is_dir(), metadata.is_file()));
            entries.push(DirEntry { path, is_dir, is_file });
        }
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(entries)
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn is_dir(&self, path: &Path) -> bool {
        path.is_dir()
    }
}

/// An in-memory file tree for tests and embedders. Directories exist implicitly as parents of files.
#[derive(Debug, Clone, Default)]
pub struct MemoryFs {
    files: BTreeMap<PathBuf, String>,
}

impl MemoryFs {
    /// An empty tree.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a file, builder style.
    #[must_use]
    pub fn with_file(mut self, path: impl Into<PathBuf>, content: impl Into<String>) -> Self {
        self.insert(path, content);
        self
    }

    /// Adds or replaces a file.
    pub fn insert(&mut self, path: impl Into<PathBuf>, content: impl Into<String>) {
        self.files.insert(path.into(), content.into());
    }
}

fn not_found(path: &Path) -> io::Error {
    io::Error::new(io::ErrorKind::NotFound, format!("{} not found", path.display()))
}

impl FileSystem for MemoryFs {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        self.files.get(path).cloned().ok_or_else(|| not_found(path))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let mut children: BTreeMap<PathBuf, (bool, bool)> = BTreeMap::new();
        for file in self.files.keys() {
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
            .map(|(path, (is_dir, is_file))| DirEntry { path, is_dir, is_file })
            .collect())
    }

    fn is_file(&self, path: &Path) -> bool {
        self.files.contains_key(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.files.keys().any(|file| {
            file.strip_prefix(path)
                .is_ok_and(|rest| rest.components().next().is_some())
        })
    }
}
