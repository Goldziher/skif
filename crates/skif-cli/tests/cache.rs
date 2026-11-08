//! `skif --cache` (ADR 0066): the module cache on disk never changes the report.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::fs;
use std::path::Path;

use support::{fixture, run, skif};

/// The `js` fixture with a few more sources, so the store holds several entries of different languages.
fn project() -> tempfile::TempDir {
    let dir = fixture("js");
    let app = dir.path().join("packages").join("app");
    fs::write(
        app.join("index.js"),
        "import 'left-pad';\nimport { used } from './util.ts';\nimport './style.css';\nused();\n",
    )
    .unwrap();
    fs::write(
        app.join("util.ts"),
        "export const used = () => 1;\nexport const unused = 2;\n",
    )
    .unwrap();
    fs::write(app.join("style.css"), "@import './base.css';\n").unwrap();
    fs::write(app.join("base.css"), "a { color: red; }\n").unwrap();
    fs::write(app.join("orphan.ts"), "export const orphan = 3;\n").unwrap();
    dir
}

/// The JSON report and exit code of `skif --format json` on `dir` with `flags`.
fn report(dir: &Path, flags: &[&str]) -> (String, i32) {
    let result = run(skif().current_dir(dir).args(["--format", "json"]).args(flags));
    assert!(matches!(result.code, 0 | 1), "{}", result.stderr);
    assert!(!result.stderr.contains("warning"), "{}", result.stderr);
    (result.stdout, result.code)
}

#[test]
fn no_cache_a_cold_cache_and_a_warm_cache_print_the_same_report() {
    let dir = project();
    let store = tempfile::tempdir().unwrap();
    let location = store.path().join("nested").join("cache");
    let flags = ["--cache", "--cache-location", location.to_str().unwrap()];
    let file = location.join("modules.bin");

    let uncached = report(dir.path(), &[]);
    assert!(
        uncached.0.contains("unused-exports") || uncached.0.contains("unused-files"),
        "{}",
        uncached.0
    );
    let cold = report(dir.path(), &flags);
    let written = fs::read(&file).unwrap();
    assert!(written.starts_with(b"SKIFMOD\0"));
    let warm = report(dir.path(), &flags);
    assert_eq!(cold, uncached);
    assert_eq!(warm, uncached);
    // A fully warm run rewrites nothing.
    assert_eq!(fs::read(&file).unwrap(), written);

    // A damaged store is ignored and replaced.
    fs::write(&file, b"garbage").unwrap();
    assert_eq!(report(dir.path(), &flags), uncached);
    assert_eq!(fs::read(&file).unwrap(), written);

    // An edit misses once and is stored; the report follows the edit.
    fs::write(
        dir.path().join("packages").join("app").join("util.ts"),
        "export const used = () => 1;\nexport const unused = 2;\nexport const another = 3;\n",
    )
    .unwrap();
    let edited = report(dir.path(), &[]);
    assert_ne!(edited, uncached);
    assert_eq!(report(dir.path(), &flags), edited);
    assert!(fs::read(&file).unwrap() != written, "the edited file was not stored");
    assert_eq!(report(dir.path(), &flags), edited);
}

#[test]
fn the_default_store_is_below_node_modules_of_the_root() {
    let dir = project();
    let uncached = report(dir.path(), &[]);
    let cold = report(&dir.path().join("packages").join("app"), &["--cache"]);
    let file = dir
        .path()
        .join("node_modules")
        .join(".cache")
        .join("skif")
        .join("modules.bin");
    assert!(fs::read(&file).unwrap().starts_with(b"SKIFMOD\0"));
    let warm = report(dir.path(), &["--cache"]);
    assert_eq!(cold, uncached);
    assert_eq!(warm, uncached);
}

#[test]
fn a_store_that_cannot_be_written_is_a_warning_only() {
    let dir = project();
    let uncached = report(dir.path(), &[]);
    // A file where the cache directory should be.
    let blocked = dir.path().join("blocked");
    fs::write(&blocked, "").unwrap();
    let result = run(skif().current_dir(dir.path()).args([
        "--format",
        "json",
        "--cache",
        "--cache-location",
        blocked.to_str().unwrap(),
    ]));
    assert_eq!((result.stdout, result.code), uncached);
    assert!(
        result.stderr.contains("warning: cannot write the module cache to"),
        "{}",
        result.stderr
    );
}

#[test]
fn cache_location_requires_cache() {
    let dir = project();
    let result = run(skif().current_dir(dir.path()).args(["--cache-location", "x"]));
    assert_eq!(result.code, 2, "{}", result.stderr);
    assert!(result.stderr.contains("--cache"), "{}", result.stderr);
    assert!(!dir.path().join("x").exists());
}
