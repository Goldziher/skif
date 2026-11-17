//! The cache store of the command line (ADR 0007): on by default, and never a changed report or exit code.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use assert_cmd::Command;
use std::fs;
use std::path::{Path, PathBuf};

use support::{Run, fixture, run};

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

/// The files of [`project`] a run parses: the entry file and what it reaches; the orphan is reported unparsed.
const SOURCES: u64 = 4;

/// The binary with the cache as a user finds it: on, with no variable of its own set, and the per-user cache
/// directory in `home`, so nothing is written outside the test's directories.
fn skif(home: &Path) -> Command {
    let mut command = support::skif();
    command
        .env_remove("SKIF_CACHE")
        .env("HOME", home)
        .env("XDG_CACHE_HOME", home.join("xdg"))
        .env("LOCALAPPDATA", home.join("xdg"));
    command
}

/// The JSON report and exit code of a run.
fn outcome(result: &Run) -> (String, i32) {
    assert!(matches!(result.code, 0 | 1), "{}", result.stderr);
    (result.stdout.clone(), result.code)
}

/// The JSON report and exit code of `skif --format json` on `dir` without a cache.
fn uncached(dir: &Path) -> (String, i32) {
    let result = run(support::skif()
        .current_dir(dir)
        .args(["--format", "json", "--no-cache"]));
    assert!(!result.stderr.contains("warning"), "{}", result.stderr);
    outcome(&result)
}

/// `skif --format json --debug` on `dir` with `flags` and the cache on.
fn cached(dir: &Path, home: &Path, flags: &[&str]) -> Run {
    run(skif(home)
        .current_dir(dir)
        .args(["--format", "json", "--debug"])
        .args(flags))
}

/// The hits, misses and bytes written of the `modules` namespace, from the `--debug` line; `None` when the run
/// cached nothing.
fn counters(stderr: &str) -> Option<(u64, u64, u64)> {
    let line = stderr
        .lines()
        .find_map(|line| line.strip_prefix("skif debug: cache modules: "))?;
    let numbers: Vec<u64> = line
        .split(", ")
        .map(|part| part.split_once(' ').unwrap().0.parse().unwrap())
        .collect();
    let [hits, misses, bytes] = numbers.as_slice() else {
        panic!("not three counts: {line}");
    };
    Some((*hits, *misses, *bytes))
}

/// Every `modules.bin` below `location`.
fn stores(location: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(location) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .map(|entry| entry.unwrap().path().join("modules.bin"))
        .filter(|file| file.is_file())
        .collect();
    files.sort();
    files
}

/// The one `modules.bin` below `location`.
fn store(location: &Path) -> PathBuf {
    let files = stores(location);
    assert_eq!(files.len(), 1, "{files:?} in {}", location.display());
    files[0].clone()
}

fn no_warning(result: &Run) {
    assert!(!result.stderr.contains("warning"), "{}", result.stderr);
}

#[test]
fn no_cache_a_cold_cache_and_a_warm_cache_print_the_same_report() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    let location = home.path().join("nested").join("cache");
    let flags = ["--cache-location", location.to_str().unwrap()];

    let uncached = uncached(dir.path());
    assert!(
        uncached.0.contains("unused-exports") || uncached.0.contains("unused-files"),
        "{}",
        uncached.0
    );
    let cold = cached(dir.path(), home.path(), &flags);
    no_warning(&cold);
    assert_eq!(outcome(&cold), uncached);
    let (hits, misses, bytes) = counters(&cold.stderr).unwrap();
    assert_eq!((hits, misses), (0, SOURCES), "{}", cold.stderr);
    assert!(bytes > 0);
    let file = store(&location);
    let written = fs::read(&file).unwrap();
    assert!(written.starts_with(b"SKIFSTOR"));
    // The directories skif created are tagged and ignored.
    for created in [location.as_path(), file.parent().unwrap()] {
        assert!(created.join("CACHEDIR.TAG").is_file(), "{}", created.display());
        assert_eq!(fs::read_to_string(created.join(".gitignore")).unwrap(), "*\n");
    }

    // A warm run hits every entry, prints the same bytes and rewrites nothing.
    let warm = cached(dir.path(), home.path(), &flags);
    no_warning(&warm);
    assert_eq!(outcome(&warm), uncached);
    assert_eq!(counters(&warm.stderr), Some((SOURCES, 0, 0)), "{}", warm.stderr);
    assert_eq!(fs::read(&file).unwrap(), written);

    // An edit misses once and is stored; the report follows the edit.
    fs::write(
        dir.path().join("packages").join("app").join("util.ts"),
        "export const used = () => 1;\nexport const unused = 2;\nexport const another = 3;\n",
    )
    .unwrap();
    let edited = self::uncached(dir.path());
    assert_ne!(edited, uncached);
    let after = cached(dir.path(), home.path(), &flags);
    assert_eq!(outcome(&after), edited);
    let (hits, misses, _) = counters(&after.stderr).unwrap();
    assert_eq!((hits, misses), (SOURCES - 1, 1), "{}", after.stderr);
    assert!(fs::read(&file).unwrap() != written, "the edited file was not stored");
    let again = cached(dir.path(), home.path(), &flags);
    assert_eq!(outcome(&again), edited);
    assert_eq!(counters(&again.stderr), Some((SOURCES, 0, 0)), "{}", again.stderr);
}

