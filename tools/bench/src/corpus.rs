//! The pinned corpus: real monorepos fetched at fixed commits.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Manifest {
    repo: Vec<Repo>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Repo {
    pub name: String,
    pub url: String,
    pub rev: String,
    pub package_manager: String,
}

/// Reads the corpus manifest.
///
/// # Errors
/// Fails when the file cannot be read or parsed, or a pin is not a full 40-character commit SHA.
pub fn load(path: &Path) -> Result<Vec<Repo>> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let manifest: Manifest = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
    for repo in &manifest.repo {
        if repo.rev.len() != 40 || !repo.rev.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("{}: rev must be a full commit SHA, got {:?}", repo.name, repo.rev);
        }
    }
    Ok(manifest.repo)
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
    git(&dir, &["fetch", "--quiet", "--depth", "1", "origin", &repo.rev])?;
    git(&dir, &["checkout", "--quiet", "--force", "--detach", &repo.rev])?;
    Ok(dir)
}

/// Fetches every repository of the corpus manifest.
///
/// # Errors
/// Fails on the first repository that cannot be fetched.
pub fn fetch_all(manifest: &Path, cache: &Path) -> Result<Vec<(Repo, PathBuf)>> {
    fs::create_dir_all(cache).with_context(|| format!("creating {}", cache.display()))?;
    load(manifest)?
        .into_iter()
        .map(|repo| fetch_repo(&repo, cache).map(|dir| (repo, dir)))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn shipped_manifest_is_valid_and_pinned() {
        let repos = load(Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/corpus.toml"))).unwrap();
        assert!(repos.len() >= 5);
        let packages: std::collections::HashSet<_> = repos.iter().map(|r| r.package_manager.as_str()).collect();
        assert!(["pnpm", "yarn", "npm"].iter().all(|pm| packages.contains(pm)));
    }

    #[test]
    fn rejects_branch_pins() {
        let dir = std::env::temp_dir().join(format!("skif-bench-corpus-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("corpus.toml");
        fs::write(
            &file,
            "[[repo]]\nname = \"x\"\nurl = \"u\"\nrev = \"main\"\npackage-manager = \"npm\"\n",
        )
        .unwrap();
        assert!(load(&file).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
