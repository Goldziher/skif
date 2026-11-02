//! The versioned JSON renderer.

use std::io::{self, Write};

use serde::Serialize;
use serde_json::{Map, Value};
use skif_core::{Diagnostic, Fix, FixSafety, Location, Related, Report, Severity, Span, Summary};

use crate::options::RenderOptions;

/// Current JSON schema version; bumped only on breaking changes.
pub const SCHEMA_VERSION: u32 = 1;

/// Top-level members the document owns; `extra` members may not reuse them.
const RESERVED: [&str; 4] = ["version", "skif", "summary", "diagnostics"];

/// Top-level members of an error document that `extra` members may not reuse.
const RESERVED_ERROR: [&str; 3] = ["version", "skif", "error"];

fn ensure_unreserved(extra: &Map<String, Value>, reserved: &[&str]) -> io::Result<()> {
    match extra.keys().find(|key| reserved.contains(&key.as_str())) {
        Some(key) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("extra member `{key}` collides with a reserved key of the document"),
        )),
        None => Ok(()),
    }
}

/// The version reported as `skif` when the caller does not supply one.
const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Serialize)]
struct Document<'a> {
    version: u32,
    skif: &'a str,
    summary: JsonSummary,
    diagnostics: Vec<JsonDiagnostic<'a>>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Serialize)]
struct JsonSummary {
    errors: usize,
    warnings: usize,
    infos: usize,
    fixable: usize,
    packages: usize,
}

#[derive(Serialize)]
struct JsonDiagnostic<'a> {
    rule: &'a str,
    severity: Severity,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    help: Option<&'a str>,
    location: JsonLocation,
    related: Vec<JsonRelated<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    package: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dependency: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
    fixes: Vec<JsonFix<'a>>,
}

#[derive(Serialize)]
struct JsonLocation {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    span: Option<JsonSpan>,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct JsonSpan {
    start: u32,
    end: u32,
    start_line: u32,
    start_col: u32,
    end_line: u32,
    end_col: u32,
}

#[derive(Serialize)]
struct JsonRelated<'a> {
    message: &'a str,
    location: JsonLocation,
}

#[derive(Serialize)]
struct JsonFix<'a> {
    title: &'a str,
    safety: &'static str,
}

#[derive(Serialize)]
struct ErrorDocument<'a> {
    version: u32,
    skif: &'static str,
    error: ErrorBody<'a>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<&'a str>,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    column: Option<u32>,
}

/// A failure to run, as reported in the `error` document.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct ErrorInfo<'a> {
    /// Machine-readable category, such as `config` or `workspace`.
    pub kind: &'a str,
    /// Human-readable description.
    pub message: &'a str,
    /// File the failure concerns, when known.
    pub path: Option<&'a str>,
    /// 1-based line, when known.
    pub line: Option<u32>,
    /// 1-based column, when known.
    pub column: Option<u32>,
}

impl<'a> ErrorInfo<'a> {
    /// An error of `kind` with `message` and no location.
    #[must_use]
    pub const fn new(kind: &'a str, message: &'a str) -> Self {
        Self {
            kind,
            message,
            path: None,
            line: None,
            column: None,
        }
    }

    /// Attaches the file and, when known, the position the failure concerns.
    #[must_use]
    pub const fn at(mut self, path: &'a str, line: Option<u32>, column: Option<u32>) -> Self {
        self.path = Some(path);
        self.line = line;
        self.column = column;
        self
    }
}

fn location(location: &Location, options: &RenderOptions) -> JsonLocation {
    JsonLocation {
        path: options.display_path(&location.path),
        span: location.span.as_ref().map(span),
    }
}

const fn span(span: &Span) -> JsonSpan {
    JsonSpan {
        start: span.start,
        end: span.end,
        start_line: span.start_line,
        start_col: span.start_col,
        end_line: span.end_line,
        end_col: span.end_col,
    }
}

fn related<'a>(related: &'a Related, options: &RenderOptions) -> JsonRelated<'a> {
    JsonRelated {
        message: &related.message,
        location: location(&related.location, options),
    }
}

fn fix(fix: &Fix) -> JsonFix<'_> {
    JsonFix {
        title: &fix.title,
        safety: match fix.safety {
            FixSafety::Safe => "safe",
            _ => "unsafe",
        },
    }
}

