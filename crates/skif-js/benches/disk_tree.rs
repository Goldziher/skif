//! File discovery and resolution over a generated tree on disk, as a large pnpm monorepo has it.
//!
//! The tree has 100 packages with 210 directories of one module each (more than 20 000 directories and 21 000
//! modules), a pnpm-style `node_modules` (a virtual store at the root, symbolic links in every package) and a git
//! index. It is generated once per run in the system's temporary directory. The groups compare the directory walk with
//! the git index (`[js] discovery`), and the probing of file names that resolution does. Without `git` on the `PATH`
//! the index groups are skipped.

#![allow(clippy::expect_used)]

use std::fmt::Write as _;
use std::fs;
use std::hint::black_box;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main};
use skif_config::{Config, JsDiscovery};
use skif_core::{CachedFs, FileSystem, OsFs};
use skif_js::JsProvider;
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

fn git(root: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args(args)
        .current_dir(root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn run(workspace: &Workspace, discovery: JsDiscovery) -> usize {
    let mut config = Config::default();
    config.js.discovery = discovery;
    let provider = JsProvider::new(Arc::new(CachedFs::new(OsFs)));
    let registry = Registry::new(vec![Box::new(provider)]).expect("the JS provider registers");
    run_unsorted(&registry, workspace, &config, &RunOptions::default())
        .expect("the JS run succeeds")
        .len()
}

fn bench_discovery(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("temporary directory is created");
    let root = dir.path();
    generate(root);
    let indexed = git(root, &["init", "-q"]) && git(root, &["add", "-A"]);
    let workspace =
        Workspace::discover_with_fs(root, &Config::default(), &OsFs).expect("the generated workspace discovers");

    let mut group = c.benchmark_group("js_disk_tree");
    group.sample_size(10);
    group.bench_function(BenchmarkId::new("analysis", "walk"), |b| {
        b.iter(|| black_box(run(&workspace, JsDiscovery::Walk)));
    });
    if indexed {
        group.bench_function(BenchmarkId::new("analysis", "git-index"), |b| {
            b.iter(|| black_box(run(&workspace, JsDiscovery::Git)));
        });
    }
    group.finish();

    let directories: Vec<_> = (0..PACKAGES)
        .flat_map(|package| (0..DIRS_PER_PACKAGE).map(move |d| format!("packages/p{package}/src/d{d}")))
        .map(|relative| root.join(relative))
        .collect();
    let mut group = c.benchmark_group("js_probe");
    group.sample_size(10);
    group.bench_function("extensions_per_directory", |b| {
        b.iter_batched(
            || CachedFs::new(OsFs),
            |fs| {
                directories
                    .iter()
                    .filter(|directory| {
                        ["mod.tsx", "mod.d.ts", "mod.js", "mod.mjs", "mod.ts"]
                            .iter()
                            .any(|name| fs.is_file(&directory.join(name)))
                    })
                    .count()
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

criterion_group!(benches, bench_discovery);
criterion_main!(benches);
