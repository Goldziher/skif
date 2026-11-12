//! `skif --cache` (ADR 0029): the module cache on disk never changes the report.

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
fn under_bazel_test_the_default_store_is_in_the_test_tmpdir() {
    let dir = project();
    let scratch = tempfile::tempdir().unwrap();
    let uncached = report(dir.path(), &[]);
    let result = run(skif()
        .current_dir(dir.path())
        .env("TEST_TMPDIR", scratch.path())
        .args(["--format", "json", "--cache"]));
    assert_eq!((result.stdout, result.code), uncached, "{}", result.stderr);
    let file = scratch.path().join("skif").join("modules.bin");
    assert!(fs::read(&file).unwrap().starts_with(b"SKIFMOD\0"));
    assert!(
        !dir.path().join("node_modules").join(".cache").join("skif").exists(),
        "nothing is written below the root"
    );

    // An explicit location wins over the scratch directory.
    let location = scratch.path().join("explicit");
    let explicit = run(skif().current_dir(dir.path()).env("TEST_TMPDIR", scratch.path()).args([
        "--format",
        "json",
        "--cache",
        "--cache-location",
        location.to_str().unwrap(),
    ]));
    assert_eq!((explicit.stdout, explicit.code), uncached, "{}", explicit.stderr);
    assert!(
        fs::read(location.join("modules.bin"))
            .unwrap()
            .starts_with(b"SKIFMOD\0")
    );
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

/// Three standalone packages whose entry files import `shared/lib.ts`, which lies outside all of them.
fn shared_closure() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (name, offset) in [("a", 1), ("b", 2), ("c", 3)] {
        let root = dir.path().join(name);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("package.json"),
            format!(r#"{{"name": "{name}", "version": "1.0.0", "main": "index.ts"}}"#),
        )
        .unwrap();
        fs::write(
            root.join("index.ts"),
            format!("import {{ shared }} from '../shared/lib';\nexport const {name} = shared + {offset};\n"),
        )
        .unwrap();
    }
    fs::create_dir_all(dir.path().join("shared")).unwrap();
    fs::write(dir.path().join("shared").join("lib.ts"), "export const shared = 1;\n").unwrap();
    dir
}

/// The module cache counts summed over the `js module cache <root>: H hits, M misses, W waits` lines of
/// `--performance`: the lookups served without parsing (hits, and waits for another root's parse) and the parses. The
/// counters are shared by the roots linted in parallel, so only the sums are exact, and whether a lookup of a file
/// another root is parsing hits or waits depends on timing.
fn module_cache_counts(stderr: &str) -> (usize, usize) {
    stderr
        .lines()
        .filter(|line| line.contains("js module cache "))
        .map(|line| {
            let counts: Vec<usize> = line
                .rsplit(": ")
                .next()
                .unwrap()
                .split(", ")
                .map(|count| count.split_once(' ').unwrap().0.parse().unwrap())
                .collect();
            let [hits, misses, waits] = counts.as_slice() else {
                panic!("not three counts: {line}");
            };
            (hits + waits, *misses)
        })
        .fold((0, 0), |(served, parsed), (s, p)| (served + s, parsed + p))
}

/// The JSON report, exit code and module cache counts of `skif -r --performance` on `dir` with `flags`.
fn recursive_run(dir: &Path, flags: &[&str]) -> ((String, i32), (usize, usize)) {
    let result = run(skif()
        .current_dir(dir)
        .args(["--recursive", "--performance", "--format", "json"])
        .args(flags));
    assert!(matches!(result.code, 0 | 1), "{}", result.stderr);
    let counts = module_cache_counts(&result.stderr);
    ((result.stdout, result.code), counts)
}

#[test]
fn a_recursive_run_parses_a_file_several_roots_import_once() {
    let dir = shared_closure();
    let (report, counts) = recursive_run(dir.path(), &[]);
    assert_eq!(root_count(&report.0), 3, "{}", report.0);
    // Only the shared file goes through the cache of the run, parsed once and served to the two other roots; each
    // entry file is its root's own and is parsed outside it.
    assert_eq!(counts, (2, 1));

    // `--cache` holds every file and is shared the same way; a warm run parses nothing.
    let store = tempfile::tempdir().unwrap();
    let flags = ["--cache", "--cache-location", store.path().to_str().unwrap()];
    let (cold, cold_counts) = recursive_run(dir.path(), &flags);
    assert_eq!(cold_counts, (2, 4));
    let (warm, warm_counts) = recursive_run(dir.path(), &flags);
    assert_eq!(warm_counts, (6, 0));
    assert_eq!(cold, report);
    assert_eq!(warm, report);
}

fn root_count(report: &str) -> usize {
    let report: serde_json::Value = serde_json::from_str(report).unwrap();
    report["roots"].as_array().unwrap().len()
}
