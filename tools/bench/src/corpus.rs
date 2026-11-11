//! The pinned corpus: real monorepos fetched at fixed commits.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Tool versions the comparison commands are pinned to: `parity`, `syncpack-diff` and `deadcode` run them through
/// `npx`, and `run` installs them into a scratch prefix and times their executables.
#[derive(Debug, Clone, Deserialize)]
pub struct Tools {
    pub knip: String,
    pub syncpack: String,
    pub sherif: String,
}

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub tools: Tools,
    pub repo: Vec<Repo>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Repo {
    pub name: String,
    pub url: String,
    pub rev: String,
    pub package_manager: String,
    /// `bench` repositories are timed by `skif-bench run`; `parity` repositories are compared with knip and syncpack.
    #[serde(default)]
    pub suites: Vec<String>,
    /// When non-empty, only these directories (plus the files at the root) are checked out.
    #[serde(default)]
    pub sparse: Vec<String>,
    /// `false` when dependencies cannot be installed for this repository; `parity --install` then compares it
    /// uninstalled.
    #[serde(default = "default_install")]
    pub install: bool,
    #[serde(default)]
    pub notes: String,
    /// `true` when skif runs with `--recursive` on this repository, because it holds nested roots that are not
    /// workspace members. knip analyses their files from the root, and skif's paths stay relative to the start
    /// directory, so the two stay comparable.
    #[serde(default)]
    pub recursive: bool,
    /// Path prefixes knip does not read here (a deny-all `.gitignore` hides them), so it reports everything their
    /// manifests list as unused and nothing about their files. Findings below them on one side only are a known
    /// deviation, not a disagreement.
    #[serde(default)]
    pub knip_unread: Vec<String>,
    /// Seconds the install may take, instead of the global timeout.
    #[serde(default)]
    pub install_timeout_secs: Option<u64>,
    /// The install command (program first) to use instead of the package manager's default.
    #[serde(default)]
    pub install_command: Vec<String>,
    /// Entry files of the root workspace (globs relative to the checkout) that neither tool finds by itself, such as
    /// the entry points a bundler script names. Both tools get them in addition to their default entries, so files
    /// only these reach are not reported as unused on both sides.
    #[serde(default)]
    pub entry: Vec<String>,
    /// Extra arguments for knip when `skif-bench run` times it here, so that it reads the same files as skif (such as
    /// `--no-gitignore` where a deny-all `.gitignore` hides the workspaces from knip). `parity` does not use them.
    #[serde(default)]
    pub bench_knip_args: Vec<String>,
    /// Why knip's report on this repository is not a valid comparison (such as configurations that import workspace
    /// packages which are never built). The repository is still compared, and the report marks the row with this.
    #[serde(default)]
    pub knip_invalid: Option<String>,
    /// How to give knip a root it can run in where the checkout has none: `rush` runs it in a scratch copy with a
    /// root `package.json` made from `rush.json`.
    #[serde(default)]
    pub knip_root: Option<String>,
    /// A command (program first) run in the checkout after a successful install, before the install counts as done:
    /// it builds the workspace packages whose built output the tools' configurations load, which an install without
    /// lifecycle scripts leaves unbuilt.
    #[serde(default)]
    pub build_command: Vec<String>,
    /// Seconds the `build-command` may take; the install's time limit when unset.
    #[serde(default)]
    pub build_timeout_secs: Option<u64>,
}

/// The `knip-root` of a Rush repository: knip runs in the checkout with a root `package.json` made from `rush.json`,
/// which is there only while knip runs.
pub const RUSH_KNIP_ROOT: &str = "rush";

/// The `//` field of the root `package.json` the knip comparison writes into a Rush checkout for the knip run: a root
/// manifest holding it is the harness's own, left behind by an interrupted run.
pub const SCRATCH_ROOT_MARK: &str = "written by skif-bench for the knip run and removed after it";

/// Whether the `package.json` at `path` is a scratch root manifest of the knip comparison ([`SCRATCH_ROOT_MARK`]).
#[must_use]
pub fn is_scratch_root(path: &Path) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .is_some_and(|manifest| manifest.get("//").and_then(serde_json::Value::as_str) == Some(SCRATCH_ROOT_MARK))
}

