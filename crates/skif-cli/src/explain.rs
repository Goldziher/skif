//! `skif explain-usage`: why the JavaScript analysis counts a file, one export of it, or a dependency as used.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};
use skif::{CachedFs, ConfigOverrides, FileSystem, OsFs, PackageId, Target, UsageReport};
use skif_report::escape_multiline;

use crate::args::{ExplainUsageArgs, Format, LintArgs};
use crate::config::document;
use crate::error::{CliError, Kind};
use crate::lint::{self, canonical};
use crate::output::{emit_error_with, write_stdout};
use crate::roots::{self, Resolved};

const CLEAN: u8 = 0;
const COULD_NOT_RUN: u8 = 2;

pub fn run(args: &ExplainUsageArgs) -> u8 {
    let format = Format::from(args.format);
    match execute(args, format) {
        Ok(()) => CLEAN,
        Err(failure) => {
            let (error, extra) = *failure;
            emit_error_with(&error, format, extra, &mut std::io::stdout().lock());
            COULD_NOT_RUN
        }
    }
}

pub type Failure = Box<(CliError, Map<String, Value>)>;

fn fail(kind: Kind, message: impl Into<String>) -> Failure {
    Box::new((CliError::new(kind, message), Map::new()))
}

/// Splits `FILE#EXPORT` into the file and the export name.
fn split_target(target: &str) -> (&str, Option<&str>) {
    match target.split_once('#') {
        Some((file, export)) if !export.is_empty() => (file, Some(export)),
        Some((file, _)) => (file, None),
        None => (target, None),
    }
}

/// A file to explain, by the path it was given as and by its canonical path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetFile {
    /// The path as written, made absolute but with its links kept: a file below a linked directory (Bazel's
    /// `bazel-bin`) is reached by the analysis, and named in the lint report, through the link. `None` when it climbs
    /// with `..`, which only the canonical path resolves reliably.
    written: Option<PathBuf>,
    /// The canonical absolute path.
    canonical: PathBuf,
}

/// `file`, which is relative to `cwd` when it is not absolute; an error when it does not exist.
pub fn resolve_file(cwd: &Path, file: &str) -> Result<TargetFile, CliError> {
    let path = Path::new(file);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let canonical = canonical(&absolute)?;
    let written = (!absolute.components().any(|component| component == Component::ParentDir))
        .then(|| crate::lint::absolute_file(&absolute));
    Ok(TargetFile { written, canonical })
}

/// The target whose root is the deepest ancestor of `file`.
fn owning_target<'a>(targets: &'a [Target], file: &Path) -> Option<&'a Target> {
    targets
        .iter()
        .filter(|target| file.starts_with(&target.root))
        .max_by_key(|target| target.root.components().count())
}

/// The target that owns `file` with the file's path in it: by the path as written first, as the lint report names a
/// file below a linked directory, then by the canonical path.
fn locate<'a>(targets: &'a [Target], file: &'a TargetFile) -> Option<(&'a Target, &'a Path)> {
    file.written
        .iter()
        .chain(std::iter::once(&file.canonical))
        .find_map(|path| owning_target(targets, path).map(|target| (target, path.as_path())))
}

fn overrides(args: &ExplainUsageArgs) -> ConfigOverrides {
    ConfigOverrides::default()
        .with_production(args.production.then_some(true))
        .with_config_file(args.config.as_ref().map(|file| crate::lint::absolute_file(file)))
        .with_no_config(args.no_config)
}

