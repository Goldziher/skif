//! Writing a `FixPlan` to disk, root-relatively and atomically per file.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use skif::{FileChange, FixPlan};

use crate::error::{CliError, Kind};

/// Applies `plan` under `root`. Every path in the plan is resolved against `root`, never the working directory.
pub fn write_plan(plan: &FixPlan, root: &Path) -> Result<(), CliError> {
    for change in &plan.changes {
        match change {
            FileChange::Modify {
                path, new_contents, ..
            } => write_atomic(&target(root, path)?, new_contents.as_bytes())
                .map_err(|error| CliError::io("cannot write", path, &error))?,
            FileChange::Create { path, contents } => {
                let target = target(root, path)?;
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|error| CliError::io("cannot create", parent, &error))?;
                }
                write_atomic(&target, contents.as_bytes()).map_err(|error| CliError::io("cannot write", path, &error))?;
            }
            FileChange::Delete { path } => match fs::remove_file(target(root, path)?) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(CliError::io("cannot delete", path, &error)),
            },
            _ => {}
        }
    }
    Ok(())
}

fn target(root: &Path, relative: &Path) -> Result<PathBuf, CliError> {
    if relative.components().all(|component| matches!(component, Component::Normal(_))) {
        Ok(root.join(relative))
    } else {
        Err(CliError::new(
            Kind::Io,
            format!("refusing to write outside the workspace: {}", relative.display()),
        ))
    }
}

/// Writes to a temporary sibling and renames it over `target`, keeping the original permissions.
fn write_atomic(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let name = target.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let temporary = target.with_file_name(format!(".{name}.skif-tmp"));
    fs::write(&temporary, bytes)?;
    if let Ok(metadata) = fs::metadata(target) {
        let _ = fs::set_permissions(&temporary, metadata.permissions());
    }
    fs::rename(&temporary, target).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_replaces_the_file_and_leaves_no_temporary() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.json"), "old").unwrap();
        write_atomic(&dir.path().join("a.json"), b"new").unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("a.json")).unwrap(), "new");
        assert!(!dir.path().join(".a.json.skif-tmp").exists());
    }

    #[test]
    fn rejects_paths_that_escape_the_root() {
        let root = Path::new("root");
        assert!(target(root, Path::new("../x")).is_err());
        assert!(target(root, Path::new("/abs")).is_err());
        assert_eq!(target(root, Path::new("a/b.json")).unwrap(), root.join("a").join("b.json"));
    }
}