fn diagnostic<'a>(diagnostic: &'a Diagnostic, options: &RenderOptions) -> JsonDiagnostic<'a> {
    JsonDiagnostic {
        rule: diagnostic.rule.as_str(),
        severity: diagnostic.severity,
        message: &diagnostic.message,
        help: diagnostic.help.as_deref(),
        location: location(&diagnostic.location, options),
        related: diagnostic.related.iter().map(|r| related(r, options)).collect(),
        package: diagnostic.package.as_deref(),
        dependency: diagnostic.dependency.as_deref(),
        detail: diagnostic.detail.as_deref(),
        fixes: diagnostic.fixes.iter().map(fix).collect(),
    }
}

const fn summary(summary: &Summary) -> JsonSummary {
    JsonSummary {
        errors: summary.errors,
        warnings: summary.warnings,
        infos: summary.infos,
        fixable: summary.fixable,
        packages: summary.packages,
    }
}

fn write_document(document: &impl Serialize, out: &mut dyn Write) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *out, document)?;
    writeln!(out)
}

/// Renders `report` as the versioned JSON document, in the report's own (deterministic) order.
///
/// # Errors
///
/// Returns any error produced by writing to `out`.
pub fn render_json(report: &Report, options: &RenderOptions, out: &mut dyn Write) -> io::Result<()> {
    render_json_with(report, options, Map::new(), out)
}

/// Renders `report` like [`render_json`] and adds the top-level `extra` members (such as the `fix` and `install`
/// objects of a fix run) after `diagnostics`.
///
/// # Errors
///
/// Returns an [`io::ErrorKind::InvalidInput`] error when an `extra` member is named `version`, `skif`, `summary` or
/// `diagnostics`, and any error produced by writing to `out`.
pub fn render_json_with(
    report: &Report,
    options: &RenderOptions,
    extra: Map<String, Value>,
    out: &mut dyn Write,
) -> io::Result<()> {
    ensure_unreserved(&extra, &RESERVED)?;
    let document = Document {
        version: SCHEMA_VERSION,
        skif: options.tool_version.as_deref().unwrap_or(TOOL_VERSION),
        summary: summary(&report.summary),
        diagnostics: report.diagnostics.iter().map(|d| diagnostic(d, options)).collect(),
        extra,
    };
    write_document(&document, out)
}

/// Renders a failure as `{"version": 1, "error": {"message": ...}}`.
///
/// # Errors
///
/// Returns any error produced by writing to `out`.
pub fn render_json_error(message: &str, out: &mut dyn Write) -> io::Result<()> {
    write_document(
        &ErrorDocument {
            version: SCHEMA_VERSION,
            skif: TOOL_VERSION,
            error: ErrorBody {
                kind: None,
                message,
                path: None,
                line: None,
                column: None,
            },
            extra: Map::new(),
        },
        out,
    )
}

/// Renders a failure as `{"version": 1, "error": {"kind", "message", "path"?, "line"?, "column"?}}`.
///
/// # Errors
///
/// Returns any error produced by writing to `out`.
pub fn render_json_error_info(info: &ErrorInfo<'_>, out: &mut dyn Write) -> io::Result<()> {
    render_json_error_info_with(info, Map::new(), out)
}

/// Renders a failure like [`render_json_error_info`] and adds the top-level `extra` members (such as the `fix`
/// object of a fix run that completed before the failure).
///
/// # Errors
///
/// Returns an [`io::ErrorKind::InvalidInput`] error when an `extra` member is named `version`, `skif` or `error`, and
/// any error produced by writing to `out`.
pub fn render_json_error_info_with(
    info: &ErrorInfo<'_>,
    extra: Map<String, Value>,
    out: &mut dyn Write,
) -> io::Result<()> {
    ensure_unreserved(&extra, &RESERVED_ERROR)?;
    write_document(
        &ErrorDocument {
            version: SCHEMA_VERSION,
            skif: TOOL_VERSION,
            error: ErrorBody {
                kind: Some(info.kind),
                message: info.message,
                path: info.path,
                line: info.line,
                column: info.column,
            },
            extra,
        },
        out,
    )
}
