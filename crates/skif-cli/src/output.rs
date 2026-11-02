//! Rendering reports and errors, and getting the bytes to stdout or `--output`.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::Map;
use skif::render::{RenderOptions, render_github, render_human, render_sarif};
use skif::{FileSystem, Report, RuleMeta};
use skif_report::{ErrorInfo, escape_multiline, render_json_error_info_with, render_json_with};

use crate::args::Format;
use crate::error::{CliError, Kind};
use crate::fix::FixRun;
use crate::write::write_atomic;

/// Writes `bytes` to stdout; a failure (a closed pipe, a full disk) is an error, not something to swallow.
pub fn write_stdout(bytes: &[u8]) -> Result<(), CliError> {
    let mut out = io::stdout().lock();
    out.write_all(bytes)
        .and_then(|()| out.flush())
        .map_err(|error| CliError::new(Kind::Io, format!("cannot write to stdout: {error}")))
}

/// Writes `text` and a newline to stderr. A closed or broken stderr is ignored: `eprintln!` would panic on it, and
/// diagnostics about a run are not worth failing the run for.
pub fn stderr_line(text: &str) {
    let mut err = io::stderr().lock();
    let _ = err.write_all(text.as_bytes()).and_then(|()| err.write_all(b"\n"));
}

/// Reports a failure: a JSON `error` document on stdout in JSON mode, `error: ...` on stderr otherwise.
pub fn emit_error(error: &CliError, format: Format) {
    emit_error_with(error, format, Map::new());
}

/// Like [`emit_error`], with `extra` top-level members in the JSON document (the `fix` object of a completed fix).
pub fn emit_error_with(error: &CliError, format: Format, extra: Map<String, serde_json::Value>) {
    if format == Format::Json {
        let mut info = ErrorInfo::new(error.kind.as_str(), &error.message);
        if let Some(path) = &error.path {
            info = info.at(path, error.line, error.column);
        }
        let mut buffer = Vec::new();
        if render_json_error_info_with(&info, extra, &mut buffer).is_ok() && write_stdout(&buffer).is_ok() {
            return;
        }
    }
    stderr_line(&format!("error: {}", escape_multiline(&error.to_string())));
}

/// Files whose text a code frame or a UTF-16 column conversion may need: every file a diagnostic or related
/// location points into.
fn framed_files(report: &Report) -> BTreeSet<PathBuf> {
    let mut files = BTreeSet::new();
    for diagnostic in &report.diagnostics {
        if diagnostic.location.span.is_some() {
            files.insert(diagnostic.location.path.clone());
        }
        for related in &diagnostic.related {
            if related.location.span.is_some() {
                files.insert(related.location.path.clone());
            }
        }
    }
    files
}

/// How to render one report.
pub struct Rendering<'a> {
    pub format: Format,
    /// Append GitHub annotations to the human rendering.
    pub annotations: bool,
    pub color: bool,
    pub root: &'a Path,
    /// The repository top-level, for annotation paths.
    pub repo_root: Option<PathBuf>,
    /// Rules listed in the SARIF driver.
    pub rules: Vec<&'static RuleMeta>,
}

/// Renders `report` (and the fix run, if any) as `rendering` says.
pub fn render(
    report: &Report,
    rendering: &Rendering<'_>,
    fs: &dyn FileSystem,
    fix: Option<&FixRun>,
) -> Result<Vec<u8>, CliError> {
    let root = rendering.root;
    let mut options = RenderOptions::default()
        .with_color(rendering.color)
        .with_root(root)
        .with_tool_version(env!("CARGO_PKG_VERSION"));
    if let Some(repo_root) = &rendering.repo_root {
        options = options.with_repo_root(repo_root);
    }
    if matches!(rendering.format, Format::Human | Format::Sarif) {
        for path in framed_files(report) {
            if let Ok(text) = fs.read_to_string(&root.join(&path)) {
                options = options.with_source(&path, text.to_string());
            }
        }
    }
    if rendering.format == Format::Sarif {
        options = options.with_rules(rendering.rules.iter().copied());
    }
    let mut buffer = Vec::new();
    write_report(report, rendering, &options, fix, &mut buffer)
        .map_err(|error| CliError::new(Kind::Io, format!("cannot render the report: {error}")))?;
    Ok(buffer)
}

fn write_report(
    report: &Report,
    rendering: &Rendering<'_>,
    options: &RenderOptions,
    fix: Option<&FixRun>,
    out: &mut Vec<u8>,
) -> io::Result<()> {
    match rendering.format {
        Format::Human => {
            if let Some(run) = fix {
                run.write_human(out)?;
            }
            render_human(report, options, out)?;
            if rendering.annotations {
                render_github(report, options, out)?;
            }
            Ok(())
        }
        Format::Json => {
            let mut extra = Map::new();
            if let Some(run) = fix {
                extra.insert("fix".to_owned(), run.to_json());
            }
            render_json_with(report, options, extra, out)
        }
        Format::Sarif => render_sarif(report, options, out),
        Format::Github => render_github(report, options, out),
    }
}

/// Sends the rendering to `--output` (atomically) when given, stdout otherwise.
pub fn emit(bytes: &[u8], output: Option<&Path>) -> Result<(), CliError> {
    match output {
        Some(path) => write_atomic(path, bytes).map_err(|error| CliError::io("cannot write", path, &error)),
        None => write_stdout(bytes),
    }
}
