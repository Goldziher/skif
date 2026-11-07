//! The pinned corpus: real monorepos fetched at fixed commits.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

/// Tool versions the comparison commands are pinned to.
#[derive(Debug, Clone, Deserialize)]
pub struct Tools {
    pub knip: String,
    pub syncpack: String,
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
    /// `false` when dependencies cannot be installed for this repository.
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
}

const fn default_install() -> bool {
    true
}

impl Repo {
    fn in_suite(&self, suite: &str) -> bool {
        self.suites.iter().any(|s| s == suite)
    }
}

/// Reads the corpus manifest.
///
/// # Errors
/// Fails when the file cannot be read or parsed, or a pin is not a full 40-character commit SHA.
pub fn load_manifest(path: &Path) -> Result<Manifest> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let manifest: Manifest = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    for repo in &manifest.repo {
        if repo.rev.len() != 40 || !repo.rev.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("{}: rev must be a full commit SHA, got {:?}", repo.name, repo.rev);
        }
        if let Some(unknown) = repo.suites.iter().find(|s| !["bench", "parity"].contains(&s.as_str())) {
            bail!("{}: unknown suite {unknown:?}", repo.name);
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
    Ok(dir)
}

/// Fetches the repositories of one suite, optionally narrowed to the named ones.
///
/// # Errors
/// Fails on the first repository that cannot be fetched.
pub fn fetch_suite(
    manifest: &Path,
    suite: &str,
    only: &[String],
    cache: &Path,
) -> Result<(Tools, Vec<(Repo, PathBuf)>)> {
    fs::create_dir_all(cache).with_context(|| format!("creating {}", cache.display()))?;
    let (tools, repos) = select(manifest, suite, only)?;
    let fetched = repos
        .into_iter()
        .map(|repo| fetch_repo(&repo, cache).map(|dir| (repo, dir)))
        .collect::<Result<Vec<_>>>()?;
    Ok((tools, fetched))
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
        assert!(!tools.knip.is_empty() && !tools.syncpack.is_empty());
        assert_eq!(bench.len(), 5);
        assert_eq!(parity.len(), manifest().repo.len());
        assert!(select(file, "bench", &["vscode".to_owned()]).is_err());
        assert_eq!(select(file, "parity", &["jest".to_owned()]).unwrap().1.len(), 1);
    }

    #[test]
    fn rejects_branch_pins() {
        let dir = std::env::temp_dir().join(format!("skif-bench-corpus-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("corpus.toml");
        fs::write(
            &file,
            "[tools]\nknip = \"1\"\nsyncpack = \"1\"\n[[repo]]\nname = \"x\"\nurl = \"u\"\nrev = \"main\"\npackage-manager = \"npm\"\n",
        )
        .unwrap();
        assert!(load_manifest(&file).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
