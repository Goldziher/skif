//! Package-manager detection: the single source of truth for every rule and fix.

use std::path::Path;

use serde_json::Value;
use skif_config::{Config, PackageManager};

use skif_core::FileSystem;

fn from_name(name: &str) -> Option<PackageManager> {
    match name {
        "npm" => Some(PackageManager::Npm),
        "pnpm" => Some(PackageManager::Pnpm),
        "yarn" => Some(PackageManager::Yarn),
        "bun" => Some(PackageManager::Bun),
        _ => None,
    }
}

fn from_dev_engine(entry: &Value) -> Option<PackageManager> {
    from_name(entry.get("name")?.as_str()?)
}

/// The package manager a root `package.json` declares: the `packageManager` field (`pnpm@9.12.0`), then
/// `devEngines.packageManager` (an object or a list; the first known entry wins).
#[must_use]
pub fn package_manager_from_manifest(manifest: &Value) -> Option<PackageManager> {
    if let Some(field) = manifest.get("packageManager").and_then(Value::as_str) {
        let name = field.split('@').next().unwrap_or(field);
        if let Some(package_manager) = from_name(name) {
            return Some(package_manager);
        }
    }
    match manifest.get("devEngines")?.get("packageManager")? {
        Value::Array(entries) => entries.iter().find_map(from_dev_engine),
        entry => from_dev_engine(entry),
    }
}

fn from_lockfile(fs: &dyn FileSystem, root: &Path) -> Option<PackageManager> {
    let exists = |name: &str| fs.is_file(&root.join(name));
    if exists("pnpm-lock.yaml") {
        Some(PackageManager::Pnpm)
    } else if exists("bun.lock") || exists("bun.lockb") {
        Some(PackageManager::Bun)
    } else if exists("yarn.lock") {
        Some(PackageManager::Yarn)
    } else if exists("package-lock.json") {
        Some(PackageManager::Npm)
    } else {
        None
    }
}

/// Yarn Berry leaves `.yarnrc.yml` and `.yarn/releases` in a checkout whether or not `yarn.lock` is committed.
fn from_yarn_berry_files(fs: &dyn FileSystem, root: &Path) -> Option<PackageManager> {
    (fs.is_file(&root.join(".yarnrc.yml")) || fs.is_dir(&root.join(".yarn").join("releases")))
        .then_some(PackageManager::Yarn)
}

/// A `bunfig.toml` with an `[install]` table configures Bun's installer, so a checkout that commits no lockfile
/// installs with Bun. One without it configures only Bun's runtime or test runner (`[test]`, `preload`), which a
/// repository installed by npm or Yarn may use too, so it does not decide.
fn from_bunfig(fs: &dyn FileSystem, root: &Path) -> Option<PackageManager> {
    fs.read_to_string(&root.join("bunfig.toml"))
        .ok()?
        .parse::<toml::Table>()
        .ok()?
        .get("install")
        .is_some_and(toml::Value::is_table)
        .then_some(PackageManager::Bun)
}

/// Detects the package manager, first match wins: `[workspace] package-manager` in the config, the manifest
/// declaration, the one `rush.json` names, a lockfile (`pnpm-lock.yaml`, `bun.lock`/`bun.lockb`, `yarn.lock`,
/// `package-lock.json`), then the presence of `pnpm-workspace.yaml`, then the files Yarn Berry leaves
/// (`.yarnrc.yml`, `.yarn/releases`), then a `bunfig.toml` with an `[install]` table. `None` when nothing indicates
/// a manager.
pub(crate) fn detect(
    fs: &dyn FileSystem,
    root: &Path,
    config: &Config,
    manifest: &Value,
    from_rush: Option<PackageManager>,
    has_pnpm_workspace: bool,
) -> Option<PackageManager> {
    config
        .workspace
        .package_manager
        .or_else(|| package_manager_from_manifest(manifest))
        .or(from_rush)
        .or_else(|| from_lockfile(fs, root))
        .or_else(|| has_pnpm_workspace.then_some(PackageManager::Pnpm))
        .or_else(|| from_yarn_berry_files(fs, root))
        .or_else(|| from_bunfig(fs, root))
}