/// Explains `file`, or its `export`, in the root found from `start` (canonical).
///
/// A file outside the module graph is a failure that carries the report, so the reason reaches the user.
pub fn explain(
    start: &Path,
    file: &TargetFile,
    export: Option<&str>,
    overrides: &ConfigOverrides,
) -> Result<UsageReport, Failure> {
    let Resolved { targets, .. } =
        roots::resolve(start, false, &CachedFs::new(OsFs), overrides).map_err(|error| Box::new((error, Map::new())))?;
    let (target, path) = locate(&targets, file).ok_or_else(|| {
        fail(
            Kind::Usage,
            format!(
                "{} is not inside a workspace or package found at {}",
                file.canonical.display(),
                start.display()
            ),
        )
    })?;
    let relative = path.strip_prefix(&target.root).unwrap_or(path);
    let fs: Arc<dyn FileSystem> = Arc::new(OsFs);
    let report = skif::explain_usage(&target.config, &fs, &target.workspace, relative, export);
    if !report.in_graph {
        let value = serde_json::to_value(&report).map_err(|error| fail(Kind::Internal, error.to_string()))?;
        let mut extra = Map::new();
        extra.insert("report".to_owned(), value);
        return Err(Box::new((
            CliError::new(Kind::Usage, report.render().trim_end()),
            extra,
        )));
    }
    Ok(report)
}

fn execute(args: &ExplainUsageArgs, format: Format) -> Result<(), Failure> {
    let cwd = std::env::current_dir()
        .map_err(|error| fail(Kind::Io, format!("cannot read the current directory: {error}")))?;
    if let Some(name) = &args.dependency {
        return explain_dependency(args, &cwd, name, format);
    }
    let target = args
        .target
        .as_deref()
        .ok_or_else(|| fail(Kind::Usage, "give a FILE to explain, or --dependency NAME"))?;
    let (file, export) = split_target(target);
    let file = resolve_file(&cwd, file).map_err(|error| Box::new((error, Map::new())))?;
    let start =
        canonical(args.path.as_deref().unwrap_or(Path::new("."))).map_err(|error| Box::new((error, Map::new())))?;
    let report = explain(&start, &file, export, &overrides(args))?;
    match format {
        Format::Json => {
            let value = serde_json::to_value(&report).map_err(|error| fail(Kind::Internal, error.to_string()))?;
            let mut members = Map::new();
            members.insert("report".to_owned(), value);
            let text = document(members).map_err(|error| Box::new((error, Map::new())))?;
            write_stdout(text.as_bytes()).map_err(|error| Box::new((error, Map::new())))
        }
        _ => write_stdout(report.render().as_bytes()).map_err(|error| Box::new((error, Map::new()))),
    }
}

/// The project path of `explain-usage --dependency`, where the one positional argument is `PATH`: there is no file.
fn dependency_path(args: &ExplainUsageArgs) -> Result<&Path, Failure> {
    match (&args.target, &args.path) {
        (None, None) => Ok(Path::new(".")),
        (Some(target), None) => Ok(Path::new(target)),
        (None, Some(path)) => Ok(path.as_path()),
        (Some(_), Some(_)) => Err(fail(Kind::Usage, "--dependency explains no file: give PATH alone")),
    }
}

/// The package of `target` named `value`, else the one whose directory `value` (relative to `cwd`) is.
fn find_package(target: &Target, cwd: &Path, value: &str) -> Result<PackageId, Failure> {
    let by_name = target
        .workspace
        .iter()
        .find(|(_, package)| package.name.as_deref() == Some(value));
    let by_dir = || {
        let dir = canonical(&cwd.join(value)).ok()?;
        let relative = dir.strip_prefix(&target.root).ok()?;
        target
            .workspace
            .iter()
            .find(|(_, package)| package.dir.as_path() == relative)
    };
    by_name.or_else(by_dir).map(|(id, _)| id).ok_or_else(|| {
        fail(
            Kind::Usage,
            format!(
                "no package is named `{value}` or lies in that directory in the workspace at {}",
                target.root.display()
            ),
        )
    })
}

