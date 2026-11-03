//! `RunOptions::threads` covers workspace discovery and JS graph building, and `1` spawns no threads at all.
//!
//! Every file system call is recorded with the thread that made it and the process's thread count at that moment.
//! Discovery loads members and the graph parses files from inside rayon work, so a call from any thread other than
//! the caller means rayon was used, and a higher thread count than before the run means a thread was spawned. The
//! counts are sampled during the run because the pool's threads are gone by the time it returns. This file holds a
//! single test so no other test thread changes the process's thread count.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;
use std::io;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};

use skif::{ConfigOverrides, DirEntry, FileSystem, Linter, MemoryFs, RunOptions, load_config};

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

    fn reset(&self) {
        *self.seen.lock().unwrap() = Seen::default();
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

fn fixture() -> MemoryFs {
    let mut fs = MemoryFs::new().with_file(
        "/ws/package.json",
        r#"{"name": "root", "private": true, "packageManager": "pnpm@10.0.0", "workspaces": ["packages/*"]}"#,
    );
    for name in ["a", "b", "c", "d"] {
        fs = fs
            .with_file(
                format!("/ws/packages/{name}/package.json"),
                format!(r#"{{"name": "{name}", "version": "1.0.0", "private": true, "main": "index.js"}}"#),
            )
            .with_file(
                format!("/ws/packages/{name}/index.js"),
                "import './one.js';\nimport './two.js';\nimport './three.js';\n",
            )
            .with_file(format!("/ws/packages/{name}/one.js"), "export const one = 1;\n")
            .with_file(format!("/ws/packages/{name}/two.js"), "export const two = 2;\n")
            .with_file(format!("/ws/packages/{name}/three.js"), "export const three = 3;\n");
    }
    fs
}

/// Discovers and lints the fixture under `threads`; returns the thread ids and the highest thread count observed.
fn observe(fs: &Arc<Recording>, threads: NonZeroUsize) -> (usize, HashSet<ThreadId>, usize) {
    let config = load_config(Path::new("/ws"), fs.as_ref(), &ConfigOverrides::default()).unwrap();
    let linter = Linter::with_fs(&config, Arc::clone(fs) as Arc<dyn FileSystem>).unwrap();
    let options = RunOptions::default().with_threads(threads);
    fs.reset();
    let workspace = linter
        .discover_with_fs(Path::new("/ws"), fs.as_ref(), &options)
        .unwrap();
    assert!(workspace.all_packages().count() >= 4);
    let report = linter.run_with(&workspace, &options).unwrap();
    let seen = fs.seen.lock().unwrap();
    (report.diagnostics.len(), seen.threads.clone(), seen.max_thread_count)
}

#[test]
fn one_thread_runs_discovery_and_graph_building_on_the_calling_thread() {
    let fs = Arc::new(Recording {
        inner: fixture(),
        seen: Mutex::new(Seen::default()),
    });
    let caller = thread::current().id();
    let before = thread_count();

    let (sequential_diagnostics, threads, max_count) = observe(&fs, NonZeroUsize::MIN);
    assert!(fs.seen.lock().unwrap().calls > 20, "the run must reach the file system");
    assert_eq!(threads, HashSet::from([caller]), "work ran off the calling thread");
    assert!(
        max_count <= before,
        "{max_count} threads during the run, {before} before"
    );

    // Negative control: a pool of two moves the same work off the calling thread, so the check above can fail.
    let (pooled_diagnostics, threads, _) = observe(&fs, NonZeroUsize::new(2).unwrap());
    assert!(
        !threads.contains(&caller),
        "a 2-thread pool should run the work on its own threads"
    );
    assert_eq!(pooled_diagnostics, sequential_diagnostics);
}
