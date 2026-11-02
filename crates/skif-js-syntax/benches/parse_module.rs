//! `parse_module` over a small corpus of realistic TS, TSX and JS files in `benches/corpus`.

#![allow(clippy::expect_used)]

use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use skif_js_syntax::{ParseOptions, parse_module};

const CORPUS: &[(&str, &str)] = &[
    ("Button.tsx", include_str!("corpus/Button.tsx")),
    ("Table.tsx", include_str!("corpus/Table.tsx")),
    ("api-client.ts", include_str!("corpus/api-client.ts")),
    ("legacy.js", include_str!("corpus/legacy.js")),
    ("reducer.ts", include_str!("corpus/reducer.ts")),
    ("store.ts", include_str!("corpus/store.ts")),
    ("utils.ts", include_str!("corpus/utils.ts")),
];

const FACTS_ONLY: ParseOptions = ParseOptions {
    in_file_references: false,
    member_accesses: false,
};

fn bench_corpus(c: &mut Criterion) {
    let bytes: usize = CORPUS.iter().map(|(_, source)| source.len()).sum();
    let mut group = c.benchmark_group("parse_module");
    group.throughput(Throughput::Bytes(bytes as u64));
    for (label, options) in [("full", ParseOptions::default()), ("facts_only", FACTS_ONLY)] {
        group.bench_function(BenchmarkId::new("corpus", label), |b| {
            b.iter(|| {
                for (name, source) in CORPUS {
                    black_box(parse_module(Path::new(name), source, &options).expect("corpus file parses"));
                }
            });
        });
    }
    group.finish();
}

fn bench_files(c: &mut Criterion) {
    let mut group = c.benchmark_group("parse_module/file");
    for (name, source) in CORPUS {
        group.throughput(Throughput::Bytes(source.len() as u64));
        group.bench_function(*name, |b| {
            b.iter(|| {
                parse_module(Path::new(name), black_box(source), &ParseOptions::default()).expect("corpus file parses")
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_corpus, bench_files);
criterion_main!(benches);
