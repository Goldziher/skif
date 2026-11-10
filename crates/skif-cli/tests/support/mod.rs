//! Shared helpers for the CLI integration tests.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

pub struct Run {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Run {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.stdout)
            .unwrap_or_else(|error| panic!("stdout is not exactly one JSON document ({error}): {}", self.stdout))
    }
}

/// The binary under test with a clean environment: no `CI`, no colour.
pub fn skif() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_skif"));
    command
        .env_remove("CI")
        .env_remove("GITHUB_ACTIONS")
        .env("NO_COLOR", "1");
    command
}

pub fn run(command: &mut Command) -> Run {
    let output = command.output().unwrap();
    Run {
        code: output.status.code().expect("skif was killed by a signal"),
        stdout: String::from_utf8(output.stdout).unwrap(),
        stderr: String::from_utf8(output.stderr).unwrap(),
    }
}

/// A temporary copy of `tests/fixtures/<name>`.
pub fn fixture(name: &str) -> TempDir {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&source, dir.path());
    dir
}

/// Copies `tests/fixtures/<name>` into the existing directory `to`.
pub fn copy_fixture(name: &str, to: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    copy_dir(&source, to);
}

fn copy_dir(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            fs::create_dir_all(&target).unwrap();
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Every file below `dir` as relative path to contents.
pub fn tree(dir: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, String>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(base).unwrap().to_path_buf(),
                    fs::read_to_string(&path).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// Relative paths whose contents differ between two trees (added, removed or modified).
pub fn changed(before: &BTreeMap<PathBuf, String>, after: &BTreeMap<PathBuf, String>) -> Vec<PathBuf> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Replaces every spelling of `root` (raw, canonical, JSON-escaped, forward-slashed) with `[ROOT]`, and the skif
/// version as [`redact_version`] does.
pub fn redact(text: &str, root: &Path) -> String {
    let text = redact_version(text);
    let mut spellings = vec![root.to_path_buf()];
    if let Ok(canonical) = root.canonicalize() {
        spellings.push(canonical);
    }
    // Longest first: on macOS the canonical `/private/var/...` contains the raw `/var/...`.
    spellings.sort_by_key(|spelling| std::cmp::Reverse(spelling.as_os_str().len()));
    let mut text = text;
    for spelling in spellings {
        let raw = spelling.display().to_string();
        // A canonical Windows path is verbatim (`\\?\C:\...`), while the CLI prints the plain drive path.
        let raw = raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_owned();
        let escaped = raw.replace('\\', "\\\\");
        let slashed = raw.replace('\\', "/");
        for form in [escaped, raw, slashed] {
            text = text.replace(&form, "[ROOT]");
        }
    }
    text
}

/// `text` with the `"skif": "<version>"` member of a JSON document spelled `"skif": "[VERSION]"`, so a release
/// does not change every snapshot.
pub fn redact_version(text: &str) -> String {
    text.replace(
        &format!("\"skif\": \"{}\"", env!("CARGO_PKG_VERSION")),
        "\"skif\": \"[VERSION]\"",
    )
}

/// A fake package manager named `name` that appends its argv and working directory to `record` and exits with
/// `exit`. Returns the directory to use as the whole `PATH`.
pub fn shim(name: &str, record: &Path, exit: i32) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    write_shim(dir.path(), name, record, exit);
    dir
}

#[cfg(unix)]
fn write_shim(dir: &Path, name: &str, record: &Path, exit: i32) {
    use std::os::unix::fs::PermissionsExt;
    let script = format!(
        "#!/bin/sh\n{{ echo \"argv:$*\"; pwd -P; }} >> '{}'\nexit {exit}\n",
        record.display()
    );
    let path = dir.join(name);
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(windows)]
fn write_shim(dir: &Path, name: &str, record: &Path, exit: i32) {
    let script = format!(
        "@echo off\r\necho argv:%*>> \"{record}\"\r\ncd >> \"{record}\"\r\nexit /b {exit}\r\n",
        record = record.display()
    );
    fs::write(dir.join(format!("{name}.cmd")), script).unwrap();
}

/// What the shim recorded: one `(argv, cwd)` pair per invocation.
pub fn invocations(record: &Path) -> Vec<(String, PathBuf)> {
    let Ok(text) = fs::read_to_string(record) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().collect();
    lines
        .chunks(2)
        .map(|pair| {
            let argv = pair[0].strip_prefix("argv:").unwrap_or(pair[0]).trim().to_owned();
            (argv, PathBuf::from(pair[1].trim()))
        })
        .collect()
}
