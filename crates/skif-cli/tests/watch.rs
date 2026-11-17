//! `skif --watch` lints again after a file changes (ADR 0013).

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
        .env("SKIF_CACHE", "0")
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

#[test]
fn the_reports_a_run_writes_do_not_start_another_run() {
    let dir = fixture("clean");
    let report = dir.path().join("report.txt");
    let child = Command::new(env!("CARGO_BIN_EXE_skif"))
        .env("SKIF_CACHE", "0")
        .args(["--watch", "--format", "json", "--format"])
        .arg(format!("human={}", report.display()))
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
    assert!(
        report.is_file(),
        "the human report is written next to the JSON on stdout"
    );
    // Writing `report.txt` below the watched root changed the tree; a watcher that sees its own output lints again
    // within a few polls, and again after that, forever.
    assert!(
        runs.recv_timeout(Duration::from_secs(3)).is_err(),
        "the watcher reacted to the report it wrote"
    );
}

/// With `SKIF_TIMINGS` set, the timings of a round reach stderr when the round ends, not only when the watcher
/// exits: the watcher runs until it is interrupted, which loses whatever is still buffered.
#[test]
fn the_timings_of_a_round_are_written_before_the_next_round() {
    let dir = fixture("clean");
    let child = Command::new(env!("CARGO_BIN_EXE_skif"))
        .env("SKIF_CACHE", "0")
        .args(["--watch", "--format", "json"])
        .arg(dir.path())
        .env_remove("CI")
        .env_remove("GITHUB_ACTIONS")
        .env("SKIF_TIMINGS", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut watcher = Watcher(child);
    let stderr = watcher.0.stderr.take().unwrap();
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            let Ok(line) = line else { return };
            if send.send(line).is_err() {
                return;
            }
        }
    });
    let mut timings = 0;
    loop {
        let line = receive.recv_timeout(TIMEOUT).expect("the end of the first round");
        if line.starts_with("skif timing: ") {
            timings += 1;
        }
        if line.starts_with("skif: watching ") {
            break;
        }
    }
    assert!(timings > 0, "no timing line before the round ended");
}

/// The cache store is rewritten by every round that parsed something; a watcher that saw it would never rest.
#[test]
fn the_cache_a_round_writes_does_not_start_another_round() {
    let dir = fixture("js");
    // A location inside the watched root that skif did not create, so no `.gitignore` of its own hides it.
    let location = dir.path().join("cache-here");
    std::fs::create_dir(&location).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_skif"))
        .args(["--watch", "--format", "json", "--cache-location"])
        .arg(&location)
        .arg(dir.path())
        .env_remove("CI")
        .env_remove("GITHUB_ACTIONS")
        .env_remove("TEST_TMPDIR")
        .env_remove("SKIF_CACHE")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut watcher = Watcher(child);
    let runs = documents(&mut watcher.0);

    runs.recv_timeout(TIMEOUT).expect("the first run");
    // The store is written once the report is out.
    let stored = || {
        std::fs::read_dir(&location)
            .unwrap()
            .any(|entry| entry.unwrap().path().join("modules.bin").is_file())
    };
    let deadline = std::time::Instant::now() + TIMEOUT;
    while !stored() {
        assert!(std::time::Instant::now() < deadline, "the first round wrote no store");
        thread::sleep(Duration::from_millis(50));
    }
    assert!(
        runs.recv_timeout(Duration::from_secs(3)).is_err(),
        "the watcher reacted to the cache it wrote"
    );
    // A source edit is still seen, and its round writes the store again without starting a third.
    std::fs::write(
        dir.path().join("packages").join("app").join("index.js"),
        "import 'left-pad';\nexport const edited = 1;\n",
    )
    .unwrap();
    runs.recv_timeout(TIMEOUT).expect("the run after the edit");
    assert!(
        runs.recv_timeout(Duration::from_secs(3)).is_err(),
        "the watcher reacted to the cache it wrote after an edit"
    );
}
