//! Parsing `pnpm-lock.yaml` and locating its entries, as the lockfile rules do once per finding.

use std::fmt::Write as _;
use std::hint::black_box;
use std::path::PathBuf;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use skif_workspace::{PNPM_LOCKFILE, PnpmLockfile};

/// A lockfile of one importer depending on `snapshots` packages, each depending on the next.
fn lockfile(snapshots: usize) -> Arc<str> {
    let mut text = String::from("lockfileVersion: '9.0'\n\nimporters:\n\n  .:\n    dependencies:\n");
    for index in 0..snapshots {
        let _ = write!(
            text,
            "      p{index}:\n        specifier: ^1.0.0\n        version: 1.0.{index}\n"
        );
    }
    text.push_str("\npackages:\n\n");
    for index in 0..snapshots {
        let _ = write!(
            text,
            "  p{index}@1.0.{index}:\n    resolution: {{integrity: sha512-p{index}}}\n\n"
        );
    }
    text.push_str("snapshots:\n\n");
    for index in 0..snapshots {
        let next = (index + 1) % snapshots;
        let _ = write!(
            text,
            "  p{index}@1.0.{index}:\n    dependencies:\n      p{next}: 1.0.{next}\n\n"
        );
    }
    Arc::from(text)
}

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("PnpmLockfile/parse");
    group.sample_size(20);
    for snapshots in [1000, 5000] {
        let text = lockfile(snapshots);
        group.bench_with_input(BenchmarkId::from_parameter(snapshots), &text, |b, text| {
            b.iter(|| PnpmLockfile::parse(PathBuf::from(PNPM_LOCKFILE), Arc::clone(black_box(text))).is_ok());
        });
    }
    group.finish();
}

fn bench_snapshot_spans(c: &mut Criterion) {
    let mut group = c.benchmark_group("PnpmLockfile/snapshot_span");
    group.sample_size(10);
    for snapshots in [1000, 5000] {
        let text = lockfile(snapshots);
        group.bench_with_input(BenchmarkId::from_parameter(snapshots), &text, |b, text| {
            b.iter(|| {
                let Ok(lock) = PnpmLockfile::parse(PathBuf::from(PNPM_LOCKFILE), Arc::clone(text)) else {
                    return 0;
                };
                lock.snapshots
                    .keys()
                    .filter(|key| lock.snapshot_span(black_box(key)).is_some())
                    .count()
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_parse, bench_snapshot_spans);
criterion_main!(benches);
