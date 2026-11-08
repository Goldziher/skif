//! Rendering reports and errors, and getting the bytes to stdout or `--output`.

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use skif::render::{RenderOptions, render_github, render_human, render_sarif};
use skif::{FileSystem, Report, RuleMeta};
use skif_report::{
    CODEOWNERS_LOCATIONS, CodeOwners, ErrorInfo, escape_controls, escape_multiline, render_codeclimate,
    render_codeowners, render_compact, render_disclosure, render_json_error_info_with, render_json_with,
    render_markdown,
};

use crate::args::Format;
use crate::error::{CliError, Kind};
use crate::fix::{self, FixRun, RootFix};
use crate::roots;
use crate::write::write_atomic;

/// Writes `bytes` to stdout; a failure (a full disk, a closed descriptor) is an error, not something to swallow.
/// A reader that went away (`skif | head`) is not a failure of the run: the output is simply no longer wanted.
pub fn write_stdout(bytes: &[u8]) -> Result<(), CliError> {
    let mut out = io::stdout().lock();
    match out.write_all(bytes).and_then(|()| out.flush()) {
        Err(error) if error.kind() != io::ErrorKind::BrokenPipe => {
            Err(CliError::new(Kind::Io, format!("cannot write to stdout: {error}")))
        }
        _ => Ok(()),
    }
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
    pub annotations: bool,
    pub color: bool,
    /// The directory diagnostic paths are relative to.
    pub root: &'a Path,
    pub repo_root: Option<PathBuf>,
    pub rules: Vec<&'static RuleMeta>,
    /// Whether the run covers several roots: human output is grouped by root and JSON lists them.
    pub multi: bool,
    /// The roots of a multi-root run that could not be loaded, as JSON `errors` entries.
    pub errors: Vec<Value>,
    /// The directories at the depth limit the walk of a multi-root run did not enter, as a JSON array.
    pub truncated: Value,
}

/// One root of a run, with paths already relative to [`Rendering::root`].
pub struct RootView<'a> {
    pub prefix: &'a Path,
    pub report: &'a Report,
    /// The entry of the JSON `roots` array.
    pub json: Option<Value>,
    pub fix: Option<&'a FixRun>,
}