#[test]
fn a_corrupted_entry_is_a_miss_and_a_damaged_store_is_empty() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    let location = home.path().join("cache");
    let flags = ["--cache-location", location.to_str().unwrap()];
    let uncached = uncached(dir.path());
    assert_eq!(outcome(&cached(dir.path(), home.path(), &flags)), uncached);
    let file = store(&location);
    let written = fs::read(&file).unwrap();

    // One flipped byte inside the last record: that entry no longer opens, the others still hit.
    let mut flipped = written.clone();
    let last = flipped.len() - 1;
    flipped[last] ^= 0x01;
    fs::write(&file, &flipped).unwrap();
    let one = cached(dir.path(), home.path(), &flags);
    no_warning(&one);
    assert_eq!(outcome(&one), uncached);
    let (hits, misses, bytes) = counters(&one.stderr).unwrap();
    assert_eq!((hits, misses), (SOURCES - 1, 1), "{}", one.stderr);
    assert!(bytes > 0, "the entry was not written again");
    assert_eq!(fs::read(&file).unwrap(), written, "the store was not repaired");

    // A file that is not a store, and one cut short, are empty stores, said only under --debug.
    for damaged in [b"garbage".to_vec(), written[..written.len() / 2].to_vec()] {
        fs::write(&file, &damaged).unwrap();
        let all = cached(dir.path(), home.path(), &flags);
        no_warning(&all);
        assert_eq!(outcome(&all), uncached);
        assert_eq!(
            counters(&all.stderr).map(|(hits, misses, _)| (hits, misses)),
            Some((0, SOURCES))
        );
        assert!(all.stderr.contains("ignoring the cache file"), "{}", all.stderr);
        assert_eq!(fs::read(&file).unwrap(), written);
    }
}

#[test]
fn the_cache_is_on_by_default_below_an_existing_node_modules() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("node_modules")).unwrap();
    let uncached = uncached(dir.path());
    // Started in a package, the store is still the root's.
    let cold = run(skif(home.path())
        .current_dir(dir.path().join("packages").join("app"))
        .args(["--format", "json"]));
    no_warning(&cold);
    assert_eq!(outcome(&cold), uncached);
    let location = dir.path().join("node_modules").join(".cache").join("skif");
    assert!(fs::read(store(&location)).unwrap().starts_with(b"SKIFSTOR"));
    let warm = cached(dir.path(), home.path(), &[]);
    assert_eq!(outcome(&warm), uncached);
    assert_eq!(counters(&warm.stderr), Some((SOURCES, 0, 0)), "{}", warm.stderr);
    assert!(!home.path().join("xdg").exists(), "the user cache directory was used");
}

#[test]
fn without_node_modules_the_default_store_is_in_the_user_cache_directory() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    let uncached = uncached(dir.path());
    let cold = cached(dir.path(), home.path(), &[]);
    no_warning(&cold);
    assert_eq!(outcome(&cold), uncached);
    assert!(!dir.path().join("node_modules").exists(), "skif created node_modules");
    let location = home.path().join("xdg").join("skif");
    assert!(store(&location).is_file());
    let warm = cached(dir.path(), home.path(), &[]);
    assert_eq!(counters(&warm.stderr), Some((SOURCES, 0, 0)), "{}", warm.stderr);
}

