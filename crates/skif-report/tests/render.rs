#![allow(clippy::unwrap_used, clippy::expect_used)]

use skif_core::{Diagnostic, Fix, FixOp, FixSafety, LineIndex, Location, Related, Report, RuleId, Severity, Span};
use skif_report::{Format, RenderOptions, render, render_json_error};

const MANIFEST: &str =
    "{\n  \"name\": \"web\",\n  \"dependencies\": {\n    \"réact\": \"^18.0.0\",\n    \"lodash\": \"^4.0.0\"\n  }\n}\n";

fn span_of(needle: &str) -> Span {
    let start = u32::try_from(MANIFEST.find(needle).unwrap()).unwrap();
    let end = start + u32::try_from(needle.len()).unwrap();
    LineIndex::new(MANIFEST).span(start, end)
}

fn fix(title: &str, safety: FixSafety) -> Fix {
    Fix {
        title: title.to_owned(),
        safety,
        ops: vec![FixOp::DeleteFile { path: "x".into() }],
    }
}

fn diag(rule: &'static str, severity: Severity, path: &str, span: Option<Span>, message: &str) -> Diagnostic {
    Diagnostic {
        rule: RuleId::from_static(rule),
        severity,
        message: message.to_owned(),
        help: None,
        location: Location {
            path: path.into(),
            span,
        },
        related: Vec::new(),
        package: None,
        dependency: None,
        detail: None,
        fixes: Vec::new(),
    }
}

fn rich() -> Report {
    let mut mismatch = diag(
        "version-mismatch",
        Severity::Error,
        "packages\\web\\package.json",
        Some(span_of("\"^18.0.0\"")),
        "réact is declared with 2 different versions",
    );
    mismatch.help = Some("align on ^18.3.1".to_owned());
    mismatch.package = Some("web".to_owned());
    mismatch.dependency = Some("réact".to_owned());
    mismatch.detail = Some("DiffersToHighestOrLowestSemver".to_owned());
    mismatch.fixes = vec![fix("Use ^18.3.1", FixSafety::Unsafe)];
    mismatch.related = vec![Related {
        message: "other declaration".to_owned(),
        location: Location {
            path: "packages/api/package.json".into(),
            span: Some(span_of("\"^4.0.0\"")),
        },
    }];
    let mut unused = diag(
        "unused-dependency",
        Severity::Warning,
        "packages/web/package.json",
        Some(span_of("\"lodash\"")),
        "lodash is never imported 日本語",
    );
    unused.fixes = vec![fix("Remove lodash", FixSafety::Safe)];
    let info = diag(
        "sorted-keys",
        Severity::Info,
        "package.json",
        None,
        "keys are not sorted",
    );
    Report::new(vec![unused, info, mismatch], 3)
}

fn options(color: bool) -> RenderOptions {
    RenderOptions::default()
        .with_color(color)
        .with_tool_version("0.1.0")
        .with_source("packages/web/package.json", MANIFEST)
}

fn run(report: &Report, format: Format, options: &RenderOptions) -> String {
    let mut out = Vec::new();
    render(report, format, options, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn human_empty() {
    insta::assert_snapshot!(run(&Report::new(Vec::new(), 2), Format::Human, &options(false)));
}

#[test]
fn human_rich() {
    insta::assert_snapshot!(run(&rich(), Format::Human, &options(false)));
}

#[test]
fn human_without_sources_skips_frames() {
    let rendered = run(&rich(), Format::Human, &RenderOptions::default());
    assert!(!rendered.contains("^^^"));
    assert!(rendered.contains("packages/web/package.json:4:14"));
}

#[test]
fn human_color_off_has_no_escapes_and_on_has() {
    assert!(!run(&rich(), Format::Human, &options(false)).contains('\x1b'));
    assert!(run(&rich(), Format::Human, &options(true)).contains('\x1b'));
}

#[test]
fn human_underline_uses_character_columns() {
    let rendered = run(&rich(), Format::Human, &options(false));
    let frame = rendered.lines().find(|line| line.contains("\"réact\"")).unwrap();
    let underline = rendered.lines().skip_while(|line| *line != frame).nth(1).unwrap();
    let col = |s: &str, marker: char| s.chars().position(|c| c == marker).unwrap();
    assert_eq!(col(underline, '^'), col(frame, '"') + "\"réact\": ".chars().count());
}

#[test]
fn json_empty() {
    insta::assert_snapshot!(run(&Report::new(Vec::new(), 2), Format::Json, &options(false)));
}

#[test]
fn json_rich() {
    insta::assert_snapshot!(run(&rich(), Format::Json, &options(false)));
}

#[test]
fn json_is_valid_relative_and_color_independent() {
    let rendered = run(&rich(), Format::Json, &options(true));
    assert!(!rendered.contains('\x1b'));
    assert!(!rendered.contains('\\'));
    let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["summary"]["errors"], 1);
    assert_eq!(value["diagnostics"].as_array().unwrap().len(), 3);
}

#[test]
fn json_is_deterministic() {
    let report = rich();
    assert_eq!(
        run(&report, Format::Json, &options(false)),
        run(&report, Format::Json, &options(false))
    );
}

#[test]
fn json_paths_are_relativised_to_root() {
    let report = Report::new(
        vec![diag(
            "sorted-keys",
            Severity::Info,
            "C:\\repo\\pkg\\package.json",
            None,
            "m",
        )],
        1,
    );
    let rendered = run(&report, Format::Json, &RenderOptions::default().with_root("C:\\repo"));
    assert!(rendered.contains("\"path\": \"pkg/package.json\""));
}

#[test]
fn json_error_document() {
    let mut out = Vec::new();
    render_json_error("bad \"config\"", &mut out).unwrap();
    insta::assert_snapshot!(String::from_utf8(out).unwrap());
}

#[test]
fn detailed_error_document_carries_kind_and_location() {
    let info = skif_report::ErrorInfo::new("config", "bad key").at("skif.toml", Some(3), Some(5));
    let mut out = Vec::new();
    skif_report::render_json_error_info(&info, &mut out).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(
        value,
        serde_json::json!({
            "version": 1,
            "error": {"kind": "config", "message": "bad key", "path": "skif.toml", "line": 3, "column": 5}
        })
    );
}

#[test]
fn extra_members_follow_the_diagnostics() {
    let report = Report::new(Vec::new(), 2);
    let mut extra = serde_json::Map::new();
    extra.insert("fix".to_owned(), serde_json::json!({"changed": ["package.json"]}));
    let mut out = Vec::new();
    skif_report::render_json_with(&report, &RenderOptions::default(), extra, &mut out).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(value["fix"]["changed"][0], "package.json");
    assert_eq!(value["summary"]["packages"], 2);
}
