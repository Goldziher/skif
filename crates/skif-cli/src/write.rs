//! Writing a `FixPlan` to disk: every target is validated first, every new content is staged in a temporary file
//! next to its target, and only then is anything renamed into place, so a failure leaves the tree as it was.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use skif::{FileChange, FixPlan};

use crate::error::{CliError, Kind};

/// What the plan does to one validated file.
enum Job<'a> {
    Write {
        target: PathBuf,
        bytes: &'a [u8],
        /// The previous contents of a modified file; `None` for a created one.
        original: Option<Vec<u8>>,
    },
    Remove {
        target: PathBuf,
        original: Vec<u8>,
    },
}

/// How to put a file back if a later step fails.
enum Undo {
    Restore { target: PathBuf, bytes: Vec<u8> },
    Remove { target: PathBuf },
}

/// A temporary file that is deleted unless it was renamed into place.
struct Temp(Option<PathBuf>);

impl Drop for Temp {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}

/// Directories created for new files; removed again (when still empty) unless the plan was written.
#[derive(Default)]
struct CreatedDirs {
    dirs: Vec<PathBuf>,
    keep: bool,
}

impl Drop for CreatedDirs {
    fn drop(&mut self) {
        if !self.keep {
            for dir in self.dirs.iter().rev() {
                let _ = fs::remove_dir(dir);
            }
        }
    }
}

/// Applies `plan` under `root`. Every path in the plan is resolved against `root`, never the working directory.
///
/// All targets are validated before the first byte is written, new contents are staged next to their targets and
/// renamed into place together, and a failure while doing so restores the files already replaced.
pub fn write_plan(plan: &FixPlan, root: &Path) -> Result<(), CliError> {
    let root = root
        .canonicalize()
        .map_err(|error| CliError::io("cannot resolve", root, &error))?;
    let jobs = plan
        .changes
        .iter()
        .map(|change| prepare(change, &root))
        .collect::<Result<Vec<_>, _>>()?;

    let mut dirs = CreatedDirs::default();
    let mut staged: Vec<(Temp, &Job<'_>)> = Vec::new();
    for job in &jobs {
        if let Job::Write { target, bytes, original } = job {
            let temp = stage(target, bytes, original.is_none(), &mut dirs)
                .map_err(|error| CliError::io("cannot write", target, &error))?;
            staged.push((temp, job));
        }
    }

    let mut undo: Vec<Undo> = Vec::new();
    let committed = commit(&mut staged, &jobs, &mut undo);
    if let Err(error) = committed {
        rollback(undo);
        return Err(error);
    }
    dirs.keep = true;
    Ok(())
}

fn commit(staged: &mut [(Temp, &Job<'_>)], jobs: &[Job<'_>], undo: &mut Vec<Undo>) -> Result<(), CliError> {
    for (temp, job) in staged.iter_mut() {
        let Job::Write { target, original, .. } = job else {
            continue;
        };
        let Some(path) = temp.0.take() else {
            continue;
        };
        if let Err(error) = fs::rename(&path, target) {
            temp.0 = Some(path);
            return Err(CliError::io("cannot write", target, &error));
        }
        undo.push(match original {
            Some(bytes) => Undo::Restore {
                target: target.clone(),
                bytes: bytes.clone(),
            },
            None => Undo::Remove { target: target.clone() },
        });
    }
    for job in jobs {
        if let Job::Remove { target, original } = job {
            match fs::remove_file(target) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(CliError::io("cannot delete", target, &error)),
            }
            undo.push(Undo::Restore {
                target: target.clone(),
                bytes: original.clone(),
            });
        }
    }
    Ok(())
}

fn rollback(undo: Vec<Undo>) {
    for step in undo.into_iter().rev() {
        let _ = match step {
            Undo::Restore { target, bytes } => fs::write(target, bytes),
            Undo::Remove { target } => fs::remove_file(target),
        };
    }
}

/// Validates one change and reads what a rollback would need.
fn prepare<'a>(change: &'a FileChange, root: &Path) -> Result<Job<'a>, CliError> {
    match change {
        FileChange::Modify { path, new_contents, .. } => {
            let target = target(root, path)?;
            let original = fs::read(&target).map_err(|error| CliError::io("cannot read", path, &error))?;
            Ok(Job::Write {
                target,
                bytes: new_contents.as_bytes(),
                original: Some(original),
            })
        }
        FileChange::Create { path, contents } => {
            let target = target(root, path)?;
            if fs::symlink_metadata(&target).is_ok() {
                return Err(CliError::new(
                    Kind::Io,
                    format!("refusing to create {}: it already exists", path.display()),
                ));
            }
            Ok(Job::Write {
                target,
                bytes: contents.as_bytes(),
                original: None,
            })
        }
        FileChange::Delete { path } => {
            let target = target(root, path)?;
            match fs::read(&target) {
                Ok(original) => Ok(Job::Remove { target, original }),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Job::Remove {
                    target,
                    original: Vec::new(),
                }),
                Err(error) => Err(CliError::io("cannot read", path, &error)),
            }
        }
        _ => Err(CliError::new(Kind::Io, "the fix plan holds a change this skif cannot write")),
    }
}

