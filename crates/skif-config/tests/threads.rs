//! A sequential [`ConfigLoader`] probes the package files of a workspace on the calling thread and spawns no threads.
//!
//! Every file system call is recorded with the thread that made it and the process's thread count at that moment, as
//! in `skif`'s `tests/threads.rs`. This file holds a single test so no other test thread changes the process's thread
//! count.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use skif_config::{Ceiling, ConfigCache, ConfigLoader, ConfigOverrides};
use skif_core::{DirEntry, FileSystem, MemoryFs};

const MEMBERS: usize = 64;

fn thread_count() -> usize {
    std::fs::read_dir("/proc/self/task").unwrap().count()
}

#[derive(Default)]
struct Seen {
    threads: HashSet<ThreadId>,
    max_thread_count: usize,
    calls: usize,
}

struct Recording {
    inner: MemoryFs,
    seen: Mutex<Seen>,
}

impl Recording {
    fn note(&self) {
        let mut seen = self.seen.lock().unwrap();
        seen.threads.insert(thread::current().id());
        seen.max_thread_count = seen.max_thread_count.max(thread_count());
        seen.calls += 1;
    }
}

impl FileSystem for Recording {
    fn read_to_string(&self, path: &Path) -> io::Result<Arc<str>> {
        self.note();
        self.inner.read_to_string(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.note();
        self.inner.read_dir(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        self.note();
        self.inner.is_file(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        self.note();
        self.inner.is_dir(path)
    }
}

fn members() -> Vec<PathBuf> {
    (0..MEMBERS)
        .map(|i| PathBuf::from(format!("/ws/packages/p{i}")))
        .collect()
}

/// Adds the package files of the fixture with a fresh cache; returns the package files applied, the threads that
/// probed and the highest thread count observed.
fn observe(sequential: bool) -> (Vec<PathBuf>, HashSet<ThreadId>, usize) {
    let mut inner = MemoryFs::new().with_file("/ws/skif.toml", "root = true\n");
    for i in (0..MEMBERS).step_by(8) {
        inner = inner.with_file(
            format!("/ws/packages/p{i}/skif.toml"),
            "[rules]\nempty-dependencies = \"warn\"\n",
        );
    }
    let fs = Recording {
        inner,
        seen: Mutex::new(Seen::default()),
    };
    let cache = ConfigCache::new();
    let loader = ConfigLoader::new(&fs, &cache).with_sequential(sequential);
    let mut resolved = loader
        .load(Path::new("/ws"), &Ceiling::Vcs, &ConfigOverrides::default())
        .unwrap();
    loader
        .add_package_files(&mut resolved, Path::new("/ws"), &members())
        .unwrap();
    let applied = resolved
        .sources
        .iter()
        .skip(1)
        .map(|source| source.path.clone())
        .collect();
    let seen = fs.seen.lock().unwrap();
    assert!(seen.calls > MEMBERS, "the loader must probe every member");
    (applied, seen.threads.clone(), seen.max_thread_count)
}

#[test]
fn a_sequential_loader_probes_package_files_on_the_calling_thread() {
    let caller = thread::current().id();
    let before = thread_count();

    let (sequential, threads, max_count) = observe(true);
    assert_eq!(sequential.len(), MEMBERS / 8);
    assert_eq!(threads, HashSet::from([caller]), "probing ran off the calling thread");
    assert!(
        max_count <= before,
        "{max_count} threads during the run, {before} before"
    );

    // Negative control: the default loader probes on the global rayon pool, off the calling thread, with the same
    // result, so the check above can fail.
    let (parallel, threads, _) = observe(false);
    assert!(
        threads.iter().any(|thread| *thread != caller),
        "the default loader should probe on the rayon pool"
    );
    assert_eq!(parallel, sequential);
}
