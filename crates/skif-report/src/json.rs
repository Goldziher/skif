//! The versioned JSON renderer.

use std::io::{self, Write};
use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value};
use skif_core::{Diagnostic, FixSafety, IncompletePackage, Location, Report, Severity, Summary};

use crate::options::RenderOptions;

/// A member name of the document as [`Object::key`] takes it: quoted and followed by the colon.
macro_rules! quoted {
    ($name:literal) => {
        concat!("\"", $name, "\": ")
    };
}

/// Current JSON schema version; bumped only on breaking changes.
pub const SCHEMA_VERSION: u32 = 1;

/// Top-level members the document owns; `extra` members may not reuse them.
const RESERVED: [&str; 5] = ["version", "skif", "summary", "diagnostics", "incomplete-packages"];

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
/// Returns an [`io::ErrorKind::InvalidInput`] error when an `extra` member is named `version`, `skif`, `summary`,
/// `diagnostics` or `incomplete-packages`, and any error produced by writing to `out`.
#[allow(
    clippy::needless_pass_by_value,
    reason = "a public signature: `extra` was consumed by the serializer this replaced"
)]
pub fn render_json_with(
    report: &Report,
    options: &RenderOptions,
    extra: Map<String, Value>,
    out: &mut dyn Write,
) -> io::Result<()> {
    ensure_unreserved(&extra, &RESERVED)?;
    let mut buffer = Vec::with_capacity(CHUNK + CHUNK / 4);
    let mut document = Object::open(&mut buffer, 0);
    number(document.key(quoted!("version")), u64::from(SCHEMA_VERSION));
    string(
        document.key(quoted!("skif")),
        options.tool_version.as_deref().unwrap_or(TOOL_VERSION),
    );
    write_summary(document.key(quoted!("summary")), &report.summary);
    document.key(quoted!("diagnostics")).push(b'[');
    let mut paths = Paths::default();
    for (index, diagnostic) in report.diagnostics.iter().enumerate() {
        separate(&mut buffer, index == 0, 2);
        write_diagnostic(&mut buffer, diagnostic, options, &mut paths);
        if buffer.len() >= CHUNK {
            out.write_all(&buffer)?;
            buffer.clear();
        }
    }
    close(&mut buffer, report.diagnostics.is_empty(), 1, b']');
    let mut document = Object {
        out: &mut buffer,
        depth: 0,
        empty: false,
    };
    if !report.incomplete_packages.is_empty() {
        let mut packages = Array::open(document.key(quoted!("incomplete-packages")), 1);
        for package in &report.incomplete_packages {
            write_incomplete_package(packages.element(), package, options);
        }
        packages.close();
    }
    for (key, value) in &extra {
        write_value(document.any_key(key), value)?;
    }
    document.close();
    buffer.push(b'\n');
    out.write_all(&buffer)
}

/// How much of the document is buffered before it is handed to the writer: large enough that writes are few, small
/// enough to stay in cache instead of growing a second copy of a document that can reach tens of megabytes.
const CHUNK: usize = 64 * 1024;

// The document is written by hand rather than through `serde_json::to_writer_pretty`: on a report with tens of
// thousands of diagnostics the generic serializer was the largest part of a run. The bytes are exactly the ones
// serde_json's `PrettyFormatter` writes for the same document (two-space indent, `[]` for an empty array, strings
// escaped by serde_json itself); `tests::matches_serde_json` holds the two to that.

/// An object being written at `depth` (the indentation of its braces); its members go one level deeper.
struct Object<'a> {
    out: &'a mut Vec<u8>,
    depth: usize,
    empty: bool,
}

impl<'a> Object<'a> {
    fn open(out: &'a mut Vec<u8>, depth: usize) -> Self {
        out.push(b'{');
        Self {
            out,
            depth,
            empty: true,
        }
    }

    /// Starts a member and returns the buffer to write its value to. `quoted` is one of this module's names as
    /// [`quoted!`] writes it: quoted and followed by the colon, with nothing to escape.
    fn key(&mut self, quoted: &'static str) -> &mut Vec<u8> {
        debug_assert!(
            quoted.starts_with('"') && quoted.ends_with("\": "),
            "{quoted} is not quoted"
        );
        separate(self.out, self.empty, self.depth + 1);
        self.empty = false;
        self.out.extend_from_slice(quoted.as_bytes());
        self.out
    }

