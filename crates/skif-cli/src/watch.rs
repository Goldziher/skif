//! `skif --watch`: lint, then lint again whenever a file below the root changes (ADR 0028).
//!
//! The watcher polls: it lists the files below the root with their size and modification time and compares the
//! list with the previous one. A run starts once the tree has been quiet for [`QUIET`], so a checkout or a
//! formatter that writes many files causes one run.
//!
//! Polling costs one walk of the tree, one `stat` per file, every interval; the interval grows to twice the scan's
//! own duration, so on a large repository the watcher spends at most about a third of one core scanning.
//!
//! The reports the run writes (`--output`, `--format FORMAT=FILE`) and a `--cache-location` store are left out of the
//! scan: each run rewrites them, and seeing that write would start the next run, forever.
//!
//! The rounds share one module cache (ADR 0029), pruned after each round to what it used: in memory, or with
//! `--cache` the store on disk, written after each round.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use skif::{CachedFs, MemoryModuleCache, OsFs};

use crate::args::{Format, LintArgs};
use crate::env::Env;
use crate::lint::{self, canonical};
use crate::module_cache::{CacheHost, Persistent};
use crate::output::{stderr_line, write_stdout};
use crate::roots;

/// The shortest time between two scans; a slower scan stretches it to twice its own duration.
const POLL: Duration = Duration::from_millis(200);
/// How long the tree must stay unchanged after a change before the next run.
const QUIET: Duration = Duration::from_millis(100);
/// Directories that are never entered, whatever the ignore files say.
const SKIPPED: [&str; 3] = ["node_modules", ".git", "target"];

/// Lints, then re-lints on every change, until the process is interrupted. Returns only when the root cannot be
/// resolved at all.
pub fn run(args: &LintArgs, env: Env) -> u8 {
    let dir = match watched_dir(args) {
        Ok(dir) => dir,
        Err(code) => return code,
    };
    let targets = match args.targets() {
        Ok(targets) => targets,
        Err(error) => return lint::run_failed(args, &error),
    };
    let clear = env.stdout_tty && targets.contains(&(Format::Human, None));
    let written = args.written_files();
    // One cache for every round, so a round parses only what changed: in memory, or the `--cache` store, written
    // after each round.
    let modules: Box<dyn CacheHost> = if args.cache {
        Box::new(Persistent::new(args))
    } else {
        Box::new(Arc::new(MemoryModuleCache::new()))
    };
    let mut snapshot = scan(&dir, &written);
    lint_round(args, env, clear, &dir, modules.as_ref());
    let mut debouncer = Debouncer::new(QUIET);
    let mut last_scan = Duration::ZERO;
    loop {
        std::thread::sleep(POLL.max(last_scan * 2));
        let started = Instant::now();
        let next = scan(&dir, &written);
        last_scan = started.elapsed();
        let changed = next != snapshot;
        snapshot = next;
        if debouncer.observe(changed, Instant::now()) {
            lint_round(args, env, clear, &dir, modules.as_ref());
        }
    }
}

fn lint_round(args: &LintArgs, env: Env, clear: bool, dir: &Path, modules: &dyn CacheHost) {
    if clear {
        let _ = write_stdout(b"\x1b[2J\x1b[H");
    }
    // The watcher keeps running between rounds, so every round frees what it built.
    lint::run_with_cache(args, env, lint::Teardown::Free, Some(modules));
    stderr_line(&format!(
        "skif: watching {} for changes; press Ctrl-C to stop",
        roots::display(dir)
    ));
}

/// The directory the report's paths are relative to: the workspace root, or the invocation directory of a
/// multi-root run. When the root cannot be loaded yet (an invalid `skif.toml`), the path itself is watched, so the
/// user can fix the file and see the next run.
///
/// The directory is what [`roots::resolve`] gives as the base, found without discovering the roots of a multi-root
/// run: its base is the invocation directory whatever roots it finds, and the first round discovers them.
fn watched_dir(args: &LintArgs) -> Result<PathBuf, u8> {
    let start = match canonical(&args.path) {
        Ok(start) => start,
        Err(error) => {
            // The first run reports the failure in the requested format.
            return Err(lint::run_failed(args, &error));
        }
    };
    let fallback = if start.is_dir() {
        start.clone()
    } else {
        start.parent().map_or_else(|| start.clone(), Path::to_path_buf)
    };
    if args.recursive {
        return Ok(fallback);
    }
    Ok(skif::find_target(&start, &CachedFs::new(OsFs), &lint::overrides(args)).map_or(fallback, |target| target.root))
}

/// What a scan saw of one file.
type Stamp = (Option<SystemTime>, u64);

/// Every file below `dir` with its modification time and size. `.gitignore` files are honoured, the [`SKIPPED`]
/// directories are never entered, and the `written` files (canonical paths, as [`LintArgs::written_files`] gives
/// them) are left out; so is a file that vanishes during the scan.
fn scan(dir: &Path, written: &BTreeSet<PathBuf>) -> BTreeMap<PathBuf, Stamp> {
    let walker = ignore::WalkBuilder::new(dir)
        .hidden(false)
        .require_git(false)
        .git_global(false)
        .filter_entry(|entry| {
            !(entry.file_type().is_some_and(|kind| kind.is_dir())
                && entry.file_name().to_str().is_some_and(|name| SKIPPED.contains(&name)))
        })
        .build();
    walker
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_some_and(|kind| kind.is_file()) && !written.contains(entry.path()))
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            Some((entry.into_path(), (metadata.modified().ok(), metadata.len())))
        })
        .collect()
}

