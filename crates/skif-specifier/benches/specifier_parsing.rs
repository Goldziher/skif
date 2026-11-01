// Vendored from syncpack crates/syncpack-specifier/benches/specifier_parsing.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use {
    criterion::{BatchSize, BenchmarkId, Criterion, criterion_group, criterion_main},
    skif_specifier::Specifier,
    std::{hint::black_box, sync::Arc, time::Duration},
};

/// Specifier shapes in the proportions found in real monorepos (roughly: mostly carets, some tildes and pins,
/// a tail of protocols and complex ranges).
const SHAPES: &[&str] = &[
    "^{a}.{b}.{c}",
    "^{a}.{b}.{c}",
    "^{a}.{b}.{c}",
    "^{a}.{b}.{c}",
    "^{a}.{b}.{c}",
    "^{a}.{b}.{c}",
    "~{a}.{b}.{c}",
    "~{a}.{b}.{c}",
    "{a}.{b}.{c}",
    "{a}.{b}.{c}",
    "{a}.{b}.{c}-beta.{c}",
    ">={a}.{b}.{c}",
    ">={a}.{b}.{c} <{a}.99.0",
    ">={a}.0.0 || >={b}.0.0",
    "workspace:*",
    "workspace:^",
    "catalog:",
    "catalog:react{a}",
    "npm:lodash@^{a}.{b}.{c}",
    "github:user/repo#v{a}.{b}.{c}",
    "file:../packages/shared",
    "link:../packages/shared",
    "latest",
    "*",
];

/// Deterministic specifier strings: a splitmix64 stream picks shapes and version numbers.
fn corpus(len: usize) -> Vec<String> {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    };
    (0..len)
        .map(|_| {
            let shape = SHAPES[usize::try_from(next() % SHAPES.len() as u64).unwrap_or(0)];
            shape
                .replace("{a}", &(next() % 20).to_string())
                .replace("{b}", &(next() % 30).to_string())
                .replace("{c}", &(next() % 40).to_string())
        })
        .collect()
}

/// Inputs covering every Specifier variant
const INPUTS: &[(&str, &str)] = &[
    // Simple semver
    ("exact", "1.2.3"),
    ("exact_tag", "1.2.3-beta.1"),
    ("major", "1"),
    ("minor", "1.2"),
    ("latest_star", "*"),
    ("latest_keyword", "latest"),
    // Ranges
    ("range_caret", "^1.2.3"),
    ("range_tilde", "~1.2.3"),
    ("range_gt", ">1.2.3"),
    ("range_gte", ">=1.2.3"),
    ("range_lt", "<1.2.3"),
    ("range_lte", "<=1.2.3"),
    ("range_caret_tag", "^1.2.3-beta.1"),
    ("range_major_caret", "^1"),
    ("range_minor_tilde", "~1.2"),
    // Complex
    ("complex_or", ">=1.0.0 <2.0.0 || >=3.0.0"),
    ("complex_and", ">=1.0.0 <2.0.0"),
    // Workspace protocol
    ("workspace_star", "workspace:*"),
    ("workspace_caret", "workspace:^1.2.3"),
    // Non-semver
    ("catalog", "catalog:react18"),
    ("alias", "npm:lodash@^4.17.21"),
    ("tag", "beta"),
    ("git_github", "github:user/repo#v1.2.3"),
    ("file", "file:../packages/foo"),
    ("link", "link:../packages/foo"),
    ("url", "https://example.com/package.tgz"),
    ("unsupported", "}wat{"),
    ("empty", ""),
];

fn bench_specifier_create(c: &mut Criterion) {
    let mut group = c.benchmark_group("Specifier::create");
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(2));
    for &(name, input) in INPUTS {
        group.bench_with_input(BenchmarkId::new("create", name), input, |b, input| {
            b.iter(|| Specifier::create(black_box(input)));
        });
    }
    group.finish();
}

fn bench_corpus(c: &mut Criterion) {
    let strings = corpus(2000);
    let parsed: Vec<Arc<Specifier>> = strings.iter().map(|s| Specifier::new(s)).collect();
    let mut group = c.benchmark_group("corpus");
    group.warm_up_time(Duration::from_secs(1));
    group.measurement_time(Duration::from_secs(3));

    group.bench_function("create_2000", |b| {
        b.iter(|| {
            for value in &strings {
                black_box(Specifier::create(black_box(value)));
            }
        });
    });
    group.bench_function("new_cached_2000", |b| {
        b.iter(|| {
            for value in &strings {
                black_box(Specifier::new(black_box(value)));
            }
        });
    });
    group.bench_function("satisfies_all_range_vs_5", |b| {
        let range = Specifier::new("^4.17.0");
        let others: Vec<Arc<Specifier>> = ["^4.17.21", "~4.17.0", "4.17.1", ">=4.0.0", "^4.0.0"]
            .iter()
            .map(|s| Specifier::new(s))
            .collect();
        b.iter(|| black_box(range.satisfies_all(black_box(&others))));
    });
    group.bench_function("satisfies_all_corpus_pairs", |b| {
        b.iter(|| {
            for pair in parsed.chunks(6) {
                if let Some((first, rest)) = pair.split_first() {
                    black_box(first.satisfies_all(black_box(rest)));
                }
            }
        });
    });
    group.bench_function("sort_2000", |b| {
        b.iter_batched(
            || parsed.clone(),
            |mut items| {
                items.sort();
                items
            },
            BatchSize::LargeInput,
        );
    });
    group.finish();
}

criterion_group!(benches, bench_specifier_create, bench_corpus);
criterion_main!(benches);
