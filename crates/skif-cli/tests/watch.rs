//! `skif --watch` lints again after a file changes (ADR 0050).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use support::{fixture, run, skif};

/// Generous, so a loaded machine does not fail the test; a passing run takes well under a second.
const TIMEOUT: Duration = Duration::from_secs(60);

/// Kills the watcher when the test ends, also when it fails.
struct Watcher(Child);

impl Drop for Watcher {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The JSON documents the watcher prints, one per run: a pretty-printed document ends with a `}` line.
fn documents(child: &mut Child) -> Receiver<Value> {
    let stdout = child.stdout.take().unwrap();
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let mut buffer = String::new();
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            buffer.push_str(&line);
            buffer.push('\n');
            if line == "}" {
                let document = serde_json::from_str(&buffer).unwrap();
                buffer.clear();
                if send.send(document).is_err() {
                    return;
                }
            }
        }
    });
    receive
}

#[test]
fn a_changed_manifest_is_linted_again() {
    let dir = fixture("clean");
    let child = Command::new(env!("CARGO_BIN_EXE_skif"))
        .args(["--watch", "--format", "json"])
        .arg(dir.path())
        .env_remove("CI")
        .env_remove("GITHUB_ACTIONS")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut watcher = Watcher(child);
    let runs = documents(&mut watcher.0);

    let first = runs.recv_timeout(TIMEOUT).expect("the first run");
    assert_eq!(first["diagnostics"], Value::Array(Vec::new()), "{first}");

    // An empty `devDependencies` map is a finding.
    let manifest = dir.path().join("packages").join("a").join("package.json");
    std::fs::write(
        &manifest,
        "{\n  \"name\": \"@cli/a\",\n  \"version\": \"0.0.0\",\n  \"dependencies\": {\n    \"is-number\": \"7.0.0\"\n  },\n  \"devDependencies\": {}\n}\n",
    )
    .unwrap();
    let second = runs.recv_timeout(TIMEOUT).expect("a run after the change");
    assert_eq!(second["diagnostics"][0]["rule"], "empty-dependencies", "{second}");
}

#[test]
fn watch_refuses_fix() {
    let dir = fixture("clean");
    let result = run(skif().args(["--watch", "--fix"]).arg(dir.path()));
    assert_eq!(result.code, 2, "{}", result.stderr);
    assert!(result.stderr.contains("cannot be used with"), "{}", result.stderr);
}