#[test]
fn the_flag_the_variable_and_the_configuration_turn_the_cache_off() {
    let home = tempfile::tempdir().unwrap();
    let written = |dir: &Path| dir.join("node_modules").join(".cache").exists() || home.path().join("xdg").exists();
    let fresh = || {
        let dir = project();
        fs::create_dir(dir.path().join("node_modules")).unwrap();
        dir
    };

    let dir = fresh();
    let uncached = uncached(dir.path());
    let flag = cached(dir.path(), home.path(), &["--no-cache"]);
    assert_eq!(outcome(&flag), uncached);
    assert_eq!(counters(&flag.stderr), None);
    assert!(!written(dir.path()));

    let variable = run(skif(home.path())
        .current_dir(dir.path())
        .env("SKIF_CACHE", "0")
        .args(["--format", "json", "--debug"]));
    assert_eq!(outcome(&variable), uncached);
    assert_eq!(counters(&variable.stderr), None);
    assert!(!written(dir.path()));

    fs::write(dir.path().join("skif.toml"), "[cache]\nenabled = false\n").unwrap();
    let configured = cached(dir.path(), home.path(), &[]);
    no_warning(&configured);
    assert_eq!(outcome(&configured), uncached);
    assert_eq!(counters(&configured.stderr), None);
    assert!(!written(dir.path()));

    // The variable beats the configuration, and the flag beats the variable.
    let variable_on = run(skif(home.path())
        .current_dir(dir.path())
        .env("SKIF_CACHE", "1")
        .args(["--format", "json", "--debug"]));
    assert_eq!(outcome(&variable_on), uncached);
    assert!(counters(&variable_on.stderr).is_some(), "{}", variable_on.stderr);
    let flag_off = run(skif(home.path()).current_dir(dir.path()).env("SKIF_CACHE", "1").args([
        "--format",
        "json",
        "--debug",
        "--no-cache",
    ]));
    assert_eq!(counters(&flag_off.stderr), None);
    let flag_on = run(skif(home.path())
        .current_dir(dir.path())
        .env("SKIF_CACHE", "0")
        .args(["--format", "json", "--debug", "--cache"]));
    assert_eq!(outcome(&flag_on), uncached);
    assert!(counters(&flag_on.stderr).is_some(), "{}", flag_on.stderr);

    let both = run(skif(home.path())
        .current_dir(dir.path())
        .args(["--cache", "--no-cache"]));
    assert_eq!(both.code, 2, "{}", both.stderr);
}

#[test]
fn the_location_is_the_flag_then_the_variable_then_the_configuration() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    let (from_flag, from_env) = (home.path().join("flag"), home.path().join("env"));
    // Relative to the configuration file.
    fs::write(
        dir.path().join("skif.toml"),
        "[cache]\nlocation = \".skif-cache\"\nmax-size = \"64MiB\"\n",
    )
    .unwrap();
    let from_config = dir.path().join(".skif-cache");
    let uncached = uncached(dir.path());

    let configured = cached(&dir.path().join("packages").join("app"), home.path(), &[]);
    no_warning(&configured);
    assert_eq!(outcome(&configured), uncached);
    assert!(store(&from_config).is_file());

    let variable = run(skif(home.path())
        .current_dir(dir.path())
        .env("SKIF_CACHE_DIR", &from_env)
        .args(["--format", "json"]));
    assert_eq!(outcome(&variable), uncached);
    assert!(store(&from_env).is_file());

    let flag = run(skif(home.path())
        .current_dir(dir.path())
        .env("SKIF_CACHE_DIR", home.path().join("unused"))
        .args(["--format", "json", "--cache-location"])
        .arg(&from_flag));
    assert_eq!(outcome(&flag), uncached);
    assert!(store(&from_flag).is_file());
    assert!(!home.path().join("unused").exists());
    assert!(!home.path().join("xdg").exists());
}

#[test]
fn the_size_cap_is_the_flag_then_the_variable_then_the_configuration() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    let location = home.path().join("cache");
    let uncached = uncached(dir.path());
    let lint = |variable: Option<&str>, flags: &[&str]| {
        let _ = fs::remove_dir_all(&location);
        let mut command = skif(home.path());
        command
            .current_dir(dir.path())
            .args(["--format", "json", "--cache-location"])
            .arg(&location)
            .args(flags);
        if let Some(size) = variable {
            command.env("SKIF_CACHE_MAX_SIZE", size);
        }
        let result = run(&mut command);
        assert_eq!(outcome(&result), uncached, "{}", result.stderr);
        fs::metadata(store(&location)).unwrap().len()
    };
    let full = lint(None, &[]);
    // A cap too small for any entry keeps the store's header only; the report is the same.
    let capped = lint(None, &["--cache-max-size", "64"]);
    assert!(capped < full / 2, "{capped} of {full}");
    assert_eq!(lint(Some("64"), &[]), capped);
    assert_eq!(lint(Some("64"), &["--cache-max-size", "1GiB"]), full);
    fs::write(dir.path().join("skif.toml"), "[cache]\nmax-size = 64\n").unwrap();
    assert_eq!(lint(None, &[]), capped);
    assert_eq!(lint(Some("1GiB"), &[]), full);

    let bad = run(skif(home.path())
        .current_dir(dir.path())
        .args(["--cache-max-size", "lots"]));
    assert_eq!(bad.code, 2, "{}", bad.stderr);
    let ignored = run(skif(home.path())
        .current_dir(dir.path())
        .env("SKIF_CACHE_MAX_SIZE", "lots")
        .args(["--format", "json", "--no-cache"]));
    assert_eq!(outcome(&ignored), uncached);
    assert!(
        ignored.stderr.contains("warning: ignoring SKIF_CACHE_MAX_SIZE"),
        "{}",
        ignored.stderr
    );
}

