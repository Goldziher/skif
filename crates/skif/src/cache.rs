//! Per-run memoisation of file system queries for the entry points that probe many paths.

use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_core::{CachedFs, DirEntry, FileSystem};

/// A file system reached through a pointer (`&dyn FileSystem`, `Arc<dyn FileSystem>`), so a [`CachedFs`] can wrap it.
pub(crate) struct Through<P>(P);

impl<T, P> FileSystem for Through<P>
where
    T: FileSystem + ?Sized,
    P: Deref<Target = T> + Send + Sync,
{
    fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
        self.0.read_to_string(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.0.read_dir(path)
    }

    fn read_dir_shared(&self, path: &Path) -> io::Result<Arc<[DirEntry]>> {
        self.0.read_dir_shared(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.0.is_file(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.0.is_dir(path)
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.0.canonicalize(path)
    }
}

/// A cache over `fs` that lives as long as one call: directory listings, existence checks and small configuration
/// files are answered once.
pub(crate) fn per_call(fs: &dyn FileSystem) -> CachedFs<Through<&dyn FileSystem>> {
    CachedFs::new(Through(fs))
}

/// A cache over a shared `fs`, for a value that outlives a call and clears it at the start of every run.
pub(crate) fn shared(fs: Arc<dyn FileSystem>) -> Arc<CachedFs<Through<Arc<dyn FileSystem>>>> {
    Arc::new(CachedFs::new(Through(fs)))
}
