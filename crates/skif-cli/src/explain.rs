//! `skif explain-usage`: why the JavaScript analysis counts a file, or one export of it, as used.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value};
use skif::{CachedFs, ConfigOverrides, FileSystem, OsFs, Target};

use crate::args::{ExplainUsageArgs, Format};
use crate::config::document;
use crate::error::{CliError, Kind};
use crate::lint::canonical;
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
            emit_error_with(&error, format, extra);
            COULD_NOT_RUN
        }
    }
}

type Failure = Box<(CliError, Map<String, Value>)>;

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

/// The canonical absolute path of `file`, which is relative to the current directory when it is not absolute.
fn resolve_file(cwd: &Path, file: &str) -> Result<PathBuf, CliError> {
    let path = Path::new(file);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    canonical(&absolute)
}

/// The target whose root is the deepest ancestor of `file`.
fn owning_target<'a>(targets: &'a [Target], file: &Path) -> Option<&'a Target> {
    targets
        .iter()
        .filter(|target| file.starts_with(&target.root))
        .max_by_key(|target| target.root.components().count())
}

fn overrides(args: &ExplainUsageArgs) -> ConfigOverrides {
    ConfigOverrides::default()
        .with_production(args.production.then_some(true))
        .with_config_file(
            args.config
                .as_ref()
                .map(|file| std::path::absolute(file).unwrap_or_else(|_| file.clone())),
        )
        .with_no_config(args.no_config)
}

fn execute(args: &ExplainUsageArgs, format: Format) -> Result<(), Failure> {
    let cwd = std::env::current_dir()
        .map_err(|error| fail(Kind::Io, format!("cannot read the current directory: {error}")))?;
    let (file, export) = split_target(&args.target);
    let file = resolve_file(&cwd, file).map_err(|error| Box::new((error, Map::new())))?;
    let start = canonical(&args.path).map_err(|error| Box::new((error, Map::new())))?;
    let Resolved { targets, .. } = roots::resolve(&start, false, &CachedFs::new(OsFs), &overrides(args))
        .map_err(|error| Box::new((error, Map::new())))?;
    let target = owning_target(&targets, &file).ok_or_else(|| {
        fail(
            Kind::Usage,
            format!(
                "{} is not inside a workspace or package found at {}",
                file.display(),
                start.display()
            ),
        )
    })?;
    let relative = file.strip_prefix(&target.root).unwrap_or(&file);
    let fs: Arc<dyn FileSystem> = Arc::new(OsFs);
    let report = skif::explain_usage(&target.config, &fs, &target.workspace, relative, export);
    let value = serde_json::to_value(&report).map_err(|error| fail(Kind::Internal, error.to_string()))?;
    if !report.in_graph {
        let mut extra = Map::new();
        extra.insert("report".to_owned(), value);
        return Err(Box::new((
            CliError::new(Kind::Usage, report.render().trim_end()),
            extra,
        )));
    }
    match format {
        Format::Json => {
            let mut members = Map::new();
            members.insert("report".to_owned(), value);
            let text = document(members).map_err(|error| Box::new((error, Map::new())))?;
            write_stdout(text.as_bytes()).map_err(|error| Box::new((error, Map::new())))
        }
        _ => write_stdout(report.render().as_bytes()).map_err(|error| Box::new((error, Map::new()))),
    }
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