#[test]
fn only_the_root_configuration_sets_the_cache() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("shared.toml"), "[cache]\nenabled = false\n").unwrap();
    fs::write(dir.path().join("skif.toml"), "extends = [\"./shared.toml\"]\n").unwrap();
    // The warning is printed whether or not the run caches.
    let uncached = outcome(&run(support::skif().current_dir(dir.path()).args([
        "--format",
        "json",
        "--no-cache",
    ])));
    let result = cached(dir.path(), home.path(), &[]);
    assert_eq!(outcome(&result), uncached);
    assert!(
        result.stderr.contains("warning: ignoring [cache] in") && result.stderr.contains("shared.toml"),
        "{}",
        result.stderr
    );
    // The extended file did not turn the cache off.
    assert!(counters(&result.stderr).is_some(), "{}", result.stderr);
}

#[test]
fn in_ci_the_default_store_is_read_and_not_written() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("node_modules")).unwrap();
    let location = dir.path().join("node_modules").join(".cache").join("skif");
    let uncached = uncached(dir.path());
    let ci = |flags: &[&str]| {
        run(skif(home.path())
            .current_dir(dir.path())
            .env("CI", "true")
            .args(["--format", "json", "--debug"])
            .args(flags))
    };
    let cold = ci(&[]);
    no_warning(&cold);
    assert_eq!(outcome(&cold), uncached);
    assert_eq!(
        counters(&cold.stderr).map(|(hits, misses, _)| (hits, misses)),
        Some((0, SOURCES))
    );
    assert!(!location.exists(), "CI wrote the default store");

    // A store something restored is read.
    assert_eq!(outcome(&cached(dir.path(), home.path(), &[])), uncached);
    let before = fs::read(store(&location)).unwrap();
    let warm = ci(&[]);
    assert_eq!(outcome(&warm), uncached);
    assert_eq!(counters(&warm.stderr), Some((SOURCES, 0, 0)), "{}", warm.stderr);
    assert_eq!(fs::read(store(&location)).unwrap(), before);

    // A location given explicitly is written: something restores it.
    let explicit = home.path().join("restored");
    let named = ci(&["--cache-location", explicit.to_str().unwrap()]);
    assert_eq!(outcome(&named), uncached);
    assert!(store(&explicit).is_file());
}

#[test]
fn under_bazel_test_the_default_store_is_in_the_test_tmpdir() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("node_modules")).unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let uncached = uncached(dir.path());
    let result = run(skif(home.path())
        .current_dir(dir.path())
        .env("TEST_TMPDIR", scratch.path())
        .args(["--format", "json"]));
    assert_eq!(outcome(&result), uncached, "{}", result.stderr);
    assert!(store(&scratch.path().join("skif")).is_file());
    assert!(
        !dir.path().join("node_modules").join(".cache").exists(),
        "nothing is written below the root"
    );

    // An explicit location wins over the scratch directory.
    let location = scratch.path().join("explicit");
    let explicit = run(skif(home.path())
        .current_dir(dir.path())
        .env("TEST_TMPDIR", scratch.path())
        .args(["--format", "json", "--cache-location", location.to_str().unwrap()]));
    assert_eq!(outcome(&explicit), uncached, "{}", explicit.stderr);
    assert!(store(&location).is_file());
}

