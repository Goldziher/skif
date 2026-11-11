//! The SARIF 2.1.0 renderer.
//!
//! Columns follow ADR 0012: converted to UTF-16 code units (`utf16CodeUnits`) using the source text registered with
//! [`RenderOptions::with_source`]; when some file's text is unknown the run declares `unicodeCodePoints` and emits
//! skif's scalar-value columns unchanged. Paths are percent-encoded URI references. Fixes are
//! operations on structured files rather than text replacements, so they are listed under `properties.fixes`
//! instead of the SARIF `fixes` array, which requires concrete artifact changes.

use skif_core::HashMap;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::Path;

use serde::Serialize;
use skif_core::{
    Category, Diagnostic, Fix, FixSafety, Fixability, Location, Related, Report, RuleMeta, Severity, Span,
};

use crate::options::RenderOptions;
use crate::path::is_absolute_text;

const SCHEMA_URI: &str = "https://json.schemastore.org/sarif-2.1.0.json";
const SARIF_VERSION: &str = "2.1.0";
const INFORMATION_URI: &str = "https://github.com/Goldziher/skif";
const SRCROOT: &str = "%SRCROOT%";

#[derive(Serialize)]
struct Document<'a> {
    #[serde(rename = "$schema")]
    schema: &'static str,
    version: &'static str,
    runs: [Run<'a>; 1],
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Run<'a> {
    tool: Tool<'a>,
    column_kind: &'static str,
    results: Vec<SarifResult<'a>>,
}

#[derive(Serialize)]
struct Tool<'a> {
    driver: Driver<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Driver<'a> {
    name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<&'a str>,
    information_uri: &'static str,
    rules: Vec<SarifRule<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifRule<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    short_description: Option<Text<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    help: Option<Text<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_configuration: Option<Configuration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    properties: Option<RuleProperties>,
}

#[derive(Serialize)]
struct Text<'a> {
    text: &'a str,
}

#[derive(Serialize)]
struct Configuration {
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    level: Option<&'static str>,
}

#[derive(Serialize)]
struct RuleProperties {
    category: Category,
    fixable: Fixability,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifResult<'a> {
    rule_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    rule_index: Option<usize>,
    level: &'static str,
    message: Text<'a>,
    locations: [SarifLocation<'a>; 1],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    related_locations: Vec<SarifLocation<'a>>,
    partial_fingerprints: Fingerprints,
    properties: ResultProperties<'a>,
}

#[derive(Serialize)]
struct Fingerprints {
    #[serde(rename = "skifFingerprint/v1")]
    skif: String,
}

#[derive(Serialize)]
struct ResultProperties<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    help: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    package: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dependency: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    specifier: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
    fixes: Vec<SarifFix<'a>>,
}

#[derive(Serialize)]
struct SarifFix<'a> {
    title: &'a str,
    safety: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifLocation<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<usize>,
    physical_location: PhysicalLocation,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<Text<'a>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PhysicalLocation {
    artifact_location: ArtifactLocation,
    #[serde(skip_serializing_if = "Option::is_none")]
    region: Option<Region>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactLocation {
    uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    uri_base_id: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Region {
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
}

const fn level(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        _ => "note",
    }
}

fn rule_entry(meta: &RuleMeta) -> SarifRule<'_> {
    SarifRule {
        id: meta.id,
        short_description: Some(Text { text: meta.summary }),
        help: Some(Text { text: meta.summary }),
        default_configuration: Some(Configuration {
            enabled: meta.default.is_some(),
            level: meta.default.map(level),
        }),
        properties: Some(RuleProperties {
            category: meta.category,
            fixable: meta.fixable,
        }),
    }
}

/// Rules from the options, followed by any rule that only appears in results, in order of first appearance, with
/// the index of each rule ID's first entry: every result looks its rule up there instead of scanning the list.
fn rules<'a>(report: &'a Report, options: &'a RenderOptions) -> (Vec<SarifRule<'a>>, HashMap<&'a str, usize>) {
    let mut rules: Vec<SarifRule<'a>> = options.rules().iter().copied().map(rule_entry).collect();
    let mut index: HashMap<&'a str, usize> = HashMap::default();
    for (position, rule) in rules.iter().enumerate() {
        index.entry(rule.id).or_insert(position);
    }
    for diagnostic in &report.diagnostics {
        let id = diagnostic.rule.as_str();
        if !index.contains_key(id) {
            index.insert(id, rules.len());
            rules.push(SarifRule {
                id,
                short_description: None,
                help: None,
                default_configuration: None,
                properties: None,
            });
        }
    }
    (rules, index)
}