/// Whether the workspace uses Yarn Classic (1.x), which has no `workspace:` protocol.
///
/// The `packageManager` field decides when it names Yarn; without it the first lines of `yarn.lock` do
/// (`# yarn lockfile v1`).
pub(crate) fn is_yarn_classic(fs: &dyn FileSystem, root: &Path, manifest: &Value) -> bool {
    match manifest
        .get("packageManager")
        .and_then(Value::as_str)
        .filter(|field| field.starts_with("yarn@"))
    {
        Some(field) => field.starts_with("yarn@1."),
        None => declares_yarn_lockfile_v1(fs, &root.join("yarn.lock")),
    }
}

/// The lines at the top of `yarn.lock` that may carry the Yarn 1 header.
const YARN_LOCK_HEADER_LINES: usize = 8;

/// How much of `yarn.lock` is read first to find its [`YARN_LOCK_HEADER_LINES`]; the lockfile itself can be many
/// megabytes.
const YARN_LOCK_HEAD_BYTES: usize = 4 * 1024;

/// Whether one of the first lines of the lockfile at `path` is `# yarn lockfile v1`. Only the head of the file is
/// read, unless those lines are longer than [`YARN_LOCK_HEAD_BYTES`].
fn declares_yarn_lockfile_v1(fs: &dyn FileSystem, path: &Path) -> bool {
    let Ok(head) = fs.read_prefix(path, YARN_LOCK_HEAD_BYTES) else {
        return false;
    };
    let header_end = head
        .iter()
        .enumerate()
        .filter(|(_, byte)| **byte == b'\n')
        .nth(YARN_LOCK_HEADER_LINES - 1)
        .map(|(position, _)| position);
    let header = match header_end {
        Some(end) => &head[..end],
        None if head.len() < YARN_LOCK_HEAD_BYTES => &head[..],
        None => return fs.read_to_string(path).is_ok_and(|lock| has_yarn_v1_header(&lock)),
    };
    std::str::from_utf8(header).is_ok_and(has_yarn_v1_header)
}

fn has_yarn_v1_header(text: &str) -> bool {
    text.lines()
        .take(YARN_LOCK_HEADER_LINES)
        .any(|line| line.trim() == "# yarn lockfile v1")
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::path::Path;

    use skif_core::MemoryFs;

    use super::{YARN_LOCK_HEAD_BYTES, declares_yarn_lockfile_v1};

    #[test]
    fn the_yarn_1_header_counts_only_on_the_first_eight_lines() {
        let entry = "\"left-pad@^1.0.0\":\n  version \"1.3.0\"\n";
        let long_line = format!("# {}\n", "x".repeat(YARN_LOCK_HEAD_BYTES));
        let cases = [
            (
                "# THIS IS AN AUTOGENERATED FILE.\n# yarn lockfile v1\n\n\n".to_owned(),
                true,
            ),
            ("# yarn lockfile v1".to_owned(), true),
            ("# yarn lockfile v1\r\n".to_owned(), true),
            (format!("{}# yarn lockfile v1\n", "\n".repeat(7)), true),
            (format!("{}# yarn lockfile v1\n", "\n".repeat(8)), false),
            (format!("# yarn lockfile v1\n{}", entry.repeat(1000)), true),
            (format!("{long_line}# yarn lockfile v1\n"), true),
            (format!("{}# yarn lockfile v1\n", long_line.repeat(8)), false),
            ("__metadata:\n  version: 6\n".to_owned(), false),
            (String::new(), false),
        ];
        for (text, expected) in cases {
            let fs = MemoryFs::new().with_file("/ws/yarn.lock", text.as_str());
            let head: String = text.chars().take(40).collect();
            assert_eq!(
                declares_yarn_lockfile_v1(&fs, Path::new("/ws/yarn.lock")),
                expected,
                "{head:?}"
            );
        }
        assert!(!declares_yarn_lockfile_v1(&MemoryFs::new(), Path::new("/ws/yarn.lock")));
    }
}
