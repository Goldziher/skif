//! Fix planning: `apply_fixes` turning N fixes against one `package.json` into a single file change.

#![allow(clippy::expect_used)]

use std::fmt::Write as _;
use std::hint::black_box;
use std::path::Path;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use serde_json::json;
use skif::{
    Diagnostic, Fix, FixOp, FixSafety, FixSelection, JsonAction, Location, MemoryFs, PathSeg, Report, RuleId, Severity,
    Workspace, apply_fixes, load_config,
};

const ROOT: &str = "/repo";
const MEMBER: &str = "packages/a/package.json";
const DEPENDENCIES: usize = 200;

fn member_manifest() -> String {
    let mut manifest = String::from("{\n  \"name\": \"a\",\n  \"version\": \"1.0.0\",\n  \"dependencies\": {\n");
    for i in 0..DEPENDENCIES {
        let comma = if i + 1 == DEPENDENCIES { "" } else { "," };
        let _ = writeln!(manifest, "    \"dep-{i:03}\": \"^1.{i}.0\"{comma}");
    }
    manifest.push_str("  }\n}\n");
    manifest
}

fn fixture() -> (MemoryFs, Workspace) {
    let fs = MemoryFs::new()
        .with_file(
            format!("{ROOT}/package.json"),
            r#"{"name": "root", "private": true, "workspaces": ["packages/*"]}"#,
        )
        .with_file(format!("{ROOT}/{MEMBER}"), member_manifest());
    let root = Path::new(ROOT);
    let config = load_config(root, &fs, &skif::ConfigOverrides::default()).expect("configuration loads");
    let workspace = Workspace::discover_with_fs(root, &config, &fs).expect("workspace is discoverable");
    (fs, workspace)
}

/// One diagnostic per fix; every fix rewrites a different dependency of the same file, so none conflict.
fn report(fixes: usize) -> Report {
    let diagnostics = (0..fixes)
        .map(|i| {
            let mut diagnostic = Diagnostic::new(
                RuleId::from_static("version-mismatch"),
                Severity::Warning,
                format!("dep-{i:03} is out of sync"),
                Location::new(
                    MEMBER,
                    None,
                ),
            );
            diagnostic.fixes.push(Fix::new(
                format!("align dep-{i:03}"),
                FixSafety::Safe,
                vec![FixOp::json(
                    MEMBER,
                    vec![PathSeg::Key("dependencies".into()), PathSeg::Key(format!("dep-{i:03}"))],
                    JsonAction::Set(json!("^2.0.0")),
                )],
            ));
            diagnostic
        })
        .collect();
    Report::new(diagnostics, 1)
}

fn bench_apply_fixes(c: &mut Criterion) {
    let (fs, workspace) = fixture();
    let selection = FixSelection::new(FixSafety::Safe);
    let mut group = c.benchmark_group("fix");
    for fixes in [1, 10, 100] {
        let report = report(fixes);
        group.throughput(Throughput::Elements(fixes as u64));
        group.bench_with_input(BenchmarkId::new("apply_fixes", fixes), &report, |b, report| {
            b.iter(|| apply_fixes(&workspace, black_box(report), &selection, &fs).expect("fixes plan"));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_apply_fixes);
criterion_main!(benches);