/// Decides when a series of scans should start a run: after a change, once a scan finds the tree unchanged and
/// the quiet period has passed since the last change.
#[derive(Debug)]
struct Debouncer {
    quiet: Duration,
    /// When the last change not yet followed by a run was seen.
    pending: Option<Instant>,
}

impl Debouncer {
    const fn new(quiet: Duration) -> Self {
        Self { quiet, pending: None }
    }

    /// Records one scan at `now`; true when a run should start.
    fn observe(&mut self, changed: bool, now: Instant) -> bool {
        if changed {
            self.pending = Some(now);
            return false;
        }
        match self.pending {
            Some(last) if now.saturating_duration_since(last) >= self.quiet => {
                self.pending = None;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn an_unchanged_tree_never_fires() {
        let start = Instant::now();
        let mut debouncer = Debouncer::new(100 * MS);
        assert!((0..20).all(|step| !debouncer.observe(false, start + step * 200 * MS)));
    }

    #[test]
    fn a_burst_fires_once_after_the_quiet_period() {
        let start = Instant::now();
        let mut debouncer = Debouncer::new(100 * MS);
        // Changes on three consecutive scans: nothing runs while the tree keeps changing.
        assert!(!debouncer.observe(true, start));
        assert!(!debouncer.observe(true, start + 20 * MS));
        assert!(!debouncer.observe(true, start + 40 * MS));
        // Quiet, but not for long enough yet.
        assert!(!debouncer.observe(false, start + 90 * MS));
        // Quiet for the whole period: one run.
        assert!(debouncer.observe(false, start + 140 * MS));
        assert!(!debouncer.observe(false, start + 400 * MS));
    }

    #[test]
    fn a_change_after_a_run_fires_again() {
        let start = Instant::now();
        let mut debouncer = Debouncer::new(100 * MS);
        assert!(!debouncer.observe(true, start));
        assert!(debouncer.observe(false, start + 200 * MS));
        assert!(!debouncer.observe(true, start + 400 * MS));
        assert!(debouncer.observe(false, start + 600 * MS));
    }

    /// The watched directory is the base the first round resolves: the workspace root of a member, a project's own
    /// directory, and the invocation directory of a multi-root run, recursive or not.
    #[test]
    fn the_watched_directory_is_the_base_the_run_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let top = canonical(dir.path()).unwrap();
        for (file, text) in [
            ("ws/package.json", r#"{"name": "ws", "workspaces": ["packages/*"]}"#),
            ("ws/packages/a/package.json", r#"{"name": "a"}"#),
            ("solo/package.json", r#"{"name": "solo"}"#),
        ] {
            let path = top.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        let cases = [
            (top.join("ws/packages/a"), false, top.join("ws")),
            (top.join("solo"), false, top.join("solo")),
            (top.clone(), false, top.clone()),
            (top.join("ws"), true, top.join("ws")),
            (top.clone(), true, top.clone()),
        ];
        for (path, recursive, expected) in cases {
            let args = LintArgs {
                path: path.clone(),
                recursive,
                ..LintArgs::default()
            };
            let watched = watched_dir(&args).unwrap();
            let resolved = roots::resolve(&path, recursive, &CachedFs::new(OsFs), &lint::overrides(&args))
                .map(|resolved| resolved.base)
                .unwrap();
            assert_eq!(watched, expected, "{path:?} recursive={recursive}");
            assert_eq!(watched, resolved, "{path:?} recursive={recursive}");
        }
    }

    #[test]
    fn the_scan_skips_dependencies_vcs_and_ignored_files() {
        let dir = tempfile::tempdir().unwrap();
        for file in [
            "package.json",
            "src/index.ts",
            "node_modules/x/index.js",
            ".git/HEAD",
            "target/out",
            "dist/bundle.js",
        ] {
            let path = dir.path().join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "x").unwrap();
        }
        fs::write(dir.path().join(".gitignore"), "dist/\n").unwrap();
        let files: Vec<PathBuf> = scan(dir.path(), &BTreeSet::new())
            .into_keys()
            .map(|path| path.strip_prefix(dir.path()).unwrap().to_path_buf())
            .collect();
        assert_eq!(
            files,
            [
                PathBuf::from(".gitignore"),
                PathBuf::from("package.json"),
                Path::new("src").join("index.ts"),
            ]
        );
    }

    #[test]
    fn the_scan_leaves_out_the_files_the_run_writes() {
        let dir = tempfile::tempdir().unwrap();
        let root = canonical(dir.path()).unwrap();
        fs::write(root.join("package.json"), "{}").unwrap();
        fs::write(root.join("report.sarif"), "{}").unwrap();
        let written = BTreeSet::from([root.join("report.sarif")]);
        let files: Vec<PathBuf> = scan(&root, &written).into_keys().collect();
        assert_eq!(files, [root.join("package.json")]);
    }

    #[test]
    fn the_scan_sees_a_changed_size() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("package.json"), "{}").unwrap();
        let before = scan(dir.path(), &BTreeSet::new());
        fs::write(dir.path().join("package.json"), "{ }").unwrap();
        assert_ne!(scan(dir.path(), &BTreeSet::new()), before);
    }
}
