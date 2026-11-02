use std::fmt::Write as _;
use {
    criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main},
    skif_core::{Diagnostic, LineIndex, Location, Report, RuleId, Severity},
    std::hint::black_box,
};

/// A package.json-like document: `lines` lines of ASCII, or the same with multi-byte characters mixed in.
fn document(lines: usize, utf8: bool) -> String {
    let mut out = String::with_capacity(lines * 48);
    for i in 0..lines {
        if utf8 && i % 3 == 0 {
            let _ = writeln!(out, "  \"description-{i}\": \"naïve café – 日本語 🚀 entry\",");
        } else {
            let _ = writeln!(out, "  \"@scope/package-{i}\": \"^{}.{}.{}\",", i % 20, i % 30, i % 40);
        }
    }
    out
}

fn bench_line_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("LineIndex");
    for (label, utf8) in [("ascii", false), ("utf8", true)] {
        let text = document(20_000, utf8);
        group.throughput(Throughput::Bytes(text.len() as u64));
        group.bench_with_input(BenchmarkId::new("new", label), &text, |b, text| {
            b.iter(|| LineIndex::new(black_box(text)));
        });

        let index = LineIndex::new(&text);
        let len = u32::try_from(text.len()).unwrap_or(u32::MAX);
        let offsets: Vec<u32> = (0..1000u32).map(|i| (i.wrapping_mul(2_654_435_761)) % len).collect();
        group.bench_with_input(BenchmarkId::new("line_col_x1000", label), &offsets, |b, offsets| {
            b.iter(|| {
                for &offset in offsets {
                    black_box(index.line_col(black_box(offset)));
                }
            });
        });
        group.bench_with_input(BenchmarkId::new("utf16_col_x1000", label), &offsets, |b, offsets| {
            b.iter(|| {
                for &offset in offsets {
                    black_box(index.utf16_col(black_box(offset)));
                }
            });
        });
        group.bench_with_input(BenchmarkId::new("span_x1000", label), &offsets, |b, offsets| {
            b.iter(|| {
                for &offset in offsets {
                    black_box(index.span(black_box(offset), black_box(offset.saturating_add(8))));
                }
            });
        });
    }
    group.finish();
}

const RULES: &[&str] = &[
    "version-mismatch",
    "unsorted-dependencies",
    "types-in-dependencies",
    "empty-dependencies",
    "unused-dependency",
];

fn diagnostics(count: usize) -> Vec<Diagnostic> {
    (0..count)
        .map(|i| {
            let package = (i.wrapping_mul(7919)) % 500;
            let start = u32::try_from((i.wrapping_mul(104_729)) % 4000).unwrap_or(0);
            Diagnostic {
                rule: RuleId::from_static(RULES[i % RULES.len()]),
                severity: match i % 3 {
                    0 => Severity::Error,
                    1 => Severity::Warning,
                    _ => Severity::Info,
                },
                message: format!("dependency dep-{i} is out of sync"),
                help: None,
                location: Location {
                    path: format!("packages/pkg-{package}/package.json").into(),
                    span: None,
                },
                related: Vec::new(),
                package: Some(format!("pkg-{package}")),
                package_id: None,
                dependency: Some(format!("dep-{start}")),
                detail: None,
                fixes: Vec::new(),
            }
        })
        .collect()
}

fn bench_report(c: &mut Criterion) {
    let mut group = c.benchmark_group("Report");
    group.sample_size(30);
    group.bench_function("new_10k", |b| {
        b.iter_batched(
            || diagnostics(10_000),
            |input| Report::new(input, 500),
            BatchSize::LargeInput,
        );
    });
    group.finish();
}

criterion_group!(benches, bench_line_index, bench_report);
criterion_main!(benches);