/// Removes the scratch root manifest an interrupted knip comparison left in a checkout, so no other run reads it.
///
/// # Errors
/// Fails when the file cannot be removed.
pub fn remove_scratch_root(dir: &Path) -> Result<()> {
    let path = dir.join("package.json");
    if is_scratch_root(&path) {
        eprintln!("corpus: removing the scratch root manifest {}", path.display());
        fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

const fn default_install() -> bool {
    true
}

impl Repo {
    fn in_suite(&self, suite: &str) -> bool {
        self.suites.iter().any(|s| s == suite)
    }
}

/// Whether `version` names one release, `major.minor.patch` with an optional pre-release or build suffix, and not a
/// range or a dist-tag that resolves to another release over time.
fn is_exact_version(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        && version[core.len()..]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-+.".contains(&b))
}

/// Reads the corpus manifest.
///
/// # Errors
/// Fails when the file cannot be read or parsed, a repository pin is not a full 40-character commit SHA, or a
/// comparison tool version is not exact.
pub fn load_manifest(path: &Path) -> Result<Manifest> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let manifest: Manifest = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    let tools = [
        ("knip", &manifest.tools.knip),
        ("syncpack", &manifest.tools.syncpack),
        ("sherif", &manifest.tools.sherif),
    ];
    if let Some((tool, version)) = tools.iter().find(|(_, version)| !is_exact_version(version)) {
        bail!("[tools] {tool}: the version must be exact (major.minor.patch), got {version:?}");
    }
    for repo in &manifest.repo {
        if repo.rev.len() != 40 || !repo.rev.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("{}: rev must be a full commit SHA, got {:?}", repo.name, repo.rev);
        }
        if let Some(unknown) = repo.suites.iter().find(|s| !["bench", "parity"].contains(&s.as_str())) {
            bail!("{}: unknown suite {unknown:?}", repo.name);
        }
        if let Some(root) = repo.knip_root.as_deref().filter(|root| *root != RUSH_KNIP_ROOT) {
            bail!(
                "{}: unknown knip-root {root:?} (the only one is {RUSH_KNIP_ROOT:?})",
                repo.name
            );
        }
    }
    Ok(manifest)
}

