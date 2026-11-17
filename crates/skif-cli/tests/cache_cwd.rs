//! The library entry point of the npm package with an empty cache location (ADR 0007). The test changes the
//! working directory of its process, so it is the only test of this file.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::fs;
use std::path::PathBuf;

use skif_cli::{LintCache, lint_to_json_cached};

/// `lint(path, { cache: { location: "" } })`: an empty location would be the working directory, where the store
/// would write its `.gitignore` of `*` over the project's.
#[test]
fn an_empty_location_from_a_host_is_refused_and_the_working_directory_is_untouched() {
    const IGNORE: &str = "node_modules\n# the project's own\n";
    let dir = support::fixture("js");
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::write(root.join(".gitignore"), IGNORE).unwrap();
    std::env::set_current_dir(&root).unwrap();
    let before: Vec<PathBuf> = entries(&root);

    let cache = LintCache::default().with_location(PathBuf::new());
    let error = lint_to_json_cached(&root, &skif::ConfigOverrides::default(), false, &cache).unwrap_err();
    assert!(error.to_string().contains("the cache location is empty"), "{error}");
    assert_eq!(fs::read_to_string(root.join(".gitignore")).unwrap(), IGNORE);
    assert_eq!(entries(&root), before);
}

fn entries(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().path()).collect();
    entries.sort();
    entries
}
