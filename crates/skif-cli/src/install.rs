//! The post-fix package manager step (ADR 0016): `<manager> install` in the workspace root.
//!
//! This is the only place skif starts another process. The executable is found through the platform's lookup
//! (`which`, which honours `PATHEXT`), so Windows `.cmd` shims resolve without hard-coding an extension.

use std::path::Path;
use std::process::{Command, Stdio};

use skif::PackageManager;

/// Outcome of the install step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallStatus {
    /// The package manager ran and succeeded.
    Ran { command: String },
    /// Nothing was run.
    Skipped { reason: &'static str },
    /// The package manager could not be started or exited unsuccessfully.
    Failed { command: String, message: String },
}

/// The executable name of `manager`.
pub const fn program(manager: PackageManager) -> Option<&'static str> {
    match manager {
        PackageManager::Npm => Some("npm"),
        PackageManager::Pnpm => Some("pnpm"),
        PackageManager::Yarn => Some("yarn"),
        PackageManager::Bun => Some("bun"),
        _ => None,
    }
}

/// Runs `<manager> install` with `root` as the working directory.
///
/// With `quiet`, the child's stdout is redirected to stderr so stdout stays a single JSON document.
pub fn install(manager: PackageManager, root: &Path, quiet: bool) -> InstallStatus {
    let Some(name) = program(manager) else {
        return InstallStatus::Skipped {
            reason: "unsupported package manager",
        };
    };
    let command = format!("{name} install");
    let executable = match which::which(name) {
        Ok(path) => path,
        Err(error) => {
            return InstallStatus::Failed {
                command,
                message: format!("cannot find `{name}` on PATH: {error}"),
            };
        }
    };
    eprintln!("Running `{command}` in {}", root.display());
    let stdout = if quiet {
        Stdio::from(std::io::stderr())
    } else {
        Stdio::inherit()
    };
    let status = Command::new(executable)
        .arg("install")
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(Stdio::inherit())
        .status();
    match status {
        Ok(status) if status.success() => InstallStatus::Ran { command },
        Ok(status) => InstallStatus::Failed {
            message: format!("`{command}` exited with {status}"),
            command,
        },
        Err(error) => InstallStatus::Failed {
            message: format!("cannot run `{command}`: {error}"),
            command,
        },
    }
}
