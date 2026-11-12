//! Per-run memoisation of file system queries for the entry points that probe many paths.

use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use skif_core::{CachedFs, DirEntry, FileSystem, PathKind};

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

    fn kind(&self, path: &Path) -> PathKind {
        self.0.kind(path)
    }

    fn canonicalize(&self, path: &Path) -> io::Result<PathBuf> {
        self.0.canonicalize(path)
    }

    fn read_bytes(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.0.read_bytes(path)
    }

    fn read_prefix(&self, path: &Path, limit: usize) -> io::Result<Vec<u8>> {
        self.0.read_prefix(path, limit)
    }

    fn modified(&self, path: &Path) -> Option<SystemTime> {
        self.0.modified(path)
    }
}

/// A cache over `fs` that lives as long as one call: directory listings, existence checks and small configuration
/// files are answered once.
pub(crate) fn per_call(fs: &dyn FileSystem) -> CachedFs<Through<&dyn FileSystem>> {
    CachedFs::new(Through(fs))
}

/// A cache over a shared `fs`, for a value that outlives a call; the owner clears it when the runs using it end.
pub(crate) fn shared(fs: Arc<dyn FileSystem>) -> Arc<CachedFs<Through<Arc<dyn FileSystem>>>> {
    Arc::new(CachedFs::new(Through(fs)))
}

#[cfg(test)]
mod tests {
    use skif_core::MemoryFs;

    use super::*;

    /// A file system that answers `read_bytes` and `modified` differently from the trait defaults.
    struct Marked;

    impl FileSystem for Marked {
        fn read_to_string(&self, _: &Path) -> io::Result<Arc<str>> {
            Ok(Arc::from("text"))
        }

        fn read_dir(&self, _: &Path) -> io::Result<Vec<DirEntry>> {
            Ok(Vec::new())
        }

        fn is_file(&self, _: &Path) -> bool {
            false
        }

        fn is_dir(&self, _: &Path) -> bool {
            false
        }

        fn read_bytes(&self, _: &Path) -> io::Result<Vec<u8>> {
            Ok(b"bytes".to_vec())
        }

        fn modified(&self, _: &Path) -> Option<SystemTime> {
            Some(SystemTime::UNIX_EPOCH)
        }
    }

    #[test]
    fn bytes_and_modification_time_reach_the_wrapped_file_system() {
        let through = Through(Arc::new(Marked) as Arc<dyn FileSystem>);
        assert_eq!(through.read_bytes(Path::new("/x")).unwrap(), b"bytes");
        assert_eq!(through.modified(Path::new("/x")), Some(SystemTime::UNIX_EPOCH));
        let memory = MemoryFs::new().with_file("/x", "t");
        let plain = Through(&memory);
        assert_eq!(plain.read_bytes(Path::new("/x")).unwrap(), b"t");
    }
}