/// Reads the repositories of one suite (`bench` or `parity`), optionally narrowed to the named ones.
///
/// # Errors
/// Fails when the manifest is invalid or a requested name is not in the suite.
pub fn select(path: &Path, suite: &str, only: &[String]) -> Result<(Tools, Vec<Repo>)> {
    let manifest = load_manifest(path)?;
    let repos: Vec<Repo> = manifest.repo.into_iter().filter(|r| r.in_suite(suite)).collect();
    if let Some(missing) = only.iter().find(|name| !repos.iter().any(|r| &r.name == *name)) {
        bail!("{missing}: not a repository of the {suite} suite in {}", path.display());
    }
    let repos = if only.is_empty() {
        repos
    } else {
        repos.into_iter().filter(|r| only.contains(&r.name)).collect()
    };
    Ok((manifest.tools, repos))
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn is_checked_out(dir: &Path, rev: &str) -> bool {
    dir.join(".git").exists() && git(dir, &["rev-parse", "HEAD"]).is_ok_and(|head| head == rev)
}

/// Fetches one repository at its pinned commit into `cache/<name>`; a no-op when already there.
///
/// # Errors
/// Fails when a git command fails.
pub fn fetch_repo(repo: &Repo, cache: &Path) -> Result<PathBuf> {
    let dir = cache.join(&repo.name);
    if is_checked_out(&dir, &repo.rev) {
        eprintln!("corpus: {} already at {}", repo.name, &repo.rev[..12]);
        remove_scratch_root(&dir)?;
        return Ok(dir);
    }
    eprintln!("corpus: fetching {} at {}", repo.name, &repo.rev[..12]);
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    if !dir.join(".git").exists() {
        git(&dir, &["init", "--quiet"])?;
    }
    if git(&dir, &["remote", "get-url", "origin"]).is_err() {
        git(&dir, &["remote", "add", "origin", &repo.url])?;
    }
    if !repo.sparse.is_empty() {
        let mut args = vec!["sparse-checkout", "set", "--cone"];
        args.extend(repo.sparse.iter().map(String::as_str));
        git(&dir, &args)?;
    }
    git(&dir, &["fetch", "--quiet", "--depth", "1", "origin", &repo.rev])?;
    git(&dir, &["checkout", "--quiet", "--force", "--detach", &repo.rev])?;
    remove_scratch_root(&dir)?;
    Ok(dir)
}

/// An exclusive lock on one checkout of the corpus, released when it is dropped or the process ends. A command holds
/// it from the fetch to the end of its pass over the repository, so two runs never install into, write a scratch
/// root manifest into or time the same checkout at once. The lock is the operating system's file lock on
/// [`lock_path`], so a run that dies leaves no stale lock; the file holds the process id of the holder for the error
/// message and is left in place. On Windows the lock also keeps other processes from reading the file, so the message
/// there names no process id.
#[derive(Debug)]
pub struct CheckoutLock {
    _file: File,
}

/// The lock file of the checkout `dir`: `<name>.lock` beside it, outside the checkout, so no tool reads it.
#[must_use]
pub fn lock_path(dir: &Path) -> PathBuf {
    let mut name = dir.file_name().map(std::ffi::OsStr::to_os_string).unwrap_or_default();
    name.push(".lock");
    dir.with_file_name(name)
}

/// Takes the lock of the checkout `dir`, or fails at once when another process holds it.
///
/// # Errors
/// Fails when another process holds the lock, naming its process id when the lock file can be read, or when the lock
/// file cannot be opened, locked or written; a corpus directory that is not writable is named as such, since a read-only
/// cache is not supported.
pub fn lock_checkout(dir: &Path) -> Result<CheckoutLock> {
    let path = lock_path(dir);
    let opened = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path);
    let mut file = match opened {
        Ok(file) => file,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem
            ) =>
        {
            bail!(
                "cannot create the lock file {} ({error}): the corpus directory is not writable, and skif-bench \
                 needs a writable one to lock, fetch and install its checkouts",
                path.display()
            );
        }
        Err(error) => return Err(error).with_context(|| format!("opening the lock file {}", path.display())),
    };
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            let holder = fs::read_to_string(&path)
                .ok()
                .map(|text| text.trim().to_owned())
                .filter(|pid| !pid.is_empty())
                .map_or_else(|| "another process".to_owned(), |pid| format!("process {pid}"));
            bail!(
                "{} is in use by {holder} (lock file {}): wait for that skif-bench run to finish",
                dir.display(),
                path.display()
            );
        }
        Err(TryLockError::Error(error)) => {
            return Err(error).with_context(|| format!("locking {}", path.display()));
        }
    }
    file.set_len(0)
        .and_then(|()| write!(file, "{}", std::process::id()))
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(CheckoutLock { _file: file })
}

/// The checkouts of one suite at their pinned commits, each locked ([`CheckoutLock`]) for as long as this value lives:
/// keep it until the pass over the repositories is done.
#[derive(Debug)]
pub struct Suite {
    pub tools: Tools,
    pub repos: Vec<(Repo, PathBuf)>,
    _locks: Vec<CheckoutLock>,
}

