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
    Fix::new(title.to_owned(), safety, vec![FixOp::delete_file("x")])
}

fn diag(rule: &'static str, severity: Severity, path: &str, span: Option<Span>, message: &str) -> Diagnostic {
    Diagnostic::new(RuleId::from_static(rule), severity, message, Location::new(path, span))
}

fn rich() -> Report {
    let mut mismatch = diag(
        "version-mismatch",
        Severity::Error,
        "packages/web/package.json",
        Some(span_of("\"^18.0.0\"")),
        "réact is declared with 2 different versions",
    );
    mismatch.help = Some("align on ^18.3.1".to_owned());
    mismatch.package = Some("web".to_owned());
    mismatch.dependency = Some("réact".to_owned());
    mismatch.detail = Some("DiffersToHighestOrLowestSemver".to_owned());
    mismatch.fixes = vec![fix("Use ^18.3.1", FixSafety::Unsafe)];
    mismatch.related = vec![Related::new(
        "other declaration".to_owned(),
        Location::new("packages/api/package.json", Some(span_of("\"^4.0.0\""))),
    )];
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
    // `\` is a separator only on Windows; elsewhere it is an ordinary file-name character.
    let (path, root) = if cfg!(windows) {
        ("C:\\repo\\pkg\\package.json", "C:\\repo")
    } else {
        ("/repo/pkg/package.json", "/repo")
    };
    let report = Report::new(vec![diag("sorted-keys", Severity::Info, path, None, "m")], 1);
    let rendered = run(&report, Format::Json, &RenderOptions::default().with_root(root));
    assert!(rendered.contains("\"path\": \"pkg/package.json\""), "{rendered}");
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
            "skif": env!("CARGO_PKG_VERSION"),
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

#[test]
fn json_documents_always_carry_the_tool_version() {
    let rendered = run(&Report::new(Vec::new(), 1), Format::Json, &RenderOptions::default());
    let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
    assert_eq!(value["skif"], env!("CARGO_PKG_VERSION"));
    let mut out = Vec::new();
    render_json_error("boom", &mut out).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(value["skif"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn extra_members_cannot_shadow_reserved_keys() {
    for key in ["version", "skif", "summary", "diagnostics"] {
        let mut extra = serde_json::Map::new();
        extra.insert(key.to_owned(), serde_json::json!("x"));
        let mut out = Vec::new();
        let error =
            skif_report::render_json_with(&Report::new(Vec::new(), 1), &RenderOptions::default(), extra, &mut out)
                .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{key}");
        assert!(out.is_empty(), "{key}");
    }
}

#[test]
fn human_output_escapes_control_characters() {
    let report = Report::new(
        vec![diag(
            "sorted-keys",
            Severity::Warning,
            "package.json",
            None,
            "evil \u{1b}[2J message \u{202e}",
        )],
        1,
    );
    let rendered = run(&report, Format::Human, &RenderOptions::default());
    assert!(!rendered.contains('\u{1b}'), "{rendered:?}");
    assert!(!rendered.contains('\u{202e}'), "{rendered:?}");
    assert!(rendered.contains("evil \\u{1b}[2J message \\u{202e}"), "{rendered:?}");
}

#[test]
fn human_underline_is_measured_in_terminal_cells() {
    let source = "{\n  \"\u{4e2d}\u{6587}\": \"x\"\n}\n";
    let start = u32::try_from(source.find("\"x\"").unwrap()).unwrap();
    let span = LineIndex::new(source).span(start, start + 3);
    let report = Report::new(
        vec![diag(
            "sorted-keys",
            Severity::Warning,
            "package.json",
            Some(span),
            "wide",
        )],
        1,
    );
    let options = RenderOptions::default().with_source("package.json", source);
    let rendered = run(&report, Format::Human, &options);
    let frame = rendered.lines().find(|line| line.contains('\u{4e2d}')).unwrap();
    let underline = rendered.lines().skip_while(|line| *line != frame).nth(1).unwrap();
    let bar = |line: &str| line.find("| ").unwrap() + 2;
    let before_value = &frame[bar(frame)..frame.find("\"x\"").unwrap()];
    let cells: usize = before_value
        .chars()
        .map(|c| if c == '\u{4e2d}' || c == '\u{6587}' { 2 } else { 1 })
        .sum();
    assert_eq!(underline[bar(underline)..].find('^').unwrap(), cells);
}

#[test]
fn human_diagnostics_are_ordered_by_position_within_a_file() {
    let late = diag(
        "unused-dependency",
        Severity::Warning,
        "package.json",
        Some(span_of("\"lodash\"")),
        "late",
    );
    let early = diag(
        "version-mismatch",
        Severity::Warning,
        "package.json",
        Some(span_of("\"name\"")),
        "early",
    );
    let rendered = run(
        &Report::new(vec![late, early], 1),
        Format::Human,
        &RenderOptions::default(),
    );
    assert!(
        rendered.find("early").unwrap() < rendered.find("late").unwrap(),
        "{rendered}"
    );
}

#[test]
fn the_specifier_is_reported_when_set() {
    let mut with = diag("sorted-keys", Severity::Warning, "package.json", None, "m");
    with.specifier = Some("^1.0.0".to_owned());
    let without = diag("sorted-keys", Severity::Warning, "package.json", None, "m");
    let rendered = run(
        &Report::new(vec![with, without], 1),
        Format::Json,
        &RenderOptions::default(),
    );
    let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
    assert_eq!(value["diagnostics"][0]["specifier"], "^1.0.0");
    assert!(value["diagnostics"][1].get("specifier").is_none());
    let sarif = run(
        &Report::new(vec![diag("sorted-keys", Severity::Warning, "p.json", None, "m")], 1),
        Format::Sarif,
        &RenderOptions::default(),
    );
    assert!(!sarif.contains("\"specifier\""));
}

#[test]
fn codeclimate_rich() {
    insta::assert_snapshot!(run(&rich(), Format::Codeclimate, &options(false)));
}

#[test]
fn codeclimate_empty_is_an_empty_array() {
    let rendered = run(&Report::new(Vec::new(), 2), Format::Codeclimate, &options(false));
    assert_eq!(rendered, "[]\n");
}

#[test]
fn codeclimate_fingerprints_are_unique_and_ignore_the_line() {
    let at = |line: u32| {
        let mut span = span_of("\"lodash\"");
        span.start_line = line;
        diag(
            "unused-dependency",
            Severity::Warning,
            "a/package.json",
            Some(span),
            "m",
        )
    };
    let fingerprints = |report: &Report| -> Vec<String> {
        let value: serde_json::Value =
            serde_json::from_str(&run(report, Format::Codeclimate, &options(false))).unwrap();
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|issue| issue["fingerprint"].as_str().unwrap().to_owned())
            .collect()
    };
    let twice = fingerprints(&Report::new(vec![at(5), at(9)], 1));
    assert_ne!(twice[0], twice[1]);
    assert_eq!(
        fingerprints(&Report::new(vec![at(5)], 1))[0],
        fingerprints(&Report::new(vec![at(40)], 1))[0]
    );
}

#[test]
fn compact_rich() {
    insta::assert_snapshot!(run(&rich(), Format::Compact, &options(false)));
}

#[test]
fn compact_keeps_one_line_per_diagnostic() {
    let report = Report::new(
        vec![diag("sorted-keys", Severity::Warning, "package.json", None, "one\ntwo")],
        1,
    );
    assert_eq!(
        run(&report, Format::Compact, &options(false)),
        "package.json sorted-keys one\\u{a}two\n"
    );
}

#[test]
fn markdown_rich() {
    insta::assert_snapshot!(run(&rich(), Format::Markdown, &options(false)));
}

#[test]
fn markdown_empty() {
    insta::assert_snapshot!(run(&Report::new(Vec::new(), 2), Format::Markdown, &options(false)));
}

#[test]
fn disclosure_rich() {
    insta::assert_snapshot!(run(&rich(), Format::Disclosure, &options(false)));
}

#[test]
fn codeowners_rich() {
    let codeowners =
        skif_report::CodeOwners::parse("* @org/core\n/packages/web/ @org/web @org/design\n/package.json\n");
    insta::assert_snapshot!(run(
        &rich(),
        Format::Codeowners,
        &options(false).with_codeowners(codeowners)
    ));
}

#[test]
fn codeowners_without_a_file_lists_every_diagnostic_as_unowned() {
    let rendered = run(&rich(), Format::Codeowners, &options(false));
    assert!(rendered.starts_with("(unowned) (3)\n"), "{rendered}");
}

#[test]
fn every_format_is_deterministic() {
    let report = rich();
    for format in [
        Format::Codeclimate,
        Format::Compact,
        Format::Markdown,
        Format::Disclosure,
        Format::Codeowners,
    ] {
        assert_eq!(
            run(&report, format, &options(false)),
            run(&report, format, &options(false)),
            "{format:?}"
        );
    }
}
