#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::Value;
use skif_core::{
    Category, Diagnostic, Fix, FixOp, FixSafety, Fixability, LineIndex, Location, Related, Report, RuleId, RuleMeta,
    Severity, Span,
};
use skif_report::{Format, RenderOptions, render};

const MANIFEST: &str =
    "{\n  \"name\": \"web\",\n  \"dependencies\": {\n    \"réact\": \"^18.0.0\",\n    \"lodash\": \"^4.0.0\"\n  }\n}\n";

static VERSION_MISMATCH: RuleMeta = RuleMeta {
    id: "version-mismatch",
    category: Category::Policy,
    default: Some(Severity::Error),
    fixable: Fixability::Sometimes,
    summary: "A dependency is declared with different versions.",
};

static UNUSED: RuleMeta = RuleMeta {
    id: "unused-dependency",
    category: Category::Manifest,
    default: Some(Severity::Warning),
    fixable: Fixability::Safe,
    summary: "A dependency is never imported.",
};

static SORTED_KEYS: RuleMeta = RuleMeta {
    id: "sorted-keys",
    category: Category::Manifest,
    default: None,
    fixable: Fixability::None,
    summary: "Keys are not sorted.",
};

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
    let info = diag("sorted-keys", Severity::Info, "package.json", None, "keys are not sorted");
    Report::new(vec![unused, info, mismatch], 3)
}

fn options() -> RenderOptions {
    RenderOptions::default()
        .with_tool_version("0.1.0")
        .with_rules([&VERSION_MISMATCH, &UNUSED, &SORTED_KEYS])
}

fn run(report: &Report, format: Format, options: &RenderOptions) -> String {
    let mut out = Vec::new();
    render(report, format, options, &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

fn sarif(report: &Report, options: &RenderOptions) -> Value {
    serde_json::from_str(&run(report, Format::Sarif, options)).unwrap()
}

#[test]
fn sarif_empty() {
    insta::assert_snapshot!(run(&Report::new(Vec::new(), 2), Format::Sarif, &options()));
}

#[test]
fn sarif_rich() {
    insta::assert_snapshot!(run(&rich(), Format::Sarif, &options()));
}

#[test]
fn sarif_structure() {
    let log = sarif(&rich(), &options());
    assert_eq!(log["version"], "2.1.0");
    assert_eq!(log["runs"].as_array().unwrap().len(), 1);
    let run = &log["runs"][0];
    assert_eq!(run["columnKind"], "unicodeCodePoints");
    assert_eq!(run["tool"]["driver"]["name"], "skif");
    assert_eq!(run["tool"]["driver"]["version"], "0.1.0");
    let rules = run["tool"]["driver"]["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 3);
    let levels: Vec<_> = run["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["level"].as_str().unwrap())
        .collect();
    assert_eq!(levels, ["error", "warning", "note"]);
    for result in run["results"].as_array().unwrap() {
        let index = usize::try_from(result["ruleIndex"].as_u64().unwrap()).unwrap();
        assert_eq!(rules[index]["id"], result["ruleId"]);
        let uri = result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
            .as_str()
            .unwrap();
        assert!(!uri.contains('\\'));
        assert_eq!(
            result["locations"][0]["physicalLocation"]["artifactLocation"]["uriBaseId"],
            "%SRCROOT%"
        );
        assert_eq!(result["partialFingerprints"]["skifFingerprint/v1"].as_str().unwrap().len(), 16);
    }
}

#[test]
fn sarif_regions_use_scalar_columns_and_skip_missing_spans() {
    let log = sarif(&rich(), &options());
    let results = log["runs"][0]["results"].as_array().unwrap();
    let mismatch = results.iter().find(|r| r["ruleId"] == "version-mismatch").unwrap();
    // `"réact": ` is 10 scalar values (11 UTF-8 bytes) after a 4-space indent.
    assert_eq!(mismatch["locations"][0]["physicalLocation"]["region"]["startColumn"], 15);
    let info = results.iter().find(|r| r["ruleId"] == "sorted-keys").unwrap();
    assert!(info["locations"][0]["physicalLocation"].get("region").is_none());
}

#[test]
fn sarif_unknown_rules_are_appended_and_disabled_rules_marked() {
    let report = Report::new(vec![diag("custom", Severity::Warning, "a.json", None, "m")], 1);
    let log = sarif(&report, &options());
    let rules = log["runs"][0]["tool"]["driver"]["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 4);
    assert_eq!(rules[3]["id"], "custom");
    assert_eq!(log["runs"][0]["results"][0]["ruleIndex"], 3);
    assert_eq!(rules[2]["defaultConfiguration"]["enabled"], false);
    assert!(rules[2]["defaultConfiguration"].get("level").is_none());
}

#[test]
fn sarif_fingerprints_ignore_position_and_differ_by_message() {
    let at = |span, message: &str| {
        let report = Report::new(vec![diag("unused-dependency", Severity::Warning, "a.json", span, message)], 1);
        sarif(&report, &options())["runs"][0]["results"][0]["partialFingerprints"]["skifFingerprint/v1"].clone()
    };
    assert_eq!(at(None, "m"), at(Some(span_of("\"lodash\"")), "m"));
    assert_ne!(at(None, "m"), at(None, "n"));
}

#[test]
fn sarif_is_deterministic() {
    let report = rich();
    assert_eq!(
        run(&report, Format::Sarif, &options()),
        run(&report, Format::Sarif, &options())
    );
}

#[test]
fn github_empty() {
    insta::assert_snapshot!(run(&Report::new(Vec::new(), 2), Format::Github, &options()));
}

#[test]
fn github_rich() {
    insta::assert_snapshot!(run(&rich(), Format::Github, &options()));
}

#[test]
fn github_escapes_messages_and_properties() {
    let report = Report::new(
        vec![diag(
            "unused-dependency",
            Severity::Warning,
            "dir:a,b/package.json",
            None,
            "100% bad\r\nsecond: line, here",
        )],
        1,
    );
    let rendered = run(&report, Format::Github, &options());
    assert_eq!(
        rendered.lines().next().unwrap(),
        "::warning file=dir%3Aa%2Cb/package.json,title=skif unused-dependency::100%25 bad%0D%0Asecond: line, here"
    );
}

#[test]
fn github_info_is_a_notice() {
    let report = Report::new(vec![diag("sorted-keys", Severity::Info, "p.json", None, "m")], 1);
    assert!(run(&report, Format::Github, &options()).starts_with("::notice file=p.json,title=skif sorted-keys::m\n"));
}