/// Locks and fetches the repositories of one suite, optionally narrowed to the named ones.
///
/// # Errors
/// Fails on the first repository whose checkout another run holds or that cannot be fetched.
pub fn fetch_suite(manifest: &Path, suite: &str, only: &[String], cache: &Path) -> Result<Suite> {
    fs::create_dir_all(cache).with_context(|| format!("creating {}", cache.display()))?;
    let (tools, selected) = select(manifest, suite, only)?;
    let mut repos = Vec::with_capacity(selected.len());
    let mut locks = Vec::with_capacity(selected.len());
    for repo in selected {
        locks.push(lock_checkout(&cache.join(&repo.name))?);
        let dir = fetch_repo(&repo, cache)?;
        repos.push((repo, dir));
    }
    Ok(Suite {
        tools,
        repos,
        _locks: locks,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        load_manifest(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/corpus.toml"))).unwrap()
    }

    #[test]
    fn shipped_manifest_is_valid_and_pinned() {
        let repos = manifest().repo;
        assert!(repos.len() >= 16);
        let packages: std::collections::HashSet<_> = repos.iter().map(|r| r.package_manager.as_str()).collect();
        assert!(
            ["pnpm", "yarn", "npm", "bun", "rush"]
                .iter()
                .all(|pm| packages.contains(pm))
        );
        assert!(repos.iter().all(|r| !r.suites.is_empty() && !r.notes.is_empty()));
    }

    #[test]
    fn suites_split_the_corpus() {
        let file = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/corpus.toml"));
        let (tools, bench) = select(file, "bench", &[]).unwrap();
        let (_, parity) = select(file, "parity", &[]).unwrap();
        assert!(!tools.knip.is_empty() && !tools.syncpack.is_empty() && !tools.sherif.is_empty());
        assert_eq!(bench.len(), 5);
        assert_eq!(parity.len(), manifest().repo.len());
        assert!(select(file, "bench", &["vscode".to_owned()]).is_err());
        assert_eq!(select(file, "parity", &["jest".to_owned()]).unwrap().1.len(), 1);
    }

    #[test]
    fn no_repository_is_marked_as_an_invalid_knip_comparison() {
        let repos = manifest().repo;
        let marked: Vec<&str> = repos
            .iter()
            .filter(|r| r.knip_invalid.is_some())
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(marked, Vec::<&str>::new());
    }

    #[test]
    fn a_repository_reads_the_reason_its_knip_run_is_invalid() {
        let repo: Repo = toml::from_str(
            "name = \"r\"\nurl = \"u\"\nrev = \"0\"\npackage-manager = \"pnpm\"\nknip-invalid = \"configs do not load\"\n",
        )
        .unwrap();
        assert_eq!(repo.knip_invalid.as_deref(), Some("configs do not load"));
    }

    #[test]
    fn rushstack_installs_with_rush_and_runs_knip_in_a_synthesized_root() {
        let repos = manifest().repo;
        let rushstack = repos.iter().find(|r| r.name == "rushstack").unwrap();
        assert!(rushstack.install);
        assert_eq!(
            rushstack.install_command,
            [
                "node",
                "common/scripts/install-run-rush.js",
                "install",
                "--bypass-policy"
            ]
        );
        assert_eq!(rushstack.knip_root.as_deref(), Some(RUSH_KNIP_ROOT));
        assert!(
            repos
                .iter()
                .filter(|r| r.name != "rushstack")
                .all(|r| r.knip_root.is_none())
        );
    }

    #[test]
    fn n8n_builds_the_config_packages_its_tool_configurations_load() {
        let repos = manifest().repo;
        let n8n = repos.iter().find(|r| r.name == "n8n").unwrap();
        assert_eq!(n8n.build_command.first().map(String::as_str), Some("pnpm"));
        for package in [
            "@n8n/oxlint-config",
            "@n8n/vitest-config",
            "@n8n/eslint-config",
            "@n8n/stylelint-config",
            "@n8n/constants",
        ] {
            let filter = format!("{package}...");
            assert!(n8n.build_command.contains(&filter), "{filter}");
        }
        assert_eq!(&n8n.build_command[n8n.build_command.len() - 2..], ["run", "build"]);
        assert!(
            repos
                .iter()
                .filter(|r| r.name != "n8n")
                .all(|r| r.build_command.is_empty())
        );
    }

    #[test]
    fn opencode_installs_with_a_pinned_bun() {
        let repos = manifest().repo;
        let opencode = repos.iter().find(|r| r.name == "opencode").unwrap();
        assert_eq!(&opencode.install_command[..5], ["mise", "x", "bun@1.3.14", "--", "bun"]);
        assert!(opencode.install_command.iter().any(|arg| arg == "--frozen-lockfile"));
    }

    #[test]
    fn only_the_harness_scratch_root_manifest_is_removed() {
        let dir = std::env::temp_dir().join(format!("skif-bench-scratch-root-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let manifest = dir.join("package.json");
        let own = serde_json::json!({ "//": SCRATCH_ROOT_MARK, "private": true, "workspaces": ["apps/a"] });
        fs::write(&manifest, own.to_string()).unwrap();
        assert!(is_scratch_root(&manifest));
        remove_scratch_root(&dir).unwrap();
        assert!(!manifest.exists());
        remove_scratch_root(&dir).unwrap();
        fs::write(&manifest, r#"{ "name": "real", "//": "a comment" }"#).unwrap();
        remove_scratch_root(&dir).unwrap();
        assert!(manifest.exists(), "a manifest of the repository stays");
        fs::write(&manifest, "{ not json").unwrap();
        remove_scratch_root(&dir).unwrap();
        assert!(manifest.exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_checkout_another_run_holds_fails_at_once_and_frees_when_released() {
        let cache = std::env::temp_dir().join(format!("skif-bench-lock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&cache);
        fs::create_dir_all(&cache).unwrap();
        let dir = cache.join("jest");
        assert_eq!(lock_path(&dir), cache.join("jest.lock"));
        let held = lock_checkout(&dir).unwrap();
        // Windows locks are mandatory: the holder's PID cannot be read while the lock is held.
        #[cfg(unix)]
        assert_eq!(
            fs::read_to_string(lock_path(&dir)).unwrap(),
            std::process::id().to_string()
        );
        let error = lock_checkout(&dir).unwrap_err().to_string();
        assert!(error.contains("is in use by"), "{error}");
        assert!(error.contains("jest.lock"), "{error}");
        #[cfg(unix)]
        assert!(error.contains(&format!("process {}", std::process::id())), "{error}");
        drop(held);
        // A child another test forks in this process holds a copy of the descriptor, and with it the lock, until it
        // execs, so the release is observed within a bounded wait rather than at once.
        let mut again = lock_checkout(&dir);
        for _ in 0..200 {
            if again.is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
            again = lock_checkout(&dir);
        }
        assert!(again.is_ok(), "a released lock is free within 2 s: {again:?}");
        drop(again);
        fs::remove_dir_all(&cache).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_corpus_directory_that_is_not_writable_is_named_as_such() {
        use std::os::unix::fs::PermissionsExt as _;
        let cache = std::env::temp_dir().join(format!("skif-bench-lock-read-only-{}", std::process::id()));
        let _ = fs::remove_dir_all(&cache);
        fs::create_dir_all(&cache).unwrap();
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o555)).unwrap();
        // A superuser writes into a read-only directory anyway, so there is nothing to refuse.
        let writable = File::create(cache.join("probe")).is_ok();
        let result = lock_checkout(&cache.join("jest"));
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir_all(&cache).unwrap();
        if writable {
            return;
        }
        let error = result.unwrap_err().to_string();
        assert!(error.contains("the corpus directory is not writable"), "{error}");
        assert!(error.contains("jest.lock"), "{error}");
    }

    /// Writes a corpus manifest with these tool versions and one repository pinned to `rev`, and loads it.
    fn load_with(name: &str, (knip, syncpack, sherif): (&str, &str, &str), rev: &str) -> Result<Manifest> {
        let dir = std::env::temp_dir().join(format!("skif-bench-corpus-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("corpus.toml");
        fs::write(
            &file,
            format!(
                "[tools]\nknip = {knip:?}\nsyncpack = {syncpack:?}\nsherif = {sherif:?}\n[[repo]]\nname = \"x\"\nurl = \"u\"\nrev = {rev:?}\npackage-manager = \"npm\"\n"
            ),
        )
        .unwrap();
        let loaded = load_manifest(&file);
        fs::remove_dir_all(&dir).unwrap();
        loaded
    }

    const PINNED: (&str, &str, &str) = ("6.44.1", "15.3.3", "1.13.0");

    #[test]
    fn rejects_branch_pins() {
        let error = load_with("branch", PINNED, "main").unwrap_err().to_string();
        assert!(error.contains("x: rev must be a full commit SHA"), "{error}");
        assert!(load_with("sha", PINNED, &"a".repeat(40)).is_ok());
    }

    #[test]
    fn rejects_a_comparison_tool_version_that_is_not_exact() {
        let sha = "a".repeat(40);
        for (tools, tool, version) in [
            (("^6.44.1", "15.3.3", "1.13.0"), "knip", "^6.44.1"),
            (("6.44.1", "latest", "1.13.0"), "syncpack", "latest"),
            (("6.44.1", "15.3.3", "1.13"), "sherif", "1.13"),
            (("6.44.1", "15.x.3", "1.13.0"), "syncpack", "15.x.3"),
            (("6.44.1", "15.3.3", ">=1.13.0"), "sherif", ">=1.13.0"),
            (("6.44.1 || 7.0.0", "15.3.3", "1.13.0"), "knip", "6.44.1 || 7.0.0"),
        ] {
            let error = load_with("range", tools, &sha).unwrap_err().to_string();
            assert_eq!(
                error,
                format!("[tools] {tool}: the version must be exact (major.minor.patch), got {version:?}")
            );
        }
        assert!(load_with("prerelease", ("6.0.0-beta.2", "15.3.3", "1.13.0+build.1"), &sha).is_ok());
    }
}
