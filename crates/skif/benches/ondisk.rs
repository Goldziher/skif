//! `skif -r .` over a repository on disk: ~10k directories (most of them Python and Go trees), 70 project roots
//! (pnpm workspaces, standalone packages, projects nested below workspaces) and ~5k JavaScript files.
//!
//! The in-memory benches cannot show where a real run spends its time: in directory walks and file system calls.
//! `uncached` reads the disk directly, `cached` shares one [`CachedFs`] across the run as the command line does, and
//! `cached_for_targets` also builds the linters with [`Linter::for_targets`], so they share the module cache of the
//! run as the command line's do. The roots of this repository share little code, so `cached_for_targets` should cost
//! what `cached` does. Run it with `SKIF_TIMINGS=1` to see the phases of a single run on stderr.
//!
//! `shared_closure_on_disk` lints 10 and 40 standalone packages that all import one directory of shared code outside
//! them: `separate` builds a linter per root, each parsing the shared code itself, and `shared` builds them with
//! [`Linter::for_targets`], which parses it once for the run. The root count should not multiply the parse time.

#![allow(clippy::expect_used)]

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
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

/// The modules of the shared directory.
const SHARED_FILES: usize = 400;
/// The functions each shared module exports.
const SHARED_FUNCTIONS: usize = 50;

/// Writes `roots` standalone packages below `root` whose entry files import `shared/index.ts`, which re-exports
/// [`SHARED_FILES`] modules outside every package.
fn write_shared_closure(root: &Path, roots: usize) {
    let shared = root.join("shared");
    std::fs::create_dir_all(&shared).expect("directory is created");
    for file in 0..SHARED_FILES {
        let body = (0..SHARED_FUNCTIONS).fold(String::new(), |mut body, function| {
            let _ = writeln!(
                body,
                "export function f{file}_{function}(x: number): number {{\n  return x + {function};\n}}"
            );
            body
        });
        std::fs::write(shared.join(format!("m{file}.ts")), body).expect("file is written");
    }
    let index = (0..SHARED_FILES).fold(String::new(), |mut index, file| {
        let _ = writeln!(index, "export * from './m{file}';");
        index
    });
    std::fs::write(shared.join("index.ts"), index).expect("file is written");
    for package in 0..roots {
        let dir = root.join(format!("p{package}"));
        std::fs::create_dir_all(&dir).expect("directory is created");
        std::fs::write(
            dir.join("package.json"),
            format!(r#"{{"name": "p{package}", "version": "1.0.0", "main": "index.ts"}}"#),
        )
        .expect("file is written");
        std::fs::write(
            dir.join("index.ts"),
            format!("import {{ f0_0 }} from '../shared/index';\nexport const value{package} = f0_0({package});\n"),
        )
        .expect("file is written");
    }
}

/// [`lint_recursive`] over a [`CachedFs`] with the linters of the run built by [`Linter::for_targets`], sharing the
/// module cache of the run.
fn lint_recursive_shared(root: &Path, overrides: &ConfigOverrides) -> usize {
    let fs: Arc<dyn FileSystem> = Arc::new(CachedFs::new(OsFs));
    let targets = skif::discover_all(root, fs.as_ref(), overrides).expect("generated roots are discoverable");
    let linters = Linter::for_targets(&targets, fs).expect("built-in rules register");
    targets
        .par_iter()
        .zip(&linters)
        .map(|(target, linter)| linter.run(&target.workspace).expect("run succeeds").diagnostics.len())
        .sum()
}

fn bench_shared_closure(c: &mut Criterion) {
    let mut group = c.benchmark_group("shared_closure_on_disk");
    group.sample_size(10);
    for roots in [10, 40] {
        let dir = tempfile::tempdir().expect("temporary directory");
        write_shared_closure(dir.path(), roots);
        let root = std::fs::canonicalize(dir.path()).expect("temporary directory resolves");
        group.bench_with_input(BenchmarkId::new("separate", roots), &root, |b, root| {
            b.iter(|| lint_recursive(root, &ConfigOverrides::default(), true));
        });
        group.bench_with_input(BenchmarkId::new("shared", roots), &root, |b, root| {
            b.iter(|| lint_recursive_shared(root, &ConfigOverrides::default()));
        });
    }
    group.finish();
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
    group.bench_function("cached_for_targets", |b| {
        b.iter(|| lint_recursive_shared(&root, &ConfigOverrides::default()));
    });
    group.bench_function("cached_no_js", |b| {
        b.iter(|| lint_recursive(&root, &no_js, true));
    });
    group.finish();
}

criterion_group!(benches, bench_recursive, bench_shared_closure);
criterion_main!(benches);