#[test]
fn a_store_that_cannot_be_written_is_a_warning_only_when_it_was_asked_for() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    let uncached = uncached(dir.path());
    // A file where the cache directory should be.
    let blocked = dir.path().join("blocked");
    fs::write(&blocked, "").unwrap();
    let asked = run(skif(home.path()).current_dir(dir.path()).args([
        "--format",
        "json",
        "--cache-location",
        blocked.to_str().unwrap(),
    ]));
    assert_eq!(outcome(&asked), uncached);
    assert!(
        asked.stderr.contains("warning: cannot write the cache in"),
        "{}",
        asked.stderr
    );

    // The same failure at the default location is silent, and one line under --debug.
    fs::create_dir(dir.path().join("node_modules")).unwrap();
    fs::write(dir.path().join("node_modules").join(".cache"), "").unwrap();
    let silent = run(skif(home.path()).current_dir(dir.path()).args(["--format", "json"]));
    assert_eq!(outcome(&silent), uncached);
    assert_eq!(
        silent.stderr, "",
        "a default cache that cannot be written said something"
    );
    let debug = cached(dir.path(), home.path(), &[]);
    assert_eq!(outcome(&debug), uncached);
    no_warning(&debug);
    assert!(
        debug.stderr.contains("skif debug: cannot write the cache in"),
        "{}",
        debug.stderr
    );
}

#[cfg(unix)]
#[test]
fn a_read_only_location_degrades_to_a_cold_run() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = project();
    let home = tempfile::tempdir().unwrap();
    let location = home.path().join("cache");
    let flags = ["--cache-location", location.to_str().unwrap()];
    let uncached = uncached(dir.path());
    assert_eq!(outcome(&cached(dir.path(), home.path(), &flags)), uncached);
    let file = store(&location);
    let store_dir = file.parent().unwrap();
    // Neither readable nor writable: the run finds nothing, parses everything and cannot store it.
    fs::remove_file(&file).unwrap();
    fs::set_permissions(store_dir, fs::Permissions::from_mode(0o555)).unwrap();
    let result = cached(dir.path(), home.path(), &flags);
    fs::set_permissions(store_dir, fs::Permissions::from_mode(0o755)).unwrap();
    if fs::metadata(&file).is_ok() {
        // Running as root: permissions do not bind, and there is nothing to degrade.
        return;
    }
    assert_eq!(outcome(&result), uncached);
    assert_eq!(
        counters(&result.stderr).map(|(hits, misses, _)| (hits, misses)),
        Some((0, SOURCES))
    );
    assert!(
        result.stderr.contains("warning: cannot write the cache in"),
        "{}",
        result.stderr
    );
}

/// Makes `worktree` a linked worktree of the repository at `top`, laid out as `git worktree add` leaves them.
fn link_worktree(top: &Path, worktree: &Path) {
    let private = top.join(".git").join("worktrees").join("linked");
    fs::create_dir_all(&private).unwrap();
    fs::write(private.join("commondir"), "../..\n").unwrap();
    fs::write(worktree.join(".git"), format!("gitdir: {}\n", private.display())).unwrap();
}

#[test]
fn worktrees_of_one_repository_share_their_entries_and_two_repositories_keep_separate_stores() {
    let home = tempfile::tempdir().unwrap();
    let location = home.path().join("cache");
    let flags = ["--cache-location", location.to_str().unwrap()];
    let (one, linked, other) = (project(), project(), project());
    fs::create_dir(one.path().join(".git")).unwrap();
    link_worktree(one.path(), linked.path());
    fs::create_dir(other.path().join(".git")).unwrap();
    // The other repository differs in one file, so its run would prune the first's entry if they shared a store.
    fs::write(
        other.path().join("packages").join("app").join("util.ts"),
        "export const used = () => 2;\nexport const unused = 3;\n",
    )
    .unwrap();

    let first = cached(one.path(), home.path(), &flags);
    assert_eq!(
        counters(&first.stderr).map(|(hits, misses, _)| (hits, misses)),
        Some((0, SOURCES))
    );
    let second = cached(linked.path(), home.path(), &flags);
    assert_eq!(counters(&second.stderr), Some((SOURCES, 0, 0)), "{}", second.stderr);
    assert_eq!(outcome(&second), outcome(&first));
    assert_eq!(stores(&location).len(), 1);

    // A second repository linted at its root gets a store of its own and prunes nothing of the first.
    let foreign = cached(other.path(), home.path(), &flags);
    assert_eq!(
        counters(&foreign.stderr).map(|(hits, misses, _)| (hits, misses)),
        Some((0, SOURCES)),
        "{}",
        foreign.stderr
    );
    assert_eq!(stores(&location).len(), 2);
    let again = cached(one.path(), home.path(), &flags);
    assert_eq!(counters(&again.stderr), Some((SOURCES, 0, 0)), "{}", again.stderr);

    // `skif cache clean` in one repository leaves the other's store.
    let cleaned = run(skif(home.path())
        .current_dir(other.path())
        .args(["cache", "clean", "--format", "json"])
        .args(flags));
    assert_eq!(cleaned.json()["stores"], 1, "{}", cleaned.stderr);
    assert_eq!(stores(&location).len(), 1);
    let kept = cached(one.path(), home.path(), &flags);
    assert_eq!(counters(&kept.stderr), Some((SOURCES, 0, 0)), "{}", kept.stderr);
}

