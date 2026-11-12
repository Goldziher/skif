//! A [`FileSystem`] that reads an editor's unsaved buffers over another file system (ADR 0029).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use crate::fs::{DirEntry, FileSystem, PathKind};

/// `base` with the text of some files replaced by buffers, as an editor holds them before they are saved.
///
/// [`FileSystem::read_to_string`] and [`FileSystem::read_bytes`] return the buffer of a path that has one and that
/// `base` reports as a file. Every other question, including listings, kinds, canonical paths and modification times,
/// goes to `base`: a buffer whose file does not exist yet is ignored until it is saved, so a listing never disagrees
/// with what can be read. The buffers are fixed at construction, so a run sees one snapshot of them; a host builds a
/// new overlay for each run. Paths are compared as given, so the host keys buffers by the paths the run reads (the
/// canonical paths of a canonical root).
#[derive(Debug, Clone, Default)]
pub struct BufferFs<F> {
    base: F,
    buffers: BTreeMap<PathBuf, Arc<str>>,
}

impl<F: FileSystem> BufferFs<F> {
    /// `base` overlaid with `buffers`, keyed by path.
    #[must_use]
    pub const fn new(base: F, buffers: BTreeMap<PathBuf, Arc<str>>) -> Self {
        Self { base, buffers }
    }

    /// Adds or replaces the buffer of `path`, builder style.
    #[must_use]
    pub fn with_buffer(mut self, path: impl Into<PathBuf>, text: impl Into<Arc<str>>) -> Self {
        self.buffers.insert(path.into(), text.into());
        self
    }

    /// The file system below the buffers.
    #[must_use]
    pub const fn base(&self) -> &F {
        &self.base
    }

    /// The buffer of `path`, when it has one and `base` holds a file there.
    fn buffer(&self, path: &Path) -> Option<&Arc<str>> {
        self.buffers.get(path).filter(|_| self.base.kind(path).is_file)
    }
}

impl<F: FileSystem> FileSystem for BufferFs<F> {
    fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
        match self.buffer(path) {
            Some(text) => Ok(Arc::clone(text)),
            None => self.base.read_to_string(path),
        }
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.base.read_dir(path)
    }

    fn read_dir_shared(&self, path: &Path) -> io::Result<Arc<[DirEntry]>> {
        self.base.read_dir_shared(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.base.is_file(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.base.is_dir(path)
    }

    fn kind(&self, path: &Path) -> PathKind {
        self.base.kind(path)
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.base.canonicalize(path)
    }

    fn read_bytes(&self, path: &Path) -> io::Result<Vec<u8>> {
        match self.buffer(path) {
            Some(text) => Ok(text.as_bytes().to_vec()),
            None => self.base.read_bytes(path),
        }
    }

    fn read_prefix(&self, path: &Path, limit: usize) -> io::Result<Vec<u8>> {
        match self.buffer(path) {
            Some(text) => Ok(text.as_bytes()[..text.len().min(limit)].to_vec()),
            None => self.base.read_prefix(path, limit),
        }
    }

    fn modified(&self, path: &Path) -> Option<SystemTime> {
        self.base.modified(path)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::fs::MemoryFs;

    fn overlay() -> BufferFs<MemoryFs> {
        let base = MemoryFs::new()
            .with_file("/ws/package.json", "{}")
            .with_file("/ws/src/a.ts", "saved");
        BufferFs::new(base, BTreeMap::new())
            .with_buffer("/ws/src/a.ts", "unsaved")
            .with_buffer("/ws/src/new.ts", "never saved")
    }

    #[test]
    fn a_buffer_replaces_the_text_of_an_existing_file() {
        let fs = overlay();
        assert_eq!(&*fs.read_to_string(Path::new("/ws/src/a.ts")).unwrap(), "unsaved");
        assert_eq!(fs.read_bytes(Path::new("/ws/src/a.ts")).unwrap(), b"unsaved");
        assert_eq!(&*fs.read_to_string(Path::new("/ws/package.json")).unwrap(), "{}");
    }

    #[test]
    fn a_buffer_without_a_file_is_ignored() {
        let fs = overlay();
        let error = fs.read_to_string(Path::new("/ws/src/new.ts")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(!fs.is_file(Path::new("/ws/src/new.ts")));
    }

    #[test]
    fn listings_and_kinds_come_from_the_base() {
        let fs = overlay();
        let listed: Vec<PathBuf> = fs
            .read_dir(Path::new("/ws/src"))
            .unwrap()
            .into_iter()
            .map(|entry| entry.path)
            .collect();
        assert_eq!(listed, [PathBuf::from("/ws/src/a.ts")]);
        assert_eq!(fs.kind(Path::new("/ws/src")), PathKind::DIR);
        assert_eq!(fs.kind(Path::new("/ws/src/a.ts")), PathKind::FILE);
    }
}
