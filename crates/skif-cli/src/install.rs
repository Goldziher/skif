//! The post-fix package manager step (ADR 0016): `<manager> install` in the workspace root.
//!
//! This is the only place skif starts another process. The executable is found through the platform's lookup
//! (`which`, which honours `PATHEXT`), so Windows `.cmd` shims resolve without hard-coding an extension.

use std::path::Path;
use std::process::{Command, Stdio};

use skif::PackageManager;

use crate::output::stderr_line;

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

/// The arguments after the program name. A fix edits manifests, so the lockfile has to be allowed to change: pnpm
/// and yarn berry freeze it by default in CI, which the arguments override. npm and bun update it on `install`.
fn arguments(manager: PackageManager, root: &Path) -> Vec<&'static str> {
    match manager {
        PackageManager::Pnpm => vec!["install", "--no-frozen-lockfile"],
        PackageManager::Yarn if is_yarn_berry(root) => vec!["install", "--no-immutable"],
        _ => vec!["install"],
    }
}

/// Whether the workspace uses yarn 2 or later, which has `--no-immutable`; classic yarn rejects the flag.
fn is_yarn_berry(root: &Path) -> bool {
    root.join(".yarnrc.yml").is_file()
        || std::fs::read_to_string(root.join("yarn.lock")).is_ok_and(|lock| lock.contains("__metadata:"))
        || std::fs::read_to_string(root.join("package.json")).is_ok_and(|manifest| yarn_version_is_berry(&manifest))
}

/// Whether `package.json`'s `packageManager` field names yarn at a version of 2 or later.
fn yarn_version_is_berry(manifest: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(manifest)
        .ok()
        .and_then(|value| value.get("packageManager")?.as_str().map(str::to_owned))
        .and_then(|field| field.strip_prefix("yarn@")?.split('.').next()?.parse::<u32>().ok())
        .is_some_and(|major| major >= 2)
}

/// Runs `<manager> install` with `root` as the working directory.
///
/// With `quiet`, the child's stdout is redirected to stderr so stdout stays the report.
pub fn install(manager: PackageManager, root: &Path, quiet: bool) -> InstallStatus {
    let Some(name) = program(manager) else {
        return InstallStatus::Skipped {
            reason: "unsupported package manager",
        };
    };
    let arguments = arguments(manager, root);
    let command = format!("{name} {}", arguments.join(" "));
    let executable = match which::which(name) {
        Ok(path) => path,
        Err(error) => {
            return InstallStatus::Failed {
                command,
                message: format!("cannot find `{name}` on PATH: {error}"),
            };
        }
    };
    stderr_line(&format!("Running `{command}` in {}", skif::slash(root)));
    let stdout = if quiet {
        Stdio::from(std::io::stderr())
    } else {
        Stdio::inherit()
    };
    let status = Command::new(executable)
        .args(&arguments)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pnpm_may_change_the_lockfile() {
        assert_eq!(
            arguments(PackageManager::Pnpm, Path::new(".")),
            ["install", "--no-frozen-lockfile"]
        );
    }

    #[test]
    fn npm_and_bun_take_no_extra_flags() {
        for manager in [PackageManager::Npm, PackageManager::Bun] {
            assert_eq!(arguments(manager, Path::new(".")), ["install"]);
        }
    }

    #[test]
    fn yarn_berry_is_detected_from_the_project() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(arguments(PackageManager::Yarn, dir.path()), ["install"]);
        std::fs::write(dir.path().join("yarn.lock"), "# yarn lockfile v1\n").unwrap();
        assert_eq!(arguments(PackageManager::Yarn, dir.path()), ["install"]);
        std::fs::write(dir.path().join("yarn.lock"), "__metadata:\n  version: 8\n").unwrap();
        assert_eq!(
            arguments(PackageManager::Yarn, dir.path()),
            ["install", "--no-immutable"]
        );
    }

    #[test]
    fn yarn_package_manager_field_decides_the_flavour() {
        assert!(yarn_version_is_berry(r#"{"packageManager": "yarn@4.9.2"}"#));
        assert!(!yarn_version_is_berry(r#"{"packageManager": "yarn@1.22.22"}"#));
        assert!(!yarn_version_is_berry(r#"{"packageManager": "pnpm@9.0.0"}"#));
        assert!(!yarn_version_is_berry("{"));
    }
}
