//! Rendering reports and errors, and getting the bytes to stdout or `--output`.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::Map;
use skif::render::{ErrorInfo, RenderOptions, render_human, render_json_error_info, render_json_with};
use skif::{FileSystem, Report};

use crate::args::Format;
use crate::error::{CliError, Kind};
use crate::fix::FixRun;

pub fn write_stdout(bytes: &[u8]) {
    let mut out = io::stdout().lock();
    if out.write_all(bytes).is_ok() {
        let _ = out.flush();
    }
}

/// Reports a failure: a JSON `error` document on stdout in JSON mode, `error: ...` on stderr otherwise.
pub fn emit_error(error: &CliError, format: Format) {
    match format {
        Format::Json => {
            let mut info = ErrorInfo::new(error.kind.as_str(), &error.message);
            if let Some(path) = &error.path {
                info = info.at(path, error.line, error.column);
            }
            let mut buffer = Vec::new();
            if render_json_error_info(&info, &mut buffer).is_ok() {
                write_stdout(&buffer);
            }
        }
        Format::Human => eprintln!("error: {error}"),
    }
}

/// Files whose text a code frame may need: every file a diagnostic or related location points into.
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

/// Renders `report` (and the fix run, if any) in `format`.
pub fn render(
    report: &Report,
    format: Format,
    color: bool,
    root: &Path,
    fs: &dyn FileSystem,
    fix: Option<&FixRun>,
) -> Result<Vec<u8>, CliError> {
    let mut options = RenderOptions::default()
        .with_color(color)
        .with_root(root)
        .with_tool_version(env!("CARGO_PKG_VERSION"));
    if format == Format::Human {
        for path in framed_files(report) {
            if let Ok(text) = fs.read_to_string(&root.join(&path)) {
                options = options.with_source(&path, text.to_string());
            }
        }
    }
    let mut buffer = Vec::new();
    write_report(report, format, &options, fix, &mut buffer)
        .map_err(|error| CliError::new(Kind::Io, format!("cannot render the report: {error}")))?;
    Ok(buffer)
}

fn write_report(
    report: &Report,
    format: Format,
    options: &RenderOptions,
    fix: Option<&FixRun>,
    out: &mut Vec<u8>,
) -> io::Result<()> {
    match format {
        Format::Human => {
            if let Some(run) = fix {
                run.write_human(out)?;
            }
            render_human(report, options, out)
        }
        Format::Json => {
            let mut extra = Map::new();
            if let Some(run) = fix {
                extra.insert("fix".to_owned(), run.to_json());
            }
            render_json_with(report, options, extra, out)
        }
    }
}

/// Sends the rendering to `--output` when given, stdout otherwise.
pub fn emit(bytes: &[u8], output: Option<&Path>) -> Result<(), CliError> {
    match output {
        Some(path) => fs::write(path, bytes).map_err(|error| CliError::io("cannot write", path, &error)),
        None => {
            write_stdout(bytes);
            Ok(())
        }
    }
}