/// What a file a cache location must never replace holds, and the file itself in `dir`.
const IGNORE: &str = "node_modules\n# the project's own\n";

fn with_ignore_file(dir: &Path) -> PathBuf {
    let ignore = dir.join(".gitignore");
    fs::write(&ignore, IGNORE).unwrap();
    ignore
}

/// The project is as it was: its `.gitignore` byte for byte, and no file of a store next to it.
fn untouched(dir: &Path, ignore: &Path) {
    assert_eq!(fs::read_to_string(ignore).unwrap(), IGNORE);
    assert!(
        !dir.join("CACHEDIR.TAG").exists(),
        "a cache tag was written into the project"
    );
    let hex = |name: &str| name.len() == 32 && name.bytes().all(|byte| byte.is_ascii_hexdigit());
    let stray: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| hex(name))
        .collect();
    assert!(stray.is_empty(), "store directories in the project: {stray:?}");
}

#[test]
fn an_empty_location_flag_is_a_usage_error_and_touches_nothing() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    let ignore = with_ignore_file(dir.path());
    for arguments in [
        vec!["--format", "json", "--cache-location", ""],
        vec!["--cache-location="],
        vec!["cache", "stats", "--cache-location", ""],
        vec!["cache", "clean", "--all", "--cache-location", ""],
    ] {
        let result = run(skif(home.path()).current_dir(dir.path()).args(&arguments));
        assert_eq!(result.code, 2, "{arguments:?}: {}", result.stderr);
        // The error is on stderr, or the JSON error document of the run on stdout.
        let said = format!("{}{}", result.stdout, result.stderr);
        assert!(said.contains("the cache location is empty"), "{arguments:?}: {said}");
        untouched(dir.path(), &ignore);
    }
    // An empty variable is an unset one: the run caches in its default location.
    let unset = run(skif(home.path())
        .current_dir(dir.path())
        .env("SKIF_CACHE_DIR", "")
        .args(["--format", "json"]));
    assert!(matches!(unset.code, 0 | 1), "{}", unset.stderr);
    untouched(dir.path(), &ignore);
}

#[test]
fn a_configured_location_must_stay_inside_the_directory_of_its_file() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    let ignore = with_ignore_file(dir.path());
    let outside = home.path().join("outside");
    let absolute = format!("[cache]\nlocation = {:?}\n", outside.display().to_string());
    for (config, reason) in [
        ("[cache]\nlocation = \"\"\n", "it is empty"),
        (absolute.as_str(), "not an absolute path"),
        ("[cache]\nlocation = \"../outside\"\n", "must stay inside"),
        ("[cache]\nlocation = \"a/../../outside\"\n", "must stay inside"),
    ] {
        fs::write(dir.path().join("skif.toml"), config).unwrap();
        let result = cached(dir.path(), home.path(), &[]);
        assert_eq!(result.code, 2, "{config}: {}", result.stderr);
        // The JSON error document of the run.
        for part in ["skif.toml", "cache.location", reason] {
            assert!(
                result.stdout.contains(part),
                "{config}: no {part:?} in {}",
                result.stdout
            );
        }
        untouched(dir.path(), &ignore);
        assert!(!outside.exists() && !dir.path().parent().unwrap().join("outside").exists());
    }
    // Below the file's directory, by whatever spelling, it is used.
    fs::write(
        dir.path().join("skif.toml"),
        "[cache]\nlocation = \"a/../.skif-cache\"\n",
    )
    .unwrap();
    let inside = cached(dir.path(), home.path(), &[]);
    no_warning(&inside);
    assert!(store(&dir.path().join(".skif-cache")).is_file());
}

#[cfg(unix)]
#[test]
fn a_configured_location_that_a_link_leads_out_of_the_project_is_ignored() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    let outside = home.path().join("outside");
    fs::create_dir(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, dir.path().join("link")).unwrap();
    fs::write(dir.path().join("skif.toml"), "[cache]\nlocation = \"link/store\"\n").unwrap();
    let result = cached(dir.path(), home.path(), &[]);
    // The warning is printed whether or not the run caches.
    let uncached = outcome(&run(support::skif().current_dir(dir.path()).args([
        "--format",
        "json",
        "--no-cache",
    ])));
    assert_eq!(outcome(&result), uncached);
    assert!(
        result.stderr.contains("warning: ignoring cache.location in") && result.stderr.contains("skif.toml"),
        "{}",
        result.stderr
    );
    assert_eq!(
        fs::read_dir(&outside).unwrap().count(),
        0,
        "the store was written through the link"
    );
    // The run cached in its default location instead.
    assert!(counters(&result.stderr).is_some(), "{}", result.stderr);
}