    /// Starts the member `key`, any name, and returns the buffer to write its value to.
    fn any_key(&mut self, key: &str) -> &mut Vec<u8> {
        separate(self.out, self.empty, self.depth + 1);
        self.empty = false;
        string(self.out, key);
        self.out.extend_from_slice(b": ");
        self.out
    }

    /// Starts a member (`quoted` as for [`key`](Self::key)) with an object value one level deeper.
    fn object(&mut self, quoted: &'static str) -> Object<'_> {
        let depth = self.depth + 1;
        Object::open(self.key(quoted), depth)
    }

    fn close(self) {
        close(self.out, self.empty, self.depth, b'}');
    }
}

/// An array being written at `depth`; its elements go one level deeper.
struct Array<'a> {
    out: &'a mut Vec<u8>,
    depth: usize,
    empty: bool,
}

impl<'a> Array<'a> {
    fn open(out: &'a mut Vec<u8>, depth: usize) -> Self {
        out.push(b'[');
        Self {
            out,
            depth,
            empty: true,
        }
    }

    /// Starts an element and returns the buffer to write it to.
    fn element(&mut self) -> &mut Vec<u8> {
        separate(self.out, self.empty, self.depth + 1);
        self.empty = false;
        self.out
    }

    fn close(self) {
        close(self.out, self.empty, self.depth, b']');
    }
}

/// A comma, a newline and the indentation of the deepest level the document reaches, sliced for shallower ones.
const BREAK: &[u8; 18] = b",\n                ";

/// Writes `,\n` (only `\n` when `first`) and the indentation of `depth`.
fn separate(out: &mut Vec<u8>, first: bool, depth: usize) {
    line_break(out, usize::from(first), depth);
}

fn close(out: &mut Vec<u8>, empty: bool, depth: usize, bracket: u8) {
    if !empty {
        line_break(out, 1, depth);
    }
    out.push(bracket);
}

/// `BREAK` from `from` with the indentation of `depth`; deeper levels than it holds are padded.
fn line_break(out: &mut Vec<u8>, from: usize, depth: usize) {
    let width = 2 + 2 * depth;
    if width <= BREAK.len() {
        out.extend_from_slice(&BREAK[from..width]);
    } else {
        out.extend_from_slice(&BREAK[from..]);
        for _ in BREAK.len()..width {
            out.push(b' ');
        }
    }
}

/// Whether `serde_json` escapes something in `value`: a control character, `"` or `\\`. Eight bytes at a time.
fn needs_escape(value: &str) -> bool {
    const ONES: u64 = u64::from_ne_bytes([0x01; 8]);
    const HIGH: u64 = u64::from_ne_bytes([0x80; 8]);
    // Whether some byte of `word` is below `limit` (at most 0x80); `limit` 1 finds a zero byte.
    let below = |word: u64, limit: u8| word.wrapping_sub(ONES * u64::from(limit)) & !word & HIGH != 0;
    let (words, rest) = value.as_bytes().as_chunks::<8>();
    words.iter().any(|&bytes| {
        let word = u64::from_ne_bytes(bytes);
        below(word, 0x20) || below(word ^ (ONES * u64::from(b'"')), 1) || below(word ^ (ONES * u64::from(b'\\')), 1)
    }) || rest.iter().any(|&byte| byte < 0x20 || byte == b'"' || byte == b'\\')
}

/// A JSON string. Most strings need no escaping and are copied as they are; any other goes through `serde_json`.
fn string(out: &mut Vec<u8>, value: &str) {
    if needs_escape(value) {
        // Writing to a `Vec` cannot fail, and a `str` always serializes.
        let _ = serde_json::to_writer(&mut *out, value);
    } else {
        out.push(b'"');
        out.extend_from_slice(value.as_bytes());
        out.push(b'"');
    }
}

fn optional_string(object: &mut Object<'_>, quoted: &'static str, value: Option<&str>) {
    if let Some(value) = value {
        string(object.key(quoted), value);
    }
}

fn number(out: &mut Vec<u8>, mut value: u64) {
    let mut digits = [0_u8; 20];
    let mut start = digits.len();
    loop {
        start -= 1;
        // A single decimal digit always fits.
        digits[start] = b'0' + u8::try_from(value % 10).unwrap_or(0);
        value /= 10;
        if value == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[start..]);
}

fn count(out: &mut Vec<u8>, value: usize) {
    number(out, u64::try_from(value).unwrap_or(u64::MAX));
}