/// Percent-encodes a `/`-separated path as a URI reference: everything but unreserved characters and `/`.
fn encode_uri(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// Whether the column of `span`'s ends can be converted to UTF-16 for the file at `path`.
fn convertible(options: &RenderOptions, path: &str, span: &Span) -> bool {
    options.utf16_column(path, span.start_line, span.start_col).is_some()
        && options.utf16_column(path, span.end_line, span.end_col).is_some()
}

/// Whether every region of the report can be expressed in UTF-16 columns.
fn all_convertible(report: &Report, options: &RenderOptions) -> bool {
    report.diagnostics.iter().all(|diagnostic| {
        let mut locations = std::iter::once(&diagnostic.location).chain(diagnostic.related.iter().map(|r| &r.location));
        locations.all(|location| {
            location
                .span
                .as_ref()
                .is_none_or(|span| convertible(options, &options.display_path(&location.path), span))
        })
    })
}

fn region(span: &Span, path: &str, options: &RenderOptions, utf16: bool) -> Region {
    let column = |line: u32, column: u32| {
        if utf16 {
            options.utf16_column(path, line, column).unwrap_or(column)
        } else {
            column
        }
    };
    Region {
        start_line: span.start_line,
        start_column: column(span.start_line, span.start_col),
        end_line: span.end_line,
        end_column: column(span.end_line, span.end_col),
    }
}

/// Splits a Windows drive (`/C:`) off the front of a rooted path, so the colon stays literal as RFC 8089 writes it.
fn split_drive(rooted: &str) -> (&str, &str) {
    match rooted.as_bytes() {
        [b'/', letter, b':', ..] if letter.is_ascii_alphabetic() => rooted.split_at(3),
        _ => ("", rooted),
    }
}

/// The artifact of a location: repository-relative under `%SRCROOT%`, or a `file:` URI when the path lies outside
/// the workspace and so has no relative form.
fn artifact(options: &RenderOptions, path: &Path) -> ArtifactLocation {
    let shown = options.repo_path(path);
    if is_absolute_text(&shown) {
        let rooted = if shown.starts_with('/') {
            shown
        } else {
            format!("/{shown}")
        };
        let (drive, rest) = split_drive(&rooted);
        ArtifactLocation {
            uri: format!("file://{drive}{}", encode_uri(rest)),
            uri_base_id: None,
        }
    } else {
        ArtifactLocation {
            uri: encode_uri(&shown),
            uri_base_id: Some(SRCROOT),
        }
    }
}

fn physical(location: &Location, options: &RenderOptions, utf16: bool) -> PhysicalLocation {
    let path = options.display_path(&location.path);
    PhysicalLocation {
        region: location.span.as_ref().map(|span| region(span, &path, options, utf16)),
        artifact_location: artifact(options, &location.path),
    }
}

fn related<'a>(index: usize, related: &'a Related, options: &RenderOptions, utf16: bool) -> SarifLocation<'a> {
    SarifLocation {
        id: Some(index),
        physical_location: physical(&related.location, options, utf16),
        message: Some(Text { text: &related.message }),
    }
}

fn fix(fix: &Fix) -> SarifFix<'_> {
    SarifFix {
        title: &fix.title,
        safety: match fix.safety {
            FixSafety::Safe => "safe",
            _ => "unsafe",
        },
    }
}

