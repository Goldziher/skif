//! Running external commands with a time limit and their output captured to files.

use std::env;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// The outcome of a captured command.
pub struct Captured {
    /// Exit code; `None` when the command was killed (timeout) or ended by a signal.
    pub code: Option<i32>,
    pub timed_out: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub elapsed: Duration,
}

impl Captured {
    /// The first non-empty line of stderr, shortened, for a skip reason.
    pub fn stderr_summary(&self) -> String {
        let line = self
            .stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no output");
        line.chars().take(160).collect()
    }
}

/// Runs `command` to completion or until `timeout`, whichever is first, capturing stdout and stderr through files
/// named `<label>.out` and `<label>.err` in `scratch` so a large output cannot fill a pipe and block the child.
///
/// # Errors
/// Fails when the files cannot be written or the command cannot be started.
pub fn capture(command: &mut Command, scratch: &Path, label: &str, timeout: Duration) -> Result<Captured> {
    fs::create_dir_all(scratch).with_context(|| format!("creating {}", scratch.display()))?;
    let out_path = scratch.join(format!("{label}.out"));
    let err_path = scratch.join(format!("{label}.err"));
    let started = Instant::now();
    let mut child = command
        .stdin(Stdio::null())
        .stdout(File::create(&out_path).with_context(|| format!("creating {}", out_path.display()))?)
        .stderr(File::create(&err_path).with_context(|| format!("creating {}", err_path.display()))?)
        .spawn()
        .with_context(|| format!("starting {}", command.get_program().to_string_lossy()))?;
    let (code, timed_out) = loop {
        if let Some(status) = child.try_wait().context("waiting for the command")? {
            break (status.code(), false);
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            break (None, true);
        }
        thread::sleep(Duration::from_millis(25));
    };
    let elapsed = started.elapsed();
    let stdout = fs::read(&out_path).with_context(|| format!("reading {}", out_path.display()))?;
    let stderr = String::from_utf8_lossy(&fs::read(&err_path).unwrap_or_default()).into_owned();
    Ok(Captured {
        code,
        timed_out,
        stdout,
        stderr,
        elapsed,
    })
}

/// The first executable called `name` on `PATH`.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        env::temp_dir().join(format!("skif-bench-proc-{name}-{}", std::process::id()))
    }

    #[test]
    fn captures_output_and_exit_code() {
        let dir = scratch("ok");
        let got = capture(
            Command::new("sh").args(["-c", "echo out; echo err >&2; exit 3"]),
            &dir,
            "t",
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(got.code, Some(3));
        assert_eq!(String::from_utf8_lossy(&got.stdout).trim(), "out");
        assert_eq!(got.stderr_summary(), "err");
        assert!(!got.timed_out);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn kills_a_command_that_exceeds_the_timeout() {
        let dir = scratch("slow");
        let got = capture(Command::new("sleep").arg("30"), &dir, "t", Duration::from_millis(100)).unwrap();
        assert!(got.timed_out);
        assert_eq!(got.code, None);
        fs::remove_dir_all(&dir).unwrap();
    }
}
