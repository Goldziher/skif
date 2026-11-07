//! The versioned JSON renderer.

use std::io::{self, Write};

use serde::Serialize;
use serde_json::{Map, Value};
use skif_core::{Diagnostic, FixSafety, Location, Report, Summary};

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
/// Returns an [`io::ErrorKind::InvalidInput`] error when an `extra` member is named `version`, `skif`, `summary` or
/// `diagnostics`, and any error produced by writing to `out`.
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
    number(document.key("version"), u64::from(SCHEMA_VERSION));
    string(
        document.key("skif"),
        options.tool_version.as_deref().unwrap_or(TOOL_VERSION),
    );
    write_summary(document.key("summary"), &report.summary);
    document.key("diagnostics").push(b'[');
    for (index, diagnostic) in report.diagnostics.iter().enumerate() {
        separate(&mut buffer, index == 0, 2);
        write_diagnostic(&mut buffer, diagnostic, options);
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
    for (key, value) in &extra {
        write_value(document.key(key), value)?;
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

    /// Starts the member `key` and returns the buffer to write its value to.
    fn key(&mut self, key: &str) -> &mut Vec<u8> {
        separate(self.out, self.empty, self.depth + 1);
        self.empty = false;
        string(self.out, key);
        self.out.extend_from_slice(b": ");
        self.out
    }

    /// Starts the member `key` with an object value one level deeper.
    fn object(&mut self, key: &str) -> Object<'_> {
        let depth = self.depth + 1;
        Object::open(self.key(key), depth)
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

/// Whether `serde_json` escapes something in `value`.
#[allow(
    clippy::needless_bitwise_bool,
    reason = "not short-circuiting, the scan compiles to vector instructions"
)]
fn needs_escape(value: &str) -> bool {
    value.bytes().fold(false, |found, byte| {
        found | (byte < 0x20) | (byte == b'"') | (byte == b'\\')
    })
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

fn optional_string(object: &mut Object<'_>, key: &str, value: Option<&str>) {
    if let Some(value) = value {
        string(object.key(key), value);
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
    count(object.key("errors"), summary.errors);
    count(object.key("warnings"), summary.warnings);
    count(object.key("infos"), summary.infos);
    count(object.key("fixable"), summary.fixable);
    count(object.key("packages"), summary.packages);
    object.close();
}

/// A diagnostic, an element of the top-level `diagnostics` array (depth 2).
fn write_diagnostic(out: &mut Vec<u8>, diagnostic: &Diagnostic, options: &RenderOptions) {
    let mut object = Object::open(out, 2);
    string(object.key("rule"), diagnostic.rule.as_str());
    // Writing to a `Vec` cannot fail, and a severity always serializes.
    let _ = serde_json::to_writer(&mut *object.key("severity"), &diagnostic.severity);
    string(object.key("message"), &diagnostic.message);
    optional_string(&mut object, "help", diagnostic.help.as_deref());
    write_location(object.object("location"), &diagnostic.location, options);
    let mut related = Array::open(object.key("related"), 3);
    for entry in &diagnostic.related {
        let mut item = Object::open(related.element(), 4);
        string(item.key("message"), &entry.message);
        write_location(item.object("location"), &entry.location, options);
        item.close();
    }
    related.close();
    optional_string(&mut object, "package", diagnostic.package.as_deref());
    optional_string(&mut object, "dependency", diagnostic.dependency.as_deref());
    optional_string(&mut object, "specifier", diagnostic.specifier.as_deref());
    optional_string(&mut object, "detail", diagnostic.detail.as_deref());
    let mut fixes = Array::open(object.key("fixes"), 3);
    for fix in &diagnostic.fixes {
        let mut item = Object::open(fixes.element(), 4);
        string(item.key("title"), &fix.title);
        string(item.key("safety"), safety(fix.safety));
        item.close();
    }
    fixes.close();
    object.close();
}

fn write_location(mut object: Object<'_>, location: &Location, options: &RenderOptions) {
    string(object.key("path"), &options.display_path(&location.path));
    if let Some(span) = &location.span {
        let mut fields = object.object("span");
        for (key, value) in [
            ("start", span.start),
            ("end", span.end),
            ("start-line", span.start_line),
            ("start-col", span.start_col),
            ("end-line", span.end_line),
            ("end-col", span.end_col),
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
    use skif_core::{Fix, FixOp, Related, RuleId, Severity, Span};

    use super::*;

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

    /// What `render_json_with` wrote before it was written by hand.
    fn reference(report: &Report, options: &RenderOptions, extra: Map<String, Value>) -> Vec<u8> {
        let document = Document {
            version: SCHEMA_VERSION,
            skif: options.tool_version.as_deref().unwrap_or(TOOL_VERSION),
            summary: summary(&report.summary),
            diagnostics: report.diagnostics.iter().map(|d| diagnostic(d, options)).collect(),
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
        let info = Diagnostic::new(
            RuleId::from_static("sorted-keys"),
            Severity::Info,
            "keys",
            Location::new("C:\\repo\\a.json", None),
        );
        Report::new(vec![full, bare, info], 3)
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
