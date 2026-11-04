#![allow(clippy::unwrap_used, clippy::expect_used)]

use serde_json::Value;
use skif_core::{
    Category, Diagnostic, Fix, FixOp, FixSafety, Fixability, LineIndex, Location, Related, Report, RuleId, RuleMeta,
    Severity, Span,
};
use skif_report::{Format, RenderOptions, render};

const MANIFEST: &str =
    "{\n  \"name\": \"web\",\n  \"dependencies\": {\n    \"réact\": \"^18.0.0\",\n    \"lodash\": \"^4.0.0\"\n  }\n}\n";

static VERSION_MISMATCH: RuleMeta = RuleMeta::new(
    "version-mismatch",
    Category::Policy,
    Some(Severity::Error),
    Fixability::Sometimes,
    "A dependency is declared with different versions.",
);

static UNUSED: RuleMeta = RuleMeta::new(
    "unused-dependency",
    Category::Manifest,
    Some(Severity::Warning),
    Fixability::Safe,
    "A dependency is never imported.",
);

static SORTED_KEYS: RuleMeta = RuleMeta::new(
    "sorted-keys",
    Category::Manifest,
    None,
    Fixability::None,
    "Keys are not sorted.",
);

fn span_of(needle: &str) -> Span {
    let start = u32::try_from(MANIFEST.find(needle).unwrap()).unwrap();
    let end = start + u32::try_from(needle.len()).unwrap();
    LineIndex::new(MANIFEST).span(start, end)
}

fn fix(title: &str, safety: FixSafety) -> Fix {
    Fix::new(
        title.to_owned(),
        safety,
        vec![FixOp::delete_file("x")],
    )
}

fn diag(rule: &'static str, severity: Severity, path: &str, span: Option<Span>, message: &str) -> Diagnostic {
    Diagnostic::new(
        RuleId::from_static(rule),
        severity,
        message,
        Location::new(
            path,
            span,
        ),
    )
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
        Location::new(
            "packages/api/package.json",
            Some(span_of("\"^4.0.0\"")),
        ),
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
    // Results follow the report's order: by path, then by position.
    assert_eq!(levels, ["note", "error", "warning"]);
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
        assert_eq!(
            result["partialFingerprints"]["skifFingerprint/v1"]
                .as_str()
                .unwrap()
                .len(),
            // 16 hex digits of the hash, then `:` and the occurrence number.
            18
        );
    }
}

#[test]
fn sarif_regions_use_scalar_columns_and_skip_missing_spans() {
    let log = sarif(&rich(), &options());
    let results = log["runs"][0]["results"].as_array().unwrap();
    let mismatch = results.iter().find(|r| r["ruleId"] == "version-mismatch").unwrap();
    // `"réact": ` is 9 scalar values (10 UTF-8 bytes) after a 4-space indent, so the value starts at 14.
    assert_eq!(
        mismatch["locations"][0]["physicalLocation"]["region"]["startColumn"],
        14
    );
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
        let report = Report::new(
            vec![diag("unused-dependency", Severity::Warning, "a.json", span, message)],
            1,
        );
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
        "::warning file=dir%3Aa%2Cb/package.json,title=unused-dependency::100%25 bad%0D%0Asecond: line, here"
    );
}

#[test]
fn github_info_is_a_notice() {
    let report = Report::new(vec![diag("sorted-keys", Severity::Info, "p.json", None, "m")], 1);
    assert!(run(&report, Format::Github, &options()).starts_with("::notice file=p.json,title=sorted-keys::m\n"));
}

fn sarif_of_one(path: &str, span: Option<Span>, message: &str, options: &RenderOptions) -> Value {
    let report = Report::new(vec![diag("sorted-keys", Severity::Warning, path, span, message)], 1);
    sarif(&report, options)
}

#[test]
fn sarif_uris_are_percent_encoded() {
    let log = sarif_of_one("packages/@scope/a b/package.json", None, "m", &options());
    let uri = &log["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"];
    assert_eq!(uri, "packages/%40scope/a%20b/package.json");
}

#[test]
fn sarif_fingerprints_of_identical_diagnostics_differ() {
    let first = diag("sorted-keys", Severity::Warning, "package.json", None, "same");
    let second = diag("sorted-keys", Severity::Warning, "package.json", None, "same");
    let other = diag("sorted-keys", Severity::Warning, "package.json", None, "different");
    let log = sarif(&Report::new(vec![first, second, other], 1), &options());
    let prints: Vec<&str> = log["runs"][0]["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|result| result["partialFingerprints"]["skifFingerprint/v1"].as_str().unwrap())
        .collect();
    assert_eq!(
        prints.iter().collect::<std::collections::BTreeSet<_>>().len(),
        3,
        "{prints:?}"
    );
    // The report sorts by message, so `different` comes first; the two identical ones are numbered in order.
    let mut occurrences: Vec<&str> = prints.iter().map(|print| &print[print.len() - 2..]).collect();
    occurrences.sort_unstable();
    assert_eq!(occurrences, [":1", ":1", ":2"], "{prints:?}");
}

#[test]
fn sarif_columns_are_utf16_when_the_source_is_known() {
    let source = "{\n  \"a\u{1F600}b\": 1\n}\n";
    let start = u32::try_from(source.find('b').unwrap()).unwrap();
    let span = LineIndex::new(source).span(start, start + 1);
    assert_eq!(span.start_col, 6);

    let known = options().with_source("package.json", source);
    let region = &sarif_of_one("package.json", Some(span), "m", &known)["runs"][0];
    assert_eq!(region["columnKind"], "utf16CodeUnits");
    assert_eq!(
        region["results"][0]["locations"][0]["physicalLocation"]["region"]["startColumn"],
        7
    );

    let unknown = &sarif_of_one("package.json", Some(span), "m", &options())["runs"][0];
    assert_eq!(unknown["columnKind"], "unicodeCodePoints");
    assert_eq!(
        unknown["results"][0]["locations"][0]["physicalLocation"]["region"]["startColumn"],
        6
    );
}

#[test]
fn github_annotations_are_relative_to_the_repository_root() {
    let report = Report::new(
        vec![diag(
            "sorted-keys",
            Severity::Warning,
            "packages/a/package.json",
            None,
            "m",
        )],
        1,
    );
    let with = RenderOptions::default().with_root("/repo/sub").with_repo_root("/repo");
    let rendered = run(&report, Format::Github, &with);
    assert!(rendered.contains("file=sub/packages/a/package.json"), "{rendered}");

    let same = RenderOptions::default().with_root("/repo").with_repo_root("/repo");
    assert!(run(&report, Format::Github, &same).contains("file=packages/a/package.json"));

    let without = RenderOptions::default().with_root("/repo/sub");
    assert!(run(&report, Format::Github, &without).contains("file=packages/a/package.json"));
}