/// The absolute target of a workspace-relative `relative` path, which must stay inside `root`: no `..`, no
/// absolute paths, no symlink at the target and no symlinked directory leading out of the root.
fn target(root: &Path, relative: &Path) -> Result<PathBuf, CliError> {
    let outside = || {
        CliError::new(
            Kind::Io,
            format!("refusing to write outside the workspace: {}", relative.display()),
        )
    };
    if relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(outside());
    }
    let target = root.join(relative);
    let nearest = target
        .ancestors()
        .skip(1)
        .find(|dir| dir.exists())
        .ok_or_else(outside)?
        .canonicalize()
        .map_err(|error| CliError::io("cannot resolve", relative, &error))?;
    if !nearest.starts_with(root) {
        return Err(outside());
    }
    if fs::symlink_metadata(&target).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(CliError::new(
            Kind::Io,
            format!("refusing to write through a symlink: {}", relative.display()),
        ));
    }
    Ok(target)
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A fresh sibling of `target` that nothing else can have created: exclusive creation, process id and a counter.
fn temp_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    target.with_file_name(format!(".{name}.{}.{sequence}.skif-tmp", std::process::id()))
}

fn create_parents(target: &Path, dirs: &mut CreatedDirs) -> io::Result<()> {
    let missing: Vec<&Path> = target
        .ancestors()
        .skip(1)
        .take_while(|dir| !dir.as_os_str().is_empty() && !dir.exists())
        .collect();
    for dir in missing.into_iter().rev() {
        fs::create_dir(dir)?;
        dirs.dirs.push(dir.to_path_buf());
    }
    Ok(())
}

/// Writes `bytes` to a new temporary sibling of `target` (with its permissions, when it exists) and syncs it.
fn stage(target: &Path, bytes: &[u8], create: bool, dirs: &mut CreatedDirs) -> io::Result<Temp> {
    if create {
        create_parents(target, dirs)?;
    }
    let path = temp_path(target);
    let mut file = OpenOptions::new().write(true).create_new(true).open(&path)?;
    let temp = Temp(Some(path));
    file.write_all(bytes)?;
    if let Ok(metadata) = fs::metadata(target) {
        let _ = file.set_permissions(metadata.permissions());
    }
    file.sync_all()?;
    Ok(temp)
}

/// Writes `bytes` to `target` through a temporary sibling that is renamed over it; the temporary is removed on
/// failure.
pub fn write_atomic(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut dirs = CreatedDirs::default();
    let mut temp = stage(target, bytes, false, &mut dirs)?;
    let path = temp.0.take().unwrap_or_default();
    fs::rename(&path, target).inspect_err(|_| {
        let _ = fs::remove_file(&path);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modify(path: &str, contents: &str) -> FileChange {
        FileChange::Modify {
            path: path.into(),
            edits: Vec::new(),
            new_contents: contents.to_owned(),
        }
    }

    fn plan(changes: Vec<FileChange>) -> FixPlan {
        let mut plan = FixPlan::default();
        plan.changes = changes;
        plan
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".skif-tmp"))
            .collect()
    }

    #[test]
    fn atomic_write_replaces_the_file_and_leaves_no_temporary() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.json"), "old").unwrap();
        write_atomic(&dir.path().join("a.json"), b"new").unwrap();
        assert_eq!(fs::read_to_string(dir.path().join("a.json")).unwrap(), "new");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn rejects_paths_that_escape_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        assert!(target(&root, Path::new("../x")).is_err());
        assert!(target(&root, Path::new("/abs")).is_err());
        assert!(target(&root, Path::new("")).is_err());
        assert_eq!(target(&root, Path::new("a/b.json")).unwrap(), root.join("a").join("b.json"));
    }

    #[test]
    fn one_bad_target_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.json"), "old").unwrap();
        let bad = plan(vec![modify("a.json", "new"), modify("../escape.json", "x")]);
        assert!(write_plan(&bad, dir.path()).is_err());
        assert_eq!(fs::read_to_string(dir.path().join("a.json")).unwrap(), "old");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn a_failure_while_staging_leaves_the_tree_untouched() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.json"), "old").unwrap();
        fs::write(dir.path().join("blocker"), "file").unwrap();
        let failing = plan(vec![
            modify("a.json", "new"),
            FileChange::Create {
                path: Path::new("blocker").join("nested").join("b.json"),
                contents: "{}".to_owned(),
            },
        ]);
        assert!(write_plan(&failing, dir.path()).is_err());
        assert_eq!(fs::read_to_string(dir.path().join("a.json")).unwrap(), "old");
        assert!(leftovers(dir.path()).is_empty());
    }

    #[test]
    fn creating_over_an_existing_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.json"), "old").unwrap();
        let create = plan(vec![FileChange::Create {
            path: "a.json".into(),
            contents: "new".to_owned(),
        }]);
        assert!(write_plan(&create, dir.path()).is_err());
        assert_eq!(fs::read_to_string(dir.path().join("a.json")).unwrap(), "old");
    }

    #[test]
    fn created_files_get_their_directories() {
        let dir = tempfile::tempdir().unwrap();
        let create = plan(vec![FileChange::Create {
            path: Path::new("new").join("deep").join("c.json"),
            contents: "{}".to_owned(),
        }]);
        write_plan(&create, dir.path()).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("new").join("deep").join("c.json")).unwrap(),
            "{}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_out_of_the_root_are_not_followed() {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.json"), "secret").unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path().join("secret.json"), dir.path().join("link.json")).unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("linked-dir")).unwrap();

        let through_file = plan(vec![modify("link.json", "pwned")]);
        assert!(write_plan(&through_file, dir.path()).is_err());
        let through_dir = plan(vec![modify("linked-dir/secret.json", "pwned")]);
        assert!(write_plan(&through_dir, dir.path()).is_err());
        assert_eq!(fs::read_to_string(outside.path().join("secret.json")).unwrap(), "secret");
    }
}