#[test]
fn a_configured_location_does_not_make_a_ci_run_write() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("skif.toml"), "[cache]\nlocation = \".skif-cache\"\n").unwrap();
    let location = dir.path().join(".skif-cache");
    let ci = run(skif(home.path())
        .current_dir(dir.path())
        .env("CI", "true")
        .args(["--format", "json", "--debug"]));
    assert_eq!(outcome(&ci), uncached(dir.path()));
    assert!(ci.stderr.contains("(read only)"), "{}", ci.stderr);
    assert!(!location.exists(), "a CI run wrote the store its configuration names");
    // Outside CI the same configuration writes it, and CI then reads it.
    cached(dir.path(), home.path(), &[]);
    assert!(store(&location).is_file());
    let warm = run(skif(home.path())
        .current_dir(dir.path())
        .env("CI", "true")
        .args(["--format", "json", "--debug"]));
    assert_eq!(counters(&warm.stderr), Some((SOURCES, 0, 0)), "{}", warm.stderr);
}

#[test]
fn a_configured_cap_never_evicts_the_stores_of_other_projects() {
    let home = tempfile::tempdir().unwrap();
    let shared = home.path().join("shared");
    let (victim, hostile) = (project(), project());
    let lint = |dir: &Path, flags: &[&str]| {
        let result = run(skif(home.path())
            .current_dir(dir)
            .env("SKIF_CACHE_DIR", &shared)
            .args(["--format", "json", "--debug"])
            .args(flags));
        assert!(matches!(result.code, 0 | 1), "{}", result.stderr);
        result
    };
    lint(victim.path(), &[]);
    let [kept] = stores(&shared).try_into().unwrap();
    let before = fs::read(&kept).unwrap();

    // A checkout whose configuration caps the cache at nothing empties its own store only.
    fs::write(hostile.path().join("skif.toml"), "[cache]\nmax-size = 0\n").unwrap();
    lint(hostile.path(), &[]);
    assert_eq!(
        fs::read(&kept).unwrap(),
        before,
        "a configured cap reached another project's store"
    );
    assert_eq!(stores(&shared).len(), 2);
    let warm = lint(victim.path(), &[]);
    assert_eq!(counters(&warm.stderr), Some((SOURCES, 0, 0)), "{}", warm.stderr);

    // The user's own cap is the location's: it drops the other project's store.
    fs::remove_file(hostile.path().join("skif.toml")).unwrap();
    lint(hostile.path(), &["--cache-max-size", "0"]);
    assert!(!kept.exists(), "the flag's cap left the other store");
}

#[test]
fn a_namespace_file_over_the_cap_is_ignored_with_a_note() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    let location = home.path().join("cache");
    let flags = ["--cache-location", location.to_str().unwrap()];
    cached(dir.path(), home.path(), &flags);
    let capped = run(skif(home.path())
        .current_dir(dir.path())
        .args(["--format", "json", "--debug", "--cache-max-size", "64"])
        .args(flags));
    assert_eq!(outcome(&capped), uncached(dir.path()));
    assert!(
        capped.stderr.contains("larger than the cap of 64 bytes"),
        "{}",
        capped.stderr
    );
    assert_eq!(
        counters(&capped.stderr).map(|(hits, misses, _)| (hits, misses)),
        Some((0, SOURCES))
    );
}

