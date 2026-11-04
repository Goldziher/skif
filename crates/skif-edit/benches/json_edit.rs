use {
    criterion::{Criterion, criterion_group, criterion_main},
    serde_json::json,
    skif_core::{FixOp, InsertPosition, JsonAction, PathSeg},
    skif_edit::{FileKind, apply, plan_edits},
    std::hint::black_box,
};

const DEPENDENCIES: usize = 500;

/// A package.json with `DEPENDENCIES` dependencies in descending order (so sorting has work to do).
fn package_json() -> String {
    let deps: Vec<String> = (0..DEPENDENCIES)
        .rev()
        .map(|i| format!("    \"dep-{i:04}\": \"^{}.{}.{}\"", i % 20, i % 30, i % 40))
        .collect();
    format!(
        "{{\n  \"name\": \"bench-package\",\n  \"version\": \"1.0.0\",\n  \"dependencies\": {{\n{}\n  }},\n  \"devDependencies\": {{}}\n}}\n",
        deps.join(",\n")
    )
}

fn op(pointer: Vec<PathSeg>, action: JsonAction) -> FixOp {
    FixOp::json(
        "package.json",
        pointer,
        action,
    )
}

fn key(name: &str) -> PathSeg {
    PathSeg::Key(name.to_owned())
}

fn bench_json_edit(c: &mut Criterion) {
    let source = package_json();
    let set = op(
        vec![key("dependencies"), key("dep-0250")],
        JsonAction::Set(json!("^9.9.9")),
    );
    let remove = op(vec![key("dependencies"), key("dep-0100")], JsonAction::Remove);
    let insert = op(
        vec![key("dependencies")],
        JsonAction::insert(
            "dep-new".to_owned(),
            json!("^1.0.0"),
            InsertPosition::Sorted,
        ),
    );
    let sort = op(vec![key("dependencies")], JsonAction::SortKeys);
    let rename = op(
        vec![key("dependencies"), key("dep-0300")],
        JsonAction::RenameKey("dep-renamed".to_owned()),
    );
    let many_sets: Vec<FixOp> = (0..DEPENDENCIES)
        .step_by(5)
        .map(|i| {
            op(
                vec![key("dependencies"), key(&format!("dep-{i:04}"))],
                JsonAction::Set(json!("^2.0.0")),
            )
        })
        .collect();

    let cases: Vec<(&str, Vec<&FixOp>)> = vec![
        ("set_one", vec![&set]),
        ("remove_one", vec![&remove]),
        ("insert_sorted", vec![&insert]),
        ("sort_keys_500", vec![&sort]),
        ("rename_key", vec![&rename]),
        ("set_100_versions", many_sets.iter().collect()),
    ];

    let mut group = c.benchmark_group("skif-edit/json_500_deps");
    for (name, ops) in &cases {
        assert!(
            apply(&source, FileKind::Json, ops).is_ok(),
            "{name} must be a valid edit"
        );
        group.bench_function(format!("apply/{name}"), |b| {
            b.iter(|| black_box(apply(black_box(&source), FileKind::Json, black_box(ops))));
        });
        group.bench_function(format!("plan/{name}"), |b| {
            b.iter(|| black_box(plan_edits(black_box(&source), FileKind::Json, black_box(ops))));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_json_edit);
criterion_main!(benches);
