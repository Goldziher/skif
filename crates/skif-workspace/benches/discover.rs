//! Workspace discovery and the name lookups the rules do per dependency entry.

use std::fmt::Write as _;
use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use skif_config::Config;
use skif_workspace::{MemoryFs, Workspace};

const DEPENDENCIES_PER_PACKAGE: usize = 10;

/// A pnpm-style monorepo of `packages` members; every member depends on the next few, so `^1.2.3` repeats.
fn monorepo(packages: usize) -> MemoryFs {
    let mut fs = MemoryFs::new().with_file("/ws/package.json", r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#);
    for index in 0..packages {
        let mut dependencies = String::new();
        for offset in 1..=DEPENDENCIES_PER_PACKAGE {
            if !dependencies.is_empty() {
                dependencies.push(',');
            }
            let _ = write!(dependencies, r#""@bench/p{}":"^1.2.3""#, (index + offset) % packages);
        }
        let manifest = format!(r#"{{"name":"@bench/p{index}","version":"1.2.3","dependencies":{{{dependencies}}}}}"#);
        fs.insert(format!("/ws/packages/p{index}/package.json"), manifest);
    }
    fs
}

fn bench_discover(c: &mut Criterion) {
    let mut group = c.benchmark_group("Workspace/discover");
    group.sample_size(20);
    for packages in [500, 2000] {
        let fs = monorepo(packages);
        group.bench_with_input(BenchmarkId::from_parameter(packages), &fs, |b, fs| {
            b.iter(|| {
                Workspace::discover_with_fs(Path::new("/ws"), &Config::default(), black_box(fs)).map(|workspace| workspace.packages().len())
            });
        });
    }
    group.finish();
}

fn bench_package_by_name(c: &mut Criterion) {
    let mut group = c.benchmark_group("Workspace/package_by_name");
    for packages in [500, 2000] {
        let fs = monorepo(packages);
        let Ok(workspace) = Workspace::discover_with_fs(Path::new("/ws"), &Config::default(), &fs) else {
            continue;
        };
        group.bench_with_input(BenchmarkId::from_parameter(packages), &workspace, |b, workspace| {
            b.iter(|| {
                workspace
                    .packages()
                    .iter()
                    .flat_map(|package| package.dependencies.iter())
                    .filter(|entry| workspace.package_by_name(black_box(&entry.name)).is_some())
                    .count()
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_discover, bench_package_by_name);
criterion_main!(benches);
