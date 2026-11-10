//! File discovery and resolution over a generated tree on disk, as a large pnpm monorepo has it.
//!
//! The tree has 100 packages with 210 directories of one module each (more than 20 000 directories and 21 000
//! modules), a pnpm-style `node_modules` (a virtual store at the root, symbolic links in every package) and a git
//! index. It is generated once per run in the system's temporary directory. The groups compare the directory walk with
//! the git index (`[js] discovery`: `auto` walks, `git` checks each directory against the index time), the
//! listing of every directory, the lookup of a module's extensions in those listings, and the listing of the same
//! directories after one in a hundred changed. Every mode must report the same diagnostics as the walk before it is
//! timed. Without `git` on the `PATH` the index groups
//! are skipped.

#![allow(clippy::expect_used)]

use std::fmt::Write as _;
use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use skif_config::{Config, JsDiscovery};
use skif_core::{CachedFs, FileSystem, OsFs};
use skif_js::{JsProvider, discovery_file_system};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const PACKAGES: usize = 100;
const DIRS_PER_PACKAGE: usize = 210;
const STORE_PACKAGES: usize = 10;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("a file has a parent")).expect("directory is created");
    fs::write(path, text).expect("file is written");
}

fn store_name(index: usize) -> String {
    if index == 0 {
        "lodash".to_owned()
    } else {
        format!("dep{index}")
    }
}

fn generate(root: &Path) {
    let workspaces = "\"packages/*\"";
    write(
        &root.join("package.json"),
        &format!("{{\"name\": \"root\", \"private\": true, \"workspaces\": [{workspaces}]}}"),
    );
    write(&root.join(".gitignore"), "node_modules\n");
    for index in 0..STORE_PACKAGES {
        let name = store_name(index);
        let store = root.join(format!("node_modules/.pnpm/{name}@1.0.0/node_modules/{name}"));
        write(
            &store.join("package.json"),
            &format!("{{\"name\": \"{name}\", \"version\": \"1.0.0\", \"main\": \"index.js\"}}"),
        );
        write(&store.join("index.js"), "module.exports = function () {};\n");
        write(&store.join("debounce.js"), "module.exports = function () {};\n");
    }
    let dependencies: Vec<String> = (0..STORE_PACKAGES)
        .map(|i| format!("\"{}\": \"1.0.0\"", store_name(i)))
        .collect();
    for package in 0..PACKAGES {
        let dir = root.join(format!("packages/p{package}"));
        write(
            &dir.join("package.json"),
            &format!(
                "{{\"name\": \"@bench/p{package}\", \"private\": true, \"main\": \"src/index.ts\", \
                 \"dependencies\": {{{}}}}}",
                dependencies.join(", ")
            ),
        );
        let modules = dir.join("node_modules");
        fs::create_dir_all(&modules).expect("node_modules is created");
        for index in 0..STORE_PACKAGES {
            let name = store_name(index);
            #[cfg(unix)]
            std::os::unix::fs::symlink(
                format!("../../../node_modules/.pnpm/{name}@1.0.0/node_modules/{name}"),
                modules.join(&name),
            )
            .expect("symbolic link is created");
        }
        let mut index_source = String::new();
        for d in 0..DIRS_PER_PACKAGE {
            let _ = writeln!(index_source, "import {{ value{d} }} from \"./d{d}/mod\";");
            write(
                &dir.join(format!("src/d{d}/mod.ts")),
                &format!(
                    "import debounce from \"lodash/debounce\";\nimport dep1 from \"dep1\";\n\n\
                     export const value{d} = debounce(dep1, {d});\nexport const unused{d} = {d};\n"
                ),
            );
        }
        index_source.push_str("export const run = (): number => value0 as number;\n");
        write(&dir.join("src/index.ts"), &index_source);
    }
}

