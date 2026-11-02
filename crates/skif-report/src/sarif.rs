//! The SARIF 2.1.0 renderer.
//!
//! Columns are declared as `unicodeCodePoints` so skif's scalar-value columns are emitted unchanged. Fixes are
//! operations on structured files rather than text replacements, so they are listed under `properties.fixes`
//! instead of the SARIF `fixes` array, which requires concrete artifact changes.

use std::io::{self, Write};

use serde::Serialize;
use skif_core::{
    Category, Diagnostic, Fix, FixSafety, Fixability, Location, Related, Report, RuleMeta, Severity, Span,
};

use crate::options::RenderOptions;

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
    uri_base_id: &'static str,
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

/// Rules from the options, followed by any rule that only appears in results, in order of first appearance.
fn rules<'a>(report: &'a Report, options: &'a RenderOptions) -> Vec<SarifRule<'a>> {
    let mut rules: Vec<SarifRule<'a>> = options.rules().iter().copied().map(rule_entry).collect();
    for diagnostic in &report.diagnostics {
        let id = diagnostic.rule.as_str();
        if !rules.iter().any(|rule| rule.id == id) {
            rules.push(SarifRule {
                id,
                short_description: None,
                help: None,
                default_configuration: None,
                properties: None,
            });
        }
    }
    rules
}

const fn region(span: &Span) -> Region {
    Region {
        start_line: span.start_line,
        start_column: span.start_col,
        end_line: span.end_line,
        end_column: span.end_col,
    }
}

fn physical(location: &Location, options: &RenderOptions) -> PhysicalLocation {
    PhysicalLocation {
        artifact_location: ArtifactLocation {
            uri: options.display_path(&location.path),
            uri_base_id: SRCROOT,
        },
        region: location.span.as_ref().map(region),
    }
}

fn related<'a>(index: usize, related: &'a Related, options: &RenderOptions) -> SarifLocation<'a> {
    SarifLocation {
        id: Some(index),
        physical_location: physical(&related.location, options),
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

fn fingerprint(diagnostic: &Diagnostic, path: &str) -> String {
    let hash = fnv1a(&[
        diagnostic.rule.as_str(),
        path,
        diagnostic.package.as_deref().unwrap_or_default(),
        diagnostic.dependency.as_deref().unwrap_or_default(),
        &diagnostic.message,
    ]);
    format!("{hash:016x}")
}

fn result<'a>(diagnostic: &'a Diagnostic, rules: &[SarifRule<'_>], options: &RenderOptions) -> SarifResult<'a> {
    let id = diagnostic.rule.as_str();
    let primary = physical(&diagnostic.location, options);
    let fingerprint = fingerprint(diagnostic, &primary.artifact_location.uri);
    SarifResult {
        rule_id: id,
        rule_index: rules.iter().position(|rule| rule.id == id),
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
            .map(|(index, r)| related(index, r, options))
            .collect(),
        partial_fingerprints: Fingerprints { skif: fingerprint },
        properties: ResultProperties {
            help: diagnostic.help.as_deref(),
            package: diagnostic.package.as_deref(),
            dependency: diagnostic.dependency.as_deref(),
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
    let rules = rules(report, options);
    let results = report.diagnostics.iter().map(|d| result(d, &rules, options)).collect();
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
            column_kind: "unicodeCodePoints",
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
    fn fingerprint_is_stable() {
        assert_eq!(fnv1a(&[]), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(&["a"]), fnv1a(&["a"]));
        assert_ne!(fnv1a(&["ab", "c"]), fnv1a(&["a", "bc"]));
    }
}