fn write_summary(out: &mut Vec<u8>, summary: &Summary) {
    let mut object = Object::open(out, 1);
    count(object.key(quoted!("errors")), summary.errors);
    count(object.key(quoted!("warnings")), summary.warnings);
    count(object.key(quoted!("infos")), summary.infos);
    count(object.key(quoted!("fixable")), summary.fixable);
    count(object.key(quoted!("packages")), summary.packages);
    object.close();
}

/// The rendered `path` of the last primary and the last related location written. A report is sorted by path, so
/// the diagnostics of one file follow each other and their path is displayed and escaped once, not once each.
#[derive(Default)]
struct Paths<'r> {
    primary: RenderedPath<'r>,
    related: RenderedPath<'r>,
}

#[derive(Default)]
struct RenderedPath<'r> {
    path: Option<&'r Path>,
    json: Vec<u8>,
}

impl<'r> RenderedPath<'r> {
    /// Writes `path` as a JSON string, as `string(out, &options.display_path(path))` does.
    fn write(&mut self, out: &mut Vec<u8>, path: &'r Path, options: &RenderOptions) {
        // Equal bytes display the same: the display depends on nothing else than the path and the options.
        if self.path.is_none_or(|last| last.as_os_str() != path.as_os_str()) {
            self.json.clear();
            string(&mut self.json, &options.display_path(path));
            self.path = Some(path);
        }
        out.extend_from_slice(&self.json);
    }
}

/// A diagnostic, an element of the top-level `diagnostics` array (depth 2).
fn write_diagnostic<'r>(out: &mut Vec<u8>, diagnostic: &'r Diagnostic, options: &RenderOptions, paths: &mut Paths<'r>) {
    let mut object = Object::open(out, 2);
    string(object.key(quoted!("rule")), diagnostic.rule.as_str());
    let severity = object.key(quoted!("severity"));
    match diagnostic.severity {
        Severity::Error => string(severity, "error"),
        Severity::Warning => string(severity, "warning"),
        Severity::Info => string(severity, "info"),
        // Writing to a `Vec` cannot fail, and a severity always serializes.
        _ => drop(serde_json::to_writer(&mut *severity, &diagnostic.severity)),
    }
    string(object.key(quoted!("message")), &diagnostic.message);
    optional_string(&mut object, quoted!("help"), diagnostic.help.as_deref());
    write_location(object.object(quoted!("location")), &diagnostic.location, |out| {
        paths.primary.write(out, &diagnostic.location.path, options);
    });
    let mut related = Array::open(object.key(quoted!("related")), 3);
    for entry in &diagnostic.related {
        let mut item = Object::open(related.element(), 4);
        string(item.key(quoted!("message")), &entry.message);
        write_location(item.object(quoted!("location")), &entry.location, |out| {
            paths.related.write(out, &entry.location.path, options);
        });
        item.close();
    }
    related.close();
    optional_string(&mut object, quoted!("package"), diagnostic.package.as_deref());
    optional_string(&mut object, quoted!("dependency"), diagnostic.dependency.as_deref());
    optional_string(&mut object, quoted!("specifier"), diagnostic.specifier.as_deref());
    optional_string(&mut object, quoted!("detail"), diagnostic.detail.as_deref());
    let mut fixes = Array::open(object.key(quoted!("fixes")), 3);
    for fix in &diagnostic.fixes {
        let mut item = Object::open(fixes.element(), 4);
        string(item.key(quoted!("title")), &fix.title);
        string(item.key(quoted!("safety")), safety(fix.safety));
        item.close();
    }
    fixes.close();
    object.close();
}

/// A package whose files and dependencies were not judged, an element of `incomplete-packages` (depth 2).
fn write_incomplete_package(out: &mut Vec<u8>, package: &IncompletePackage, options: &RenderOptions) {
    let mut object = Object::open(out, 2);
    string(object.key(quoted!("name")), &package.name);
    let dir = options.display_path(&package.dir);
    string(object.key(quoted!("dir")), if dir.is_empty() { "." } else { &dir });
    let mut reasons = Array::open(object.key(quoted!("reasons")), 3);
    for reason in &package.reasons {
        let mut item = Object::open(reasons.element(), 4);
        // Writing to a `Vec` cannot fail, and a cause always serializes (a kebab-case name).
        drop(serde_json::to_writer(&mut *item.key(quoted!("cause")), &reason.cause));
        optional_string(&mut item, quoted!("plugin"), reason.plugin.as_deref());
        if let Some(file) = &reason.file {
            string(item.key(quoted!("file")), &options.display_path(file));
        }
        item.close();
    }
    reasons.close();
    object.close();
}

