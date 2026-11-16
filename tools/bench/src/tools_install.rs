//! The comparison tools (knip, syncpack, sherif) installed from the lock beside the corpus manifest, with every
//! dependency at the version the lock pins, so that `run`, `parity`, `syncpack-diff` and `deadcode` all measure and
//! compare the same tool code; `npx` would resolve the transitive dependencies afresh each time.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::corpus;
use crate::proc::{capture, find_on_path};

/// The files of the lock directory ([`corpus::tool_lock_dir`]) that `npm ci` installs from.
const TOOL_LOCK_FILES: [&str; 2] = ["package.json", "package-lock.json"];

/// The executables the lock installs, as `node_modules/.bin` names.
const LOCKED_TOOLS: [&str; 3] = ["sherif", "syncpack", "knip"];

/// How long `npm ci` of the locked tools may take.
const INSTALL_TIMEOUT: Duration = Duration::from_mins(30);

/// The `npm ci` arguments that install the locked comparison tools, without their lifecycle scripts, into `prefix`,
/// which holds a copy of the lock's `package.json` and `package-lock.json`.
pub fn tool_install_args(prefix: &Path) -> Vec<String> {
    let mut args: Vec<String> = ["ci", "--ignore-scripts", "--no-audit", "--no-fund", "--prefix"]
        .map(str::to_owned)
        .into();
    args.push(prefix.to_string_lossy().into_owned());
    args
}

/// The absolute path of the executable of an installed tool, which stays valid when a command changes directory.
///
/// # Errors
/// Fails when the current directory cannot be read to resolve a relative `prefix`.
pub fn program(prefix: &Path, package: &str) -> Result<PathBuf> {
    let path = prefix.join("node_modules").join(".bin").join(package);
    std::path::absolute(&path).with_context(|| format!("resolving {}", path.display()))
}

/// Whether `prefix` already holds a copy of the lock byte for byte and every tool's executable, so `npm ci` would
/// install what is there.
fn is_installed(lock_dir: &Path, prefix: &Path) -> bool {
    TOOL_LOCK_FILES.iter().all(
        |file| match (fs::read(lock_dir.join(file)), fs::read(prefix.join(file))) {
            (Ok(locked), Ok(copy)) => locked == copy,
            _ => false,
        },
    ) && LOCKED_TOOLS
        .iter()
        .all(|tool| program(prefix, tool).is_ok_and(|path| path.is_file()))
}

/// Installs the locked tools from `lock_dir` into `prefix` with `npm ci`, unless `prefix` already holds them from the
/// same lock. `Ok(Err(reason))` when they cannot be installed (no `npm`, or `npm ci` fails).
///
/// # Errors
/// Fails when the lock files cannot be copied or `npm` cannot be started.
pub fn install(lock_dir: &Path, prefix: &Path) -> Result<Result<(), String>> {
    if find_on_path("npm").is_none() {
        return Ok(Err("npm is not installed, so the pinned tools cannot be".to_owned()));
    }
    let prefix = std::path::absolute(prefix).with_context(|| format!("resolving {}", prefix.display()))?;
    if is_installed(lock_dir, &prefix) {
        eprintln!("bench: the locked comparison tools are already in {}", prefix.display());
        return Ok(Ok(()));
    }
    fs::create_dir_all(&prefix).with_context(|| format!("creating {}", prefix.display()))?;
    for file in TOOL_LOCK_FILES {
        let (from, to) = (lock_dir.join(file), prefix.join(file));
        fs::copy(&from, &to).with_context(|| format!("copying {} to {}", from.display(), to.display()))?;
    }
    eprintln!(
        "bench: installing the locked comparison tools into {}",
        prefix.display()
    );
    let run = capture(
        Command::new("npm")
            .args(tool_install_args(&prefix))
            .current_dir(&prefix),
        &prefix,
        "npm-ci",
        INSTALL_TIMEOUT,
    )?;
    if run.timed_out || run.code != Some(0) {
        return Ok(Err(format!(
            "npm ci of the locked tools failed: {}",
            run.failure_summary()
        )));
    }
    Ok(Ok(()))
}

/// The executable of `package` from the locked tools installed in `prefix` (installed first when needed), for a
/// command that cannot run without it.
///
/// # Errors
/// Fails when the tools cannot be installed from the lock beside `corpus_file`, or the install has no `package`.
pub fn require(corpus_file: &Path, prefix: &Path, package: &str) -> Result<PathBuf> {
    if let Err(reason) = install(&corpus::tool_lock_dir(corpus_file), prefix)? {
        bail!("{package} cannot be installed from the locked tools: {reason}");
    }
    let path = program(prefix, package)?;
    if !path.is_file() {
        bail!("the locked tools install has no {}", path.display());
    }
    Ok(path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_locked_tools_are_installed_with_npm_ci_without_scripts() {
        assert_eq!(
            tool_install_args(Path::new("/bench/tools")),
            [
                "ci",
                "--ignore-scripts",
                "--no-audit",
                "--no-fund",
                "--prefix",
                "/bench/tools"
            ]
        );
    }

    #[test]
    fn a_tool_runs_from_the_bin_of_the_install_by_absolute_path() {
        assert_eq!(
            program(Path::new("/bench/tools"), "knip").unwrap(),
            Path::new("/bench/tools/node_modules/.bin/knip")
        );
        assert!(program(Path::new("target/tools"), "knip").unwrap().is_absolute());
    }

    #[test]
    fn an_install_is_current_only_with_the_same_lock_and_every_executable() {
        let dir = std::env::temp_dir().join(format!("skif-bench-tools-install-{}", std::process::id()));
        let (lock, prefix) = (dir.join("lock"), dir.join("prefix"));
        let bin = prefix.join("node_modules/.bin");
        fs::create_dir_all(&lock).unwrap();
        fs::create_dir_all(&bin).unwrap();
        for file in TOOL_LOCK_FILES {
            fs::write(lock.join(file), "{}").unwrap();
            fs::write(prefix.join(file), "{}").unwrap();
        }
        assert!(!is_installed(&lock, &prefix), "no executables yet");
        for tool in LOCKED_TOOLS {
            fs::write(bin.join(tool), "").unwrap();
        }
        assert!(is_installed(&lock, &prefix));
        fs::write(lock.join("package-lock.json"), "{ }").unwrap();
        assert!(!is_installed(&lock, &prefix), "the lock changed");
        fs::remove_dir_all(&dir).unwrap();
    }
}
