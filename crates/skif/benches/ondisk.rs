//! `skif -r .` over a repository on disk: ~10k directories (most of them Python and Go trees), 70 project roots
//! (pnpm workspaces, standalone packages, projects nested below workspaces) and ~5k JavaScript files.
//!
//! The in-memory benches cannot show where a real run spends its time: in directory walks and file system calls.
//! `uncached` reads the disk directly, `cached` shares one [`CachedFs`] across the run as the command line does.
//! Run it with `SKIF_TIMINGS=1` to see the phases of a single run on stderr.

#![allow(clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use rayon::prelude::*;
use skif::{CachedFs, ConfigOverrides, FileSystem, Linter, OsFs};
use skif_bench::ondisk::{self, OnDisk, OnDiskOptions};

fn write(root: &Path, repo: &OnDisk) {
    for (relative, contents) in &repo.files {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().expect("generated files have a parent")).expect("directory is created");
        std::fs::write(path, contents).expect("file is written");
    }
}

/// What `skif -r <root>` does, without printing: discover the roots, lint each one, count the diagnostics.
fn lint_recursive(root: &Path, overrides: &ConfigOverrides, cached: bool) -> usize {
    let fs: Arc<dyn FileSystem> = if cached {
        Arc::new(CachedFs::new(OsFs))
    } else {
        Arc::new(OsFs)
    };
    let targets = skif::discover_all(root, fs.as_ref(), overrides).expect("generated roots are discoverable");
    targets
        .par_iter()
        .map(|target| {
            let linter = Linter::with_fs(&target.config, Arc::clone(&fs)).expect("built-in rules register");
            linter.run(&target.workspace).expect("run succeeds").diagnostics.len()
        })
        .sum()
}

fn bench_recursive(c: &mut Criterion) {
    if std::env::var_os("SKIF_TIMINGS").is_some_and(|value| !value.is_empty() && value != "0") {
        let _ = skif_core::timing::enable_stderr();
    }
    let repo = ondisk::build(&OnDiskOptions::default());
    let dir = tempfile::tempdir().expect("temporary directory");
    write(dir.path(), &repo);
    let root = std::fs::canonicalize(dir.path()).expect("temporary directory resolves");
    let no_js = ConfigOverrides::default().with_no_js(true);

    let mut group = c.benchmark_group("recursive_on_disk");
    group.sample_size(10);
    group.bench_function("uncached", |b| {
        b.iter(|| lint_recursive(&root, &ConfigOverrides::default(), false));
    });
    group.bench_function("cached", |b| {
        b.iter(|| lint_recursive(&root, &ConfigOverrides::default(), true));
    });
    group.bench_function("cached_no_js", |b| {
        b.iter(|| lint_recursive(&root, &no_js, true));
    });
    group.finish();
}

criterion_group!(benches, bench_recursive);
criterion_main!(benches);