/// A location object; `path` writes the value of its `path` member.
fn write_location(mut object: Object<'_>, location: &Location, path: impl FnOnce(&mut Vec<u8>)) {
    path(object.key(quoted!("path")));
    if let Some(span) = &location.span {
        let mut fields = object.object(quoted!("span"));
        for (key, value) in [
            (quoted!("start"), span.start),
            (quoted!("end"), span.end),
            (quoted!("start-line"), span.start_line),
            (quoted!("start-col"), span.start_col),
            (quoted!("end-line"), span.end_line),
            (quoted!("end-col"), span.end_col),
        ] {
            number(fields.key(key), u64::from(value));
        }
        fields.close();
    }
    object.close();
}

const fn safety(safety: FixSafety) -> &'static str {
    match safety {
        FixSafety::Safe => "safe",
        _ => "unsafe",
    }
}

/// An `extra` member's value, a member of the top-level object (depth 1). `serde_json` pretty-prints it; a newline in
/// its output is always structural (strings escape theirs), so indenting after each one nests it at depth 1.
fn write_value(out: &mut Vec<u8>, value: &Value) -> io::Result<()> {
    let pretty = serde_json::to_vec_pretty(value)?;
    for (index, line) in pretty.split(|&byte| byte == b'\n').enumerate() {
        if index > 0 {
            out.extend_from_slice(b"\n  ");
        }
        out.extend_from_slice(line);
    }
    Ok(())
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    //! The hand-written document against the derive-based serialization it replaced.

    use serde_json::json;
    use skif_core::{Fix, FixOp, IncompleteCause, IncompleteEntries, Related, RuleId, Severity, Span};

    use super::*;

    #[derive(Serialize)]
    struct Document<'a> {
        version: u32,
        skif: &'a str,
        summary: JsonSummary,
        diagnostics: Vec<JsonDiagnostic<'a>>,
        #[serde(rename = "incomplete-packages", skip_serializing_if = "Vec::is_empty")]
        incomplete_packages: Vec<JsonIncompletePackage<'a>>,
        #[serde(flatten)]
        extra: Map<String, Value>,
    }

    #[derive(Serialize)]
    struct JsonIncompletePackage<'a> {
        name: &'a str,
        dir: String,
        reasons: Vec<JsonIncompleteReason<'a>>,
    }

    #[derive(Serialize)]
    struct JsonIncompleteReason<'a> {
        cause: IncompleteCause,
        #[serde(skip_serializing_if = "Option::is_none")]
        plugin: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        file: Option<String>,
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
        specifier: Option<&'a str>,
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

    fn location(location: &Location, options: &RenderOptions) -> JsonLocation {
        JsonLocation {
            path: options.display_path(&location.path).into_owned(),
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
            specifier: diagnostic.specifier.as_deref(),
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

    fn incomplete_package<'a>(package: &'a IncompletePackage, options: &RenderOptions) -> JsonIncompletePackage<'a> {
        let dir = options.display_path(&package.dir).into_owned();
        JsonIncompletePackage {
            name: &package.name,
            dir: if dir.is_empty() { ".".to_owned() } else { dir },
            reasons: package
                .reasons
                .iter()
                .map(|reason| JsonIncompleteReason {
                    cause: reason.cause,
                    plugin: reason.plugin.as_deref(),
                    file: reason
                        .file
                        .as_deref()
                        .map(|file| options.display_path(file).into_owned()),
                })
                .collect(),
        }
    }

    /// What `render_json_with` wrote before it was written by hand.
    fn reference(report: &Report, options: &RenderOptions, extra: Map<String, Value>) -> Vec<u8> {
        let document = Document {
            version: SCHEMA_VERSION,
            skif: options.tool_version.as_deref().unwrap_or(TOOL_VERSION),
            summary: summary(&report.summary),
            diagnostics: report.diagnostics.iter().map(|d| diagnostic(d, options)).collect(),
            incomplete_packages: report
                .incomplete_packages
                .iter()
                .map(|package| incomplete_package(package, options))
                .collect(),
            extra,
        };
        let mut out = Vec::new();
        write_document(&document, &mut out).unwrap();
        out
    }

    fn report() -> Report {
        let span = Span::new(10, 18, 2, 3, 2, 11);
        let mut full = Diagnostic::new(
            RuleId::from_static("version-mismatch"),
            Severity::Error,
            "`a@^1` in `dependencies` of \"web\" \\ needs\nescaping\t\u{1}, and réact 日本語 does not",
            Location::new("/repo/packages/web/package.json", Some(span)),
        );
        full.help = Some("use `^2`".to_owned());
        full.package = Some("web".to_owned());
        full.dependency = Some("a".to_owned());
        full.specifier = Some("^1".to_owned());
        full.detail = Some("DiffersToHighestOrLowestSemver".to_owned());
        full.related = vec![
            Related::new("holds \"^2\"", Location::new("packages/api/package.json", Some(span))),
            Related::new("no span", Location::new("./skif.toml", None)),
        ];
        full.fixes = vec![
            Fix::new("Set `a` to `^2`", FixSafety::Unsafe, vec![FixOp::delete_file("x")]),
            Fix::new("Remove", FixSafety::Safe, Vec::new()),
        ];
        let bare = Diagnostic::new(
            RuleId::from_static("empty-dependencies"),
            Severity::Warning,
            "",
            Location::new("package.json", None),
        );
        // serde_json leaves U+2028, characters outside the BMP and DEL unescaped; the hand-written path must too.
        let mut info = Diagnostic::new(
            RuleId::from_static("sorted-keys"),
            Severity::Info,
            "keys\u{2028}line separator, \u{1f600} outside the BMP, \u{7f} delete",
            Location::new("C:\\repo\\a.json", None),
        );
        info.help = Some("\u{7f}\u{2028}\u{1f600}".to_owned());
        let reason = |cause, plugin: Option<&str>, file: Option<&str>| IncompleteEntries {
            cause,
            plugin: plugin.map(str::to_owned),
            file: file.map(Into::into),
        };
        Report::new(vec![full, bare, info], 3).with_incomplete_packages(vec![
            IncompletePackage::new(
                "web".to_owned(),
                "/repo/packages/web".into(),
                vec![
                    reason(
                        IncompleteCause::UnknownSpread,
                        Some("vite"),
                        Some("/repo/packages/web/vite.config.ts"),
                    ),
                    reason(IncompleteCause::UnmappedPublished, None, None),
                ],
            ),
            IncompletePackage::new(
                "root \"q\"".to_owned(),
                "".into(),
                vec![reason(IncompleteCause::Unspecified, None, None)],
            ),
        ])
    }

    #[test]
    fn escapes_are_found_at_every_position() {
        for special in ['"', '\\', '\n', '\0', '\u{1f}'] {
            for length in 1..20 {
                for position in 0..length {
                    let text: String = (0..length)
                        .map(|index| if index == position { special } else { 'a' })
                        .collect();
                    assert!(needs_escape(&text), "{text:?}");
                }
            }
        }
        for plain in [
            "",
            "a",
            " !#$%&'()*+,-./0123456789:;<=>?@[]^_`{|}~\u{7f}",
            "réact 日本語 ✓ and more text",
        ] {
            assert!(!needs_escape(plain), "{plain:?}");
        }
    }

    #[test]
    fn indentation_deeper_than_the_table_is_padded() {
        let mut out = Vec::new();
        line_break(&mut out, 1, 10);
        assert_eq!(out, [b"\n".as_slice(), &[b' '; 20]].concat());
    }

    #[test]
    fn matches_serde_json() {
        let mut extra = Map::new();
        extra.insert(
            "roots".to_owned(),
            json!([{"path": ".", "packages": [], "summary": {"errors": 1}}, {}, "a\nb", 1.5, null, true]),
        );
        extra.insert("errors".to_owned(), json!([]));
        extra.insert(
            "fix".to_owned(),
            json!({"nested": {"deeper": [[], [1, 2], {}]}, "text": "q\"\\"}),
        );
        extra.insert("count".to_owned(), json!(7));
        let cases = [
            (report(), RenderOptions::default(), Map::new()),
            (
                report(),
                RenderOptions::default()
                    .with_root("/repo")
                    .with_tool_version("9.9.9-\"rc\""),
                extra,
            ),
            (Report::new(Vec::new(), 0), RenderOptions::default(), Map::new()),
        ];
        for (report, options, extra) in cases {
            let mut written = Vec::new();
            render_json_with(&report, &options, extra.clone(), &mut written).unwrap();
            let expected = reference(&report, &options, extra);
            assert_eq!(
                String::from_utf8(written).unwrap(),
                String::from_utf8(expected).unwrap()
            );
        }
    }
}