/// `skif explain-usage --dependency NAME [--package NAME|DIR]`: why the dependency is used or unused in the packages
/// of the root found from the path.
fn explain_dependency(args: &ExplainUsageArgs, cwd: &Path, name: &str, format: Format) -> Result<(), Failure> {
    let plain = |error: CliError| Box::new((error, Map::new()));
    let start = canonical(dependency_path(args)?).map_err(plain)?;
    let Resolved { targets, .. } =
        roots::resolve(&start, false, &CachedFs::new(OsFs), &overrides(args)).map_err(plain)?;
    let target = owning_target(&targets, &start)
        .or_else(|| targets.first())
        .ok_or_else(|| {
            fail(
                Kind::Usage,
                format!("no workspace or package found at {}", start.display()),
            )
        })?;
    let package = args
        .package
        .as_deref()
        .map(|value| find_package(target, cwd, value))
        .transpose()?;
    let fs: Arc<dyn FileSystem> = Arc::new(OsFs);
    let explanation = skif::explain_dependency(&target.config, &fs, &target.workspace, name, package);
    match format {
        Format::Json => {
            let value = serde_json::to_value(&explanation).map_err(|error| fail(Kind::Internal, error.to_string()))?;
            let mut members = Map::new();
            members.insert("report".to_owned(), value);
            let text = document(members).map_err(plain)?;
            write_stdout(text.as_bytes()).map_err(plain)
        }
        _ => write_stdout(escape_multiline(&explanation.render()).as_bytes()).map_err(plain),
    }
}

/// `skif --trace-dependency NAME`: where every root of the run lists, imports and uses the dependency.
pub fn trace_dependency(args: &LintArgs, name: &str) -> Result<(), CliError> {
    let format = args.format();
    if !matches!(format, Format::Human | Format::Json) || args.output.is_some() {
        return Err(CliError::new(
            Kind::Usage,
            "--trace-dependency prints to stdout as `human` or `json`",
        ));
    }
    let start = canonical(&args.path)?;
    let Resolved {
        base,
        mut targets,
        failures,
        ..
    } = roots::resolve(&start, args.recursive, &CachedFs::new(OsFs), &lint::overrides(args))?;
    if let Some(failure) = failures.first() {
        return Err(CliError::new(Kind::Workspace, failure.describe()));
    }
    let fs: Arc<dyn FileSystem> = Arc::new(OsFs);
    let mut traces = Vec::with_capacity(targets.len());
    for target in &mut targets {
        lint::adjust(args, &mut target.config)?;
    }
    lint::apply_tsconfig(args, &mut targets)?;
    // The configuration check a lint run makes, so a bad setting (an absolute or escaping tsconfig) is an error here
    // too instead of being dropped by the analysis.
    roots::Linters::build(&targets)?;
    for target in &targets {
        let trace = skif::trace_dependency(&target.config, &fs, &target.workspace, name);
        let root = roots::display(target.root.strip_prefix(&base).unwrap_or(&target.root));
        traces.push((root, trace));
    }
    if format == Format::Json {
        let mut entries = Vec::with_capacity(traces.len());
        for (root, trace) in &traces {
            let mut value =
                serde_json::to_value(trace).map_err(|error| CliError::new(Kind::Internal, error.to_string()))?;
            if let Value::Object(members) = &mut value {
                members.insert("root".to_owned(), Value::String(root.clone()));
            }
            entries.push(value);
        }
        let mut members = Map::new();
        members.insert("traces".to_owned(), Value::Array(entries));
        return write_stdout(document(members)?.as_bytes());
    }
    let multi = traces.len() > 1;
    let mut text = String::new();
    for (root, trace) in &traces {
        if multi {
            text.push_str(&escape_multiline(root));
            text.push_str(":\n");
        }
        text.push_str(&escape_multiline(&trace.render()));
    }
    write_stdout(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_target_splits_at_the_export() {
        assert_eq!(split_target("src/a.ts#run"), ("src/a.ts", Some("run")));
        assert_eq!(split_target("src/a.ts#"), ("src/a.ts", None));
        assert_eq!(split_target("src/a.ts"), ("src/a.ts", None));
    }
}
