//! `[js] discovery = "auto"` gives the result of the directory walk on a real git work tree, whatever happened on disk
//! since the last `git add`; `git` lists unchanged tracked directories from the index. The tests need the `git` binary
//! and skip without it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use skif_config::{Config, JsDiscovery};
use skif_core::{CachedFs, FileSystem, FixSafety, OsFs};
use skif_js::JsProvider;
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

fn git(root: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn write(root: &Path, path: &str, text: &str) {
    let file = root.join(path);
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(file, text).unwrap();
}

/// A committed repository, or `None` when git is not available.
fn repository() -> Option<(tempfile::TempDir, PathBuf)> {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    write(
        &root,
        "package.json",
        r#"{"name":"app","private":true,"main":"src/index.ts"}"#,
    );
    write(
        &root,
        "src/index.ts",
        "import { used } from \"./used\";\nexport const run = used;\n",
    );
    write(&root, "src/used.ts", "export const used = 1;\n");
    write(&root, "src/dead.ts", "export const dead = 1;\n");
    let ready =
        git(&root, &["init", "-q"]) && git(&root, &["add", "-A"]) && git(&root, &["commit", "-q", "-m", "init"]);
    if !ready {
        eprintln!("git is not available, skipping");
        return None;
    }
    Some((dir, root))
}

fn provider() -> (Registry, Arc<CachedFs<OsFs>>) {
    let cache = Arc::new(CachedFs::new(OsFs));
    let fs: Arc<dyn FileSystem> = Arc::clone(&cache) as Arc<dyn FileSystem>;
    let registry = Registry::new(vec![Box::new(JsProvider::new(fs))]).unwrap();
    (registry, cache)
}

fn findings(registry: &Registry, root: &Path, discovery: JsDiscovery) -> Vec<String> {
    let mut config = Config::default();
    config.js.discovery = discovery;
    let workspace = Workspace::discover_with_fs(root, &config, &OsFs).unwrap();
    let mut found: Vec<String> = run_unsorted(registry, &workspace, &config, &RunOptions::default())
        .unwrap()
        .iter()
        .filter(|d| {
            matches!(
                d.rule.as_str(),
                "unused-files" | "unresolved-imports" | "unused-exports"
            )
        })
        .map(|d| format!("{} {}", d.rule.as_str(), d.location.path.display()))
        .collect();
    found.sort();
    found
}

fn same_as_the_walk(root: &Path) -> Vec<String> {
    let walked = findings(&provider().0, root, JsDiscovery::Walk);
    assert_eq!(findings(&provider().0, root, JsDiscovery::Auto), walked);
    walked
}

#[test]
fn a_clean_checkout_agrees_with_the_walk() {
    let Some((_dir, root)) = repository() else { return };
    assert_eq!(same_as_the_walk(&root), ["unused-files src/dead.ts"]);
}

#[test]
fn a_file_deleted_but_not_staged_is_neither_unused_nor_resolved() {
    let Some((_dir, root)) = repository() else { return };
    fs::remove_file(root.join("src/dead.ts")).unwrap();
    assert_eq!(same_as_the_walk(&root), Vec::<String>::new());
    fs::remove_file(root.join("src/used.ts")).unwrap();
    assert_eq!(same_as_the_walk(&root), ["unresolved-imports src/index.ts"]);
}

#[test]
fn untracked_files_and_directories_are_found() {
    let Some((_dir, root)) = repository() else { return };
    write(&root, "src/fresh.ts", "export const fresh = 1;\n");
    write(&root, "src/newdir/deep/x.ts", "export const x = 1;\n");
    let walked = same_as_the_walk(&root);
    assert!(walked.contains(&"unused-files src/fresh.ts".to_owned()), "{walked:?}");
    assert!(
        walked.contains(&"unused-files src/newdir/deep/x.ts".to_owned()),
        "{walked:?}"
    );
}

#[test]
fn an_untracked_file_older_than_the_last_index_write_is_found() {
    let Some((_dir, root)) = repository() else { return };
    write(
        &root,
        "src/extra.ts",
        "import { dead } from \"./dead\";\nexport const extra = dead;\n",
    );
    // Committing another file rewrites the index after `src/extra.ts` was created, so `src` is older than the index.
    write(&root, "notes.txt", "notes\n");
    assert!(git(&root, &["add", "notes.txt"]) && git(&root, &["commit", "-q", "-m", "notes"]));
    let walked = same_as_the_walk(&root);
    assert!(walked.contains(&"unused-files src/extra.ts".to_owned()), "{walked:?}");
}

#[test]
fn the_index_listing_sees_files_created_or_deleted_since_the_index_was_written() {
    let Some((_dir, root)) = repository() else { return };
    write(&root, "src/fresh.ts", "export const fresh = 1;\n");
    fs::remove_file(root.join("src/dead.ts")).unwrap();
    let indexed = findings(&provider().0, &root, JsDiscovery::Git);
    assert!(indexed.contains(&"unused-files src/fresh.ts".to_owned()), "{indexed:?}");
    assert!(!indexed.contains(&"unused-files src/dead.ts".to_owned()), "{indexed:?}");
}

#[test]
fn unused_files_listed_from_the_index_offer_no_deletion() {
    let Some((_dir, root)) = repository() else { return };
    let mut config = Config::default();
    config.js.discovery = JsDiscovery::Git;
    let workspace = Workspace::discover_with_fs(&root, &config, &OsFs).unwrap();
    let diagnostics = run_unsorted(&provider().0, &workspace, &config, &RunOptions::default()).unwrap();
    let unused: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == "unused-files")
        .collect();
    assert_eq!(unused.len(), 1, "{unused:?}");
    assert_eq!(unused[0].location.path, Path::new("src/dead.ts"));
    assert!(unused[0].fixes.is_empty(), "{:?}", unused[0].fixes);
    assert!(
        unused[0]
            .help
            .as_deref()
            .is_some_and(|help| help.contains("no fix deletes it") && help.contains("listed from the git index")),
        "{:?}",
        unused[0].help
    );

    config.js.discovery = JsDiscovery::Auto;
    let diagnostics = run_unsorted(&provider().0, &workspace, &config, &RunOptions::default()).unwrap();
    let unused: Vec<_> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == "unused-files")
        .collect();
    assert_eq!(unused.len(), 1, "{unused:?}");
    let titles: Vec<_> = unused[0]
        .fixes
        .iter()
        .map(|fix| (fix.title.as_str(), fix.safety))
        .collect();
    assert_eq!(titles, [("Delete `src/dead.ts`", FixSafety::Unsafe)]);
}

#[test]
fn a_second_pass_of_one_provider_sees_what_a_fix_deleted() {
    let Some((_dir, root)) = repository() else { return };
    let (registry, cache) = provider();
    assert_eq!(
        findings(&registry, &root, JsDiscovery::Auto),
        ["unused-files src/dead.ts"]
    );
    fs::remove_file(root.join("src/dead.ts")).unwrap();
    // The linter empties the cache between runs; the parsed index stays.
    cache.clear();
    assert_eq!(findings(&registry, &root, JsDiscovery::Auto), Vec::<String>::new());
}
