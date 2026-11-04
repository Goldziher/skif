//! `skif explain-usage`: why the JavaScript analysis counts a file, or one export of it, as used.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif::{CachedFs, ConfigOverrides, FileSystem, OsFs};

use crate::args::{ExplainUsageArgs, Format};
use crate::error::{CliError, Kind};
use crate::lint::canonical;
use crate::output::{emit_error, write_stdout};
use crate::roots::{self, Resolved};

const CLEAN: u8 = 0;
const COULD_NOT_RUN: u8 = 2;

pub fn run(args: &ExplainUsageArgs) -> u8 {
    match execute(args) {
        Ok(()) => CLEAN,
        Err(error) => {
            emit_error(&error, Format::Human);
            COULD_NOT_RUN
        }
    }
}

/// Splits `FILE#EXPORT` into the file and the export name.
fn split_target(target: &str) -> (&str, Option<&str>) {
    match target.split_once('#') {
        Some((file, export)) if !export.is_empty() => (file, Some(export)),
        Some((file, _)) => (file, None),
        None => (target, None),
    }
}

/// `file` relative to the workspace `root`, as paths appear in diagnostics.
fn root_relative(root: &Path, file: &str) -> PathBuf {
    let path = Path::new(file);
    path.strip_prefix(root)
        .unwrap_or_else(|_| Path::new(file.trim_start_matches("./")))
        .to_path_buf()
}

fn execute(args: &ExplainUsageArgs) -> Result<(), CliError> {
    let start = canonical(&args.path)?;
    let Resolved { targets, .. } = roots::resolve(&start, false, &CachedFs::new(OsFs), &ConfigOverrides::default())?;
    let target = targets.first().ok_or_else(|| {
        CliError::new(
            Kind::Workspace,
            format!("no workspace or package found at {}", start.display()),
        )
    })?;
    let (file, export) = split_target(&args.target);
    let fs: Arc<dyn FileSystem> = Arc::new(OsFs);
    let text = skif::explain_usage(
        &target.config,
        &fs,
        &target.workspace,
        &root_relative(&target.root, file),
        export,
    );
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

    #[test]
    fn the_file_is_relative_to_the_root() {
        let root = Path::new("/repo");
        assert_eq!(root_relative(root, "/repo/src/a.ts"), PathBuf::from("src/a.ts"));
        assert_eq!(root_relative(root, "./src/a.ts"), PathBuf::from("src/a.ts"));
        assert_eq!(root_relative(root, "src/a.ts"), PathBuf::from("src/a.ts"));
    }
}