/// 64-bit FNV-1a: a fixed algorithm, so fingerprints stay stable across toolchain releases.
fn fnv1a(parts: &[&str]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    let mut hash = OFFSET;
    for part in parts {
        for byte in part.bytes().chain(std::iter::once(0)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    hash
}

/// The hash of a diagnostic's identity, which its fingerprint is made of.
fn identity(diagnostic: &Diagnostic, path: &str) -> u64 {
    fnv1a(&[
        diagnostic.rule.as_str(),
        path,
        diagnostic.package.as_deref().unwrap_or_default(),
        diagnostic.dependency.as_deref().unwrap_or_default(),
        diagnostic.specifier.as_deref().unwrap_or_default(),
        &diagnostic.message,
    ])
}

/// Fingerprint of a diagnostic whose identity hashes to `hash`; `occurrence` (1-based) separates diagnostics that are
/// otherwise identical, such as the same message reported twice in one file.
fn fingerprint(hash: u64, occurrence: usize) -> String {
    format!("{hash:016x}:{occurrence}")
}

fn result<'a>(
    diagnostic: &'a Diagnostic,
    rules: &HashMap<&str, usize>,
    options: &RenderOptions,
    utf16: bool,
    seen: &mut HashMap<u64, usize>,
) -> SarifResult<'a> {
    let id = diagnostic.rule.as_str();
    let primary = physical(&diagnostic.location, options, utf16);
    let hash = identity(diagnostic, &primary.artifact_location.uri);
    // Counted by hash, as the formatted fingerprint with occurrence 0 was: the hash is the only part that varies.
    let occurrence = seen.entry(hash).and_modify(|count| *count += 1).or_insert(1);
    let fingerprint = fingerprint(hash, *occurrence);
    SarifResult {
        rule_id: id,
        rule_index: rules.get(id).copied(),
        level: level(diagnostic.severity),
        message: Text {
            text: &diagnostic.message,
        },
        locations: [SarifLocation {
            id: None,
            physical_location: primary,
            message: None,
        }],
        related_locations: diagnostic
            .related
            .iter()
            .enumerate()
            .map(|(index, r)| related(index, r, options, utf16))
            .collect(),
        partial_fingerprints: Fingerprints { skif: fingerprint },
        properties: ResultProperties {
            help: diagnostic.help.as_deref(),
            package: diagnostic.package.as_deref(),
            dependency: diagnostic.dependency.as_deref(),
            specifier: diagnostic.specifier.as_deref(),
            detail: diagnostic.detail.as_deref(),
            fixes: diagnostic.fixes.iter().map(fix).collect(),
        },
    }
}

/// Renders `report` as a SARIF 2.1.0 log with one run, in the report's own (deterministic) order.
///
/// Rules listed in the tool driver come from [`RenderOptions::with_rules`].
///
/// # Errors
///
/// Returns any error produced by writing to `out`.
pub fn render_sarif(report: &Report, options: &RenderOptions, out: &mut dyn Write) -> io::Result<()> {
    let (rules, rule_index) = rules(report, options);
    let utf16 = all_convertible(report, options);
    let mut seen = HashMap::default();
    let results = report
        .diagnostics
        .iter()
        .map(|d| result(d, &rule_index, options, utf16, &mut seen))
        .collect();
    let document = Document {
        schema: SCHEMA_URI,
        version: SARIF_VERSION,
        runs: [Run {
            tool: Tool {
                driver: Driver {
                    name: "skif",
                    version: options.tool_version.as_deref(),
                    information_uri: INFORMATION_URI,
                    rules,
                },
            },
            column_kind: if utf16 { "utf16CodeUnits" } else { "unicodeCodePoints" },
            results,
        }],
    };
    serde_json::to_writer_pretty(&mut *out, &document)?;
    writeln!(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_are_percent_encoded() {
        assert_eq!(encode_uri("packages/a b/package.json"), "packages/a%20b/package.json");
        assert_eq!(encode_uri("@scope/pkg#1/\u{e9}.json"), "%40scope/pkg%231/%C3%A9.json");
        assert_eq!(encode_uri("plain-name_1.0~x/y"), "plain-name_1.0~x/y");
    }

    #[test]
    fn paths_outside_the_workspace_become_file_uris() {
        let options = RenderOptions::default().with_root("/repo");
        let inside = artifact(&options, Path::new("/repo/a b/package.json"));
        assert_eq!(
            (inside.uri.as_str(), inside.uri_base_id),
            ("a%20b/package.json", Some(SRCROOT))
        );
        let outside = artifact(&options, Path::new("/elsewhere/x.json"));
        assert_eq!(
            (outside.uri.as_str(), outside.uri_base_id),
            ("file:///elsewhere/x.json", None)
        );
        let drive = artifact(&options, Path::new("C:/x/y.json"));
        assert_eq!(drive.uri, "file:///C:/x/y.json");
        let spaced = artifact(&options, Path::new("d:/my dir/y.json"));
        assert_eq!(spaced.uri, "file:///d:/my%20dir/y.json");
    }

    #[test]
    fn artifacts_are_relative_to_the_repository_top_level() {
        let options = RenderOptions::default().with_root("/repo/web").with_repo_root("/repo");
        let nested = artifact(&options, Path::new("packages/a/package.json"));
        assert_eq!(nested.uri, "web/packages/a/package.json");
    }

    #[test]
    fn fingerprint_is_stable() {
        assert_eq!(fnv1a(&[]), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(&["a"]), fnv1a(&["a"]));
        assert_ne!(fnv1a(&["ab", "c"]), fnv1a(&["a", "bc"]));
    }
}