#[test]
fn cache_stats_and_clean_show_and_remove_the_store() {
    let dir = project();
    let home = tempfile::tempdir().unwrap();
    let location = home.path().join("cache");
    let flags = ["--cache-location", location.to_str().unwrap()];
    let cache = |arguments: &[&str]| {
        let result = run(skif(home.path())
            .current_dir(dir.path())
            .arg("cache")
            .args(arguments)
            .args(flags));
        assert_eq!(result.code, 0, "{}", result.stderr);
        result
    };
    let empty = cache(&["stats", "--format", "json"]).json();
    assert_eq!(empty["stores"], serde_json::json!([]));
    assert!(!location.exists(), "stats created the location");

    cached(dir.path(), home.path(), &flags);
    let stats = cache(&["stats", "--format", "json"]).json();
    assert_eq!(stats["stores"].as_array().unwrap().len(), 1, "{stats}");
    let found = &stats["stores"][0];
    assert_eq!(found["current"], true);
    assert_eq!(found["readable"], true);
    assert_eq!(found["namespaces"][0]["name"], "modules");
    assert_eq!(found["namespaces"][0]["entries"], SOURCES);
    assert_eq!(stats["bytes"], found["bytes"]);
    let human = cache(&["stats"]).stdout;
    assert!(human.contains("(this project)"), "{human}");
    assert!(human.contains(&format!("modules: {SOURCES} entries")), "{human}");
    assert!(human.contains("total: 1 store"), "{human}");

    let cleaned = cache(&["clean", "--format", "json"]).json();
    assert_eq!(cleaned["stores"], 1);
    assert_eq!(cleaned["bytes"], stats["bytes"]);
    assert!(!location.exists(), "clean left the location behind");
    assert_eq!(cache(&["clean", "--format", "json"]).json()["stores"], 0);
    assert!(cache(&["clean", "--all"]).stdout.starts_with("removed 0 stores"));
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
    let result = run(support::skif()
        .current_dir(dir)
        .env_remove("SKIF_CACHE")
        .args(["--recursive", "--performance", "--format", "json"])
        .args(flags));
    assert!(matches!(result.code, 0 | 1), "{}", result.stderr);
    let counts = module_cache_counts(&result.stderr);
    ((result.stdout, result.code), counts)
}

#[test]
fn a_recursive_run_parses_a_file_several_roots_import_once() {
    let dir = shared_closure();
    let (report, counts) = recursive_run(dir.path(), &["--no-cache"]);
    assert_eq!(root_count(&report.0), 3, "{}", report.0);
    // Only the shared file goes through the cache of the run, parsed once and served to the two other roots; each
    // entry file is its root's own and is parsed outside it.
    assert_eq!(counts, (2, 1));

    // The cache store holds every file and is shared the same way; a warm run parses nothing.
    let store = tempfile::tempdir().unwrap();
    let flags = ["--cache-location", store.path().to_str().unwrap()];
    let (cold, cold_counts) = recursive_run(dir.path(), &flags);
    assert_eq!(cold_counts, (2, 4));
    let (warm, warm_counts) = recursive_run(dir.path(), &flags);
    assert_eq!(warm_counts, (6, 0));
    assert_eq!(cold, report);
    assert_eq!(warm, report);
}

/// The `modules` counters of `skif -r --debug` on `dir` with `flags`, and the exit code.
fn recursive_counters(dir: &Path, flags: &[&str]) -> (Option<(u64, u64, u64)>, i32) {
    let result = run(support::skif()
        .current_dir(dir)
        .env_remove("SKIF_CACHE")
        .args(["--recursive", "--debug", "--format", "json"])
        .args(flags));
    (counters(&result.stderr), result.code)
}

#[test]
fn a_cold_recursive_run_counts_one_miss_per_file() {
    let dir = shared_closure();
    let store = tempfile::tempdir().unwrap();
    let flags = ["--cache-location", store.path().to_str().unwrap()];
    // Three entry files and the file they share. The two roots that did not parse the shared file hit it or
    // wait for its parse, depending on timing; neither is a miss.
    let (cold, _) = recursive_counters(dir.path(), &flags);
    let (hits, misses, written) = cold.unwrap();
    assert_eq!(misses, 4);
    assert!(hits <= 2, "{hits} hits");
    assert!(written > 0);
    assert_eq!(recursive_counters(dir.path(), &flags).0, Some((6, 0, 0)));
}

#[test]
fn a_run_in_which_a_root_fails_drops_no_entry_of_the_store() {
    let dir = shared_closure();
    let store = tempfile::tempdir().unwrap();
    let flags = ["--cache-location", store.path().to_str().unwrap()];
    assert!(matches!(recursive_counters(dir.path(), &flags).1, 0 | 1));
    let [file] = stores(store.path()).try_into().unwrap();
    let complete = fs::metadata(&file).unwrap().len();

    // One root cannot be linted: the run fails, and what that root would have used stays.
    let config = dir.path().join("c").join("skif.toml");
    fs::write(&config, "[lint\n").unwrap();
    let (failed, code) = recursive_counters(dir.path(), &flags);
    assert_eq!(code, 2);
    assert_eq!(failed.map(|(_, misses, _)| misses), Some(0));
    assert_eq!(
        fs::metadata(&file).unwrap().len(),
        complete,
        "the failed run shrank the store"
    );

    fs::remove_file(&config).unwrap();
    assert_eq!(recursive_counters(dir.path(), &flags).0, Some((6, 0, 0)));
}

fn root_count(report: &str) -> usize {
    let report: serde_json::Value = serde_json::from_str(report).unwrap();
    report["roots"].as_array().unwrap().len()
}
