//! End-to-end benchmarks over generated monorepos held in a [`MemoryFs`], so disk speed never enters the numbers.
//!
//! The workspaces come from the `skif-bench` generator (seed 1, pnpm layout, 5 % issue rate). The
//! `target/manifest_policy_500` benchmark is the ADR 0001 budget: manifest and policy rules over 500 packages in under
//! 50 ms.

#![allow(clippy::expect_used)]

use std::fmt::Write as _;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use skif::{Config, ConfigOverrides, Linter, MemoryFs, Workspace, load_config};
use skif_bench::generate::{self, Options, PackageManager};
use skif_workspace::Manifest;

const ROOT: &str = "/repo";

const PLAIN: &str = "";

const SEMVER_GROUPS: &str = r#"
[[semver-groups]]
label = "types-caret"
dependencies = ["@types/*"]
range = "^"

[[semver-groups]]
label = "everything-else-caret"
range = "^"
"#;

const SAME_RANGE: &str = r#"
[[version-groups]]
label = "same-range"
policy = "same-range"
"#;

fn generate_files(packages: usize) -> Vec<(PathBuf, String)> {
    generate::build(&Options {
        packages,
        seed: 1,
        package_manager: PackageManager::Pnpm,
        catalog: false,
        mismatch_rate: 0.05,
    })
    .expect("generator options are valid")
    .files
}

fn memory_fs(packages: usize) -> MemoryFs {
    let mut fs = MemoryFs::new();
    for (path, contents) in generate_files(packages) {
        fs.insert(Path::new(ROOT).join(path), contents);
    }
    fs
}

fn config(toml: &str) -> Config {
    Config::from_toml_str(toml).expect("benchmark configuration is valid")
}

fn discover(fs: &MemoryFs, config: &Config) -> Workspace {
    Workspace::discover_with_fs(Path::new(ROOT), config, fs).expect("generated workspace is discoverable")
}

fn policy_linter(config: &Config) -> Linter {
    Linter::with_providers(config, vec![Box::new(skif_rules::policy_provider())]).expect("policy provider registers")
}

fn ignore_entries(count: usize) -> String {
    let mut toml = String::new();
    for i in 0..count {
        let _ = write!(
            toml,
            "\n[[ignore]]\nrules = [\"version-mismatch\"]\npackages = [\"@bench/absent-{i}\"]\ndependencies = [\"absent-dependency-{i}\"]\n"
        );
    }
    toml
}

fn bench_manifest_parse(c: &mut Criterion) {
    let manifests: Vec<(PathBuf, Arc<str>)> = generate_files(500)
        .into_iter()
        .filter(|(path, _)| path.starts_with("apps") || path.starts_with("packages"))
        .map(|(path, contents)| (path, Arc::from(contents)))
        .collect();
    let bytes: usize = manifests.iter().map(|(_, source)| source.len()).sum();
    let mut group = c.benchmark_group("manifest");
    group.throughput(Throughput::Bytes(bytes as u64));
    group.bench_function("parse_500", |b| {
        b.iter(|| {
            for (path, source) in &manifests {
                black_box(Manifest::parse(path.clone(), Arc::clone(source)).expect("generated manifest parses"));
            }
        });
    });
    group.finish();
}

fn bench_discover(c: &mut Criterion) {
    let config = config(PLAIN);
    let mut group = c.benchmark_group("discover");
    for packages in [50, 500, 2000] {
        let fs = memory_fs(packages);
        group.throughput(Throughput::Elements(packages as u64));
        if packages >= 2000 {
            group.sample_size(20);
        }
        group.bench_with_input(BenchmarkId::from_parameter(packages), &fs, |b, fs| {
            b.iter(|| discover(black_box(fs), &config));
        });
    }
    group.finish();
}

fn bench_policy(c: &mut Criterion) {
    let variants = [
        ("plain", PLAIN.to_owned()),
        ("semver_groups", SEMVER_GROUPS.to_owned()),
        ("same_range", SAME_RANGE.to_owned()),
    ];
    let mut group = c.benchmark_group("policy");
    for packages in [500, 2000] {
        let fs = memory_fs(packages);
        group.throughput(Throughput::Elements(packages as u64));
        if packages >= 2000 {
            group.sample_size(20);
        }
        for (label, toml) in &variants {
            let config = config(toml);
            let workspace = discover(&fs, &config);
            let linter = policy_linter(&config);
            group.bench_function(BenchmarkId::new(*label, packages), |b| {
                b.iter(|| linter.run(black_box(&workspace)).expect("policy run succeeds"));
            });
        }
    }
    group.finish();
}

/// Manifest, policy and catalog providers over the whole workspace; `manifest_policy_500` is the ADR 0001 target.
fn bench_target(c: &mut Criterion) {
    let config = config(PLAIN);
    let linter = Linter::new(&config).expect("built-in rules register");
    let mut group = c.benchmark_group("target");
    for packages in [50, 500, 2000] {
        let workspace = discover(&memory_fs(packages), &config);
        group.throughput(Throughput::Elements(packages as u64));
        if packages >= 2000 {
            group.sample_size(20);
        }
        group.bench_function(format!("manifest_policy_{packages}"), |b| {
            b.iter(|| linter.run(black_box(&workspace)).expect("lint run succeeds"));
        });
    }
    group.finish();
}

/// Cost of `[[ignore]]` entries: none of them match, so each diagnostic is checked against every entry.
fn bench_suppressions(c: &mut Criterion) {
    let fs = memory_fs(500);
    let mut group = c.benchmark_group("suppressions");
    group.throughput(Throughput::Elements(500));
    for entries in [0, 10, 50] {
        let config = config(&ignore_entries(entries));
        let workspace = discover(&fs, &config);
        let linter = Linter::new(&config).expect("built-in rules register");
        group.bench_function(BenchmarkId::new("ignore_entries", entries), |b| {
            b.iter(|| linter.run(black_box(&workspace)).expect("lint run succeeds"));
        });
    }
    group.finish();
}

fn override_entries(count: usize) -> String {
    let mut toml = String::new();
    for i in 0..count {
        let _ = write!(
            toml,
            "\n[[overrides]]\npackages = [\"@bench/absent-{i}\"]\n[overrides.rules]\nversion-mismatch = \"warn\"\n"
        );
    }
    toml
}

/// Cost of package-scoped `[[overrides]]` (what package files become): none of them match, so resolving which
/// overrides cover a diagnostic must not grow with their number per diagnostic. Loaded through the configuration
/// loader, because only a resolved configuration carries the scopes the overrides apply by.
fn bench_overrides(c: &mut Criterion) {
    let fs = memory_fs(500);
    let mut group = c.benchmark_group("overrides");
    group.throughput(Throughput::Elements(500));
    for entries in [0, 100, 1000] {
        let with_config = fs
            .clone()
            .with_file(format!("{ROOT}/skif.toml"), override_entries(entries));
        let config = load_config(Path::new(ROOT), &with_config, &ConfigOverrides::default())
            .expect("benchmark configuration is valid");
        let workspace = discover(&with_config, &config);
        let linter = Linter::new(&config).expect("built-in rules register");
        if entries >= 1000 {
            group.sample_size(20);
        }
        group.bench_function(BenchmarkId::new("package_overrides", entries), |b| {
            b.iter(|| linter.run(black_box(&workspace)).expect("lint run succeeds"));
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_manifest_parse,
    bench_discover,
    bench_policy,
    bench_target,
    bench_suppressions,
    bench_overrides
);
criterion_main!(benches);