/// Renders the report into memory, for a file or for one of several outputs of a run (ADR 0054).
pub fn render(
    report: &Report,
    roots: &[RootView<'_>],
    rendering: &Rendering<'_>,
    fs: &dyn FileSystem,
    fixes: &[RootFix<'_>],
) -> Result<Vec<u8>, CliError> {
    let options = render_options(report, rendering, fs)?;
    let mut buffer = Vec::new();
    write_report(report, roots, rendering, &options, fixes, &mut buffer).map_err(|error| cannot_render(&error))?;
    Ok(buffer)
}

/// Renders the report straight to stdout, for a run whose only output is stdout.
///
/// A report with tens of thousands of diagnostics runs to tens of megabytes, and holding it in memory first was a
/// visible part of the run. The price is that a renderer error after the first bytes went out (an `extra` JSON member
/// that shadows a reserved key) leaves a partial report on stdout, followed by the error; a buffered [`render`] fails
/// before anything is written. A reader that went away is not a failure, as in [`write_stdout`].
pub fn render_to_stdout(
    report: &Report,
    roots: &[RootView<'_>],
    rendering: &Rendering<'_>,
    fs: &dyn FileSystem,
    fixes: &[RootFix<'_>],
) -> Result<(), CliError> {
    let options = render_options(report, rendering, fs)?;
    let mut out = io::BufWriter::with_capacity(64 * 1024, io::stdout().lock());
    match write_report(report, roots, rendering, &options, fixes, &mut out).and_then(|()| out.flush()) {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        // The renderers' own failure (an `extra` member that shadows a reserved key); every other one is stdout's.
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => Err(cannot_render(&error)),
        Err(error) => Err(CliError::new(Kind::Io, format!("cannot write to stdout: {error}"))),
        Ok(()) => Ok(()),
    }
}

fn cannot_render(error: &io::Error) -> CliError {
    CliError::new(Kind::Io, format!("cannot render the report: {error}"))
}

/// The render options of `rendering`: sources for code frames, SARIF rules, the CODEOWNERS file.
fn render_options(report: &Report, rendering: &Rendering<'_>, fs: &dyn FileSystem) -> Result<RenderOptions, CliError> {
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
    if rendering.format == Format::Codeowners {
        options = options.with_codeowners(read_codeowners(rendering.repo_root.as_deref().unwrap_or(root), fs)?);
    }
    Ok(options)
}

/// The CODEOWNERS file of the repository at `top`, from the first of the locations GitHub and GitLab search.
fn read_codeowners(top: &Path, fs: &dyn FileSystem) -> Result<CodeOwners, CliError> {
    for location in CODEOWNERS_LOCATIONS {
        let path = top.join(location);
        if fs.is_file(&path) {
            let text = fs
                .read_to_string(&path)
                .map_err(|error| CliError::io("cannot read", &path, &error))?;
            return Ok(CodeOwners::parse(&text));
        }
    }
    // The repository lacks a file the run is configured to need, like a missing config file, not a bad flag.
    Err(CliError::new(
        Kind::Config,
        format!(
            "--format codeowners needs a CODEOWNERS file; none of {} exists in {}",
            CODEOWNERS_LOCATIONS.join(", "),
            top.display()
        ),
    ))
}

fn write_human_roots(
    roots: &[RootView<'_>],
    rendering: &Rendering<'_>,
    options: &RenderOptions,
    out: &mut dyn Write,
) -> io::Result<()> {
    for view in roots {
        if rendering.multi {
            writeln!(out, "==> {}", escape_controls(&roots::display(view.prefix)))?;
        }
        if let Some(run) = view.fix {
            run.write_human_at(view.prefix, out)?;
        }
        render_human(view.report, options, out)?;
        if rendering.multi {
            writeln!(out)?;
        }
    }
    Ok(())
}

fn write_report(
    report: &Report,
    roots: &[RootView<'_>],
    rendering: &Rendering<'_>,
    options: &RenderOptions,
    fixes: &[RootFix<'_>],
    out: &mut dyn Write,
) -> io::Result<()> {
    match rendering.format {
        Format::Human => {
            write_human_roots(roots, rendering, options, out)?;
            if rendering.multi {
                writeln!(out, "Linted {}.", plural_roots(roots.len()))?;
            }
            if rendering.annotations {
                render_github(report, options, out)?;
            }
            Ok(())
        }
        Format::Json => {
            let mut extra = Map::new();
            if !fixes.is_empty() {
                extra.insert("fix".to_owned(), fix::fixes_json(fixes));
            }
            if rendering.multi {
                extra.insert(
                    "roots".to_owned(),
                    Value::Array(roots.iter().filter_map(|view| view.json.clone()).collect()),
                );
                if !rendering.errors.is_empty() {
                    extra.insert("errors".to_owned(), Value::Array(rendering.errors.clone()));
                }
                if rendering.truncated.as_array().is_some_and(|dirs| !dirs.is_empty()) {
                    extra.insert("truncated".to_owned(), rendering.truncated.clone());
                }
            }
            render_json_with(report, options, extra, out)
        }
        Format::Sarif => render_sarif(report, options, out),
        Format::Github => render_github(report, options, out),
        Format::Codeclimate => render_codeclimate(report, options, out),
        Format::Compact => render_compact(report, options, out),
        Format::Markdown => render_markdown(report, options, out),
        Format::Disclosure => render_disclosure(report, options, out),
        Format::Codeowners => render_codeowners(report, options, out),
    }
}

fn plural_roots(count: usize) -> String {
    if count == 1 {
        "1 root".to_owned()
    } else {
        format!("{count} roots")
    }
}

pub fn emit(bytes: &[u8], output: Option<&Path>) -> Result<(), CliError> {
    match output {
        Some(path) => write_atomic(path, bytes).map_err(|error| CliError::io("cannot write", path, &error)),
        None => write_stdout(bytes),
    }
}

#[cfg(test)]
mod tests {
    use skif::MemoryFs;

    use super::*;

    #[test]
    fn a_missing_codeowners_file_is_a_config_error() {
        let error = read_codeowners(Path::new("/repo"), &MemoryFs::new()).unwrap_err();
        assert_eq!(error.kind, Kind::Config);
        assert!(error.message.contains("CODEOWNERS"), "{}", error.message);
    }

    #[test]
    fn codeowners_is_read_from_the_first_location() {
        let fs = MemoryFs::new()
            .with_file("/repo/CODEOWNERS", "* @second\n")
            .with_file("/repo/.github/CODEOWNERS", "\u{feff}* @first\n");
        let codeowners = read_codeowners(Path::new("/repo"), &fs).unwrap();
        assert_eq!(codeowners.owners("package.json"), ["@first"]);
    }
}