/// Runs git without the user's configuration, so the repository is a plain SHA-1 one whatever the host sets.
fn git(root: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(["-c", "init.defaultObjectFormat=sha1", "-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// The diagnostics of one run, one line each, in a stable order.
fn run(workspace: &Workspace, discovery: JsDiscovery) -> Vec<String> {
    let mut config = Config::default();
    config.js.discovery = discovery;
    let provider = JsProvider::new(Arc::new(CachedFs::new(OsFs)));
    let registry = Registry::new(vec![Box::new(provider)]).expect("the JS provider registers");
    let mut lines: Vec<String> = run_unsorted(&registry, workspace, &config, &RunOptions::default())
        .expect("the JS run succeeds")
        .iter()
        .map(|diagnostic| {
            format!(
                "{} {} {}",
                diagnostic.rule.as_str(),
                diagnostic.location.path.display(),
                diagnostic.message
            )
        })
        .collect();
    lines.sort();
    lines
}

/// The file system the analysis of `workspace` lists through under `discovery`, over a fresh cache.
fn overlay(workspace: &Workspace, discovery: JsDiscovery) -> Arc<dyn FileSystem> {
    let mut config = Config::default();
    config.js.discovery = discovery;
    let fs: Arc<dyn FileSystem> = Arc::new(CachedFs::new(OsFs));
    discovery_file_system(&fs, workspace, &config)
}

fn bench_discovery(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("temporary directory is created");
    let root = dir.path();
    generate(root);
    let indexed = git(root, &["init", "-q"]) && git(root, &["add", "-A"]);
    let workspace =
        Workspace::discover_with_fs(root, &Config::default(), &OsFs).expect("the generated workspace discovers");

    let walked = run(&workspace, JsDiscovery::Walk);
    let mut modes = vec![("walk", JsDiscovery::Walk)];
    if indexed {
        modes.push(("auto", JsDiscovery::Auto));
        modes.push(("git-index", JsDiscovery::Git));
    }
    for (name, discovery) in &modes {
        assert_eq!(
            &run(&workspace, *discovery),
            &walked,
            "the {name} listing must give the diagnostics of the walk"
        );
    }

    let mut group = c.benchmark_group("js_disk_tree");
    group.sample_size(10);
    for (name, discovery) in &modes {
        group.bench_function(BenchmarkId::new("analysis", name), |b| {
            b.iter(|| black_box(run(&workspace, *discovery).len()));
        });
    }
    group.finish();

    let directories: Vec<_> = (0..PACKAGES)
        .flat_map(|package| (0..DIRS_PER_PACKAGE).map(move |d| format!("packages/p{package}/src/d{d}")))
        .map(|relative| root.join(relative))
        .collect();

    let mut group = c.benchmark_group("js_listing");
    group.sample_size(10);
    for (name, discovery) in &modes {
        group.bench_function(BenchmarkId::new("every_directory", name), |b| {
            b.iter_batched(
                || overlay(&workspace, *discovery),
                |fs| {
                    directories
                        .iter()
                        .map(|directory| fs.read_dir_shared(directory).map_or(0, |entries| entries.len()))
                        .sum::<usize>()
                },
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();

    // What the index overlay changes for resolution is where a directory is listed from, so the probe workload lists
    // each directory and looks the extensions of a module up in the listing, as discovery does. The probes themselves
    // are `stat`s answered by the disk in every mode and would measure the same work whatever the mode.
    let mut group = c.benchmark_group("js_probe");
    group.sample_size(10);
    for (name, discovery) in &modes {
        group.bench_function(BenchmarkId::new("extensions_per_directory", name), |b| {
            b.iter_batched(
                || overlay(&workspace, *discovery),
                |fs| count_modules(fs.as_ref(), &directories),
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();

    // One directory in a hundred changes after the index was written: `git` lists those from disk, the others from
    // the index, and the walk (`walk`, `auto`) lists everything from disk.
    for directory in directories.iter().step_by(100) {
        write(&directory.join("fresh.ts"), "export const fresh = 1;\n");
    }
    let touched = modes
        .iter()
        .map(|(name, discovery)| {
            (
                *name,
                listed_entries(overlay(&workspace, *discovery).as_ref(), &directories),
            )
        })
        .collect::<Vec<_>>();
    let walked_entries = touched[0].1;
    for (name, entries) in &touched {
        assert_eq!(
            *entries, walked_entries,
            "the {name} listing must see the files added after the index"
        );
    }
    let mut group = c.benchmark_group("js_listing_touched");
    group.sample_size(10);
    for (name, discovery) in &modes {
        group.bench_function(BenchmarkId::new("one_percent_changed", name), |b| {
            b.iter_batched(
                || overlay(&workspace, *discovery),
                |fs| listed_entries(fs.as_ref(), &directories),
                BatchSize::PerIteration,
            );
        });
    }
    group.finish();
}

/// The number of directories with a module among `directories`, found through their listings.
fn count_modules(fs: &dyn FileSystem, directories: &[PathBuf]) -> usize {
    directories
        .iter()
        .filter(|directory| {
            fs.read_dir_shared(directory).is_ok_and(|entries| {
                ["mod.tsx", "mod.d.ts", "mod.js", "mod.mjs", "mod.ts"]
                    .iter()
                    .any(|name| entries.iter().any(|entry| entry.path.ends_with(name)))
            })
        })
        .count()
}

/// The number of entries in the listings of `directories`.
fn listed_entries(fs: &dyn FileSystem, directories: &[PathBuf]) -> usize {
    directories
        .iter()
        .map(|directory| fs.read_dir_shared(directory).map_or(0, |entries| entries.len()))
        .sum()
}

criterion_group!(benches, bench_discovery);
criterion_main!(benches);
