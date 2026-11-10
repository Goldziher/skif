//! `bun.lock`, Bun's text lockfile: the resolved packages and the patches the install applied.
//!
//! Like [`PnpmLockfile`](crate::PnpmLockfile) it is read only by the rules that need it. The binary `bun.lockb` is
//! not read.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;
use skif_core::FileSystem;

use crate::error::WorkspaceError;
use crate::lockfile::split_key;

/// The file name of Bun's text lockfile.
pub const BUN_LOCKFILE: &str = "bun.lock";

/// A parsed `bun.lock`. Only what the lockfile checks need is modelled.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BunLockfile {
    /// Workspace-relative path.
    pub path: PathBuf,
    /// Every resolved release, as `name@version`, from the first element of each `packages` entry. Workspace
    /// packages (`name@workspace:dir`) are left out.
    pub packages: BTreeSet<String>,
    /// `patchedDependencies` as Bun recorded them when it applied the patches: key to patch file.
    pub patched_dependencies: BTreeMap<String, String>,
}

impl BunLockfile {
    /// Reads `bun.lock` from the workspace `root`; `None` when there is none.
    ///
    /// # Errors
    /// [`WorkspaceError::InvalidLockfile`] when the file is not JSON with trailing commas or is not an object, and
    /// [`WorkspaceError::Io`] when it exists but cannot be read.
    pub fn read(fs: &dyn FileSystem, root: &Path) -> Result<Option<Self>, WorkspaceError> {
        let path = root.join(BUN_LOCKFILE);
        if !fs.is_file(&path) {
            return Ok(None);
        }
        let source = fs
            .read_to_string(&path)
            .map_err(|source| WorkspaceError::Io { path, source })?;
        Self::parse(PathBuf::from(BUN_LOCKFILE), &source).map(Some)
    }

    /// Parses the text of a `bun.lock`; `path` is the workspace-relative path used in errors and diagnostics.
    ///
    /// # Errors
    /// [`WorkspaceError::InvalidLockfile`] as for [`read`](Self::read).
    pub fn parse(path: PathBuf, source: &str) -> Result<Self, WorkspaceError> {
        let text = source.strip_prefix('\u{feff}').unwrap_or(source);
        let document = parse_bun_lock(text).map_err(|error| WorkspaceError::InvalidLockfile {
            path: path.clone(),
            message: error.to_string(),
        })?;
        let Value::Object(document) = document else {
            return Err(WorkspaceError::InvalidLockfile {
                path,
                message: "the lockfile is not an object".to_owned(),
            });
        };
        let packages = document
            .get("packages")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|packages| packages.values())
            .filter_map(|entry| entry.as_array()?.first()?.as_str())
            .filter(|key| split_key(key).is_some_and(|(_, version, _)| !version.starts_with("workspace:")))
            .map(str::to_owned)
            .collect();
        let patched_dependencies = document
            .get("patchedDependencies")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter_map(|(key, file)| Some((key.clone(), file.as_str()?.to_owned())))
            .collect();
        Ok(Self {
            path,
            packages,
            patched_dependencies,
        })
    }

    /// The versions of `name` the lockfile resolves, in byte order.
    #[must_use]
    pub fn versions_of(&self, name: &str) -> BTreeSet<&str> {
        self.packages
            .iter()
            .filter_map(|key| split_key(key))
            .filter(|(resolved, _, _)| *resolved == name)
            .map(|(_, version, _)| version)
            .collect()
    }
}

/// Parses the text of a `bun.lock` as JSON: Bun writes a trailing comma before `}` and `]`, which is dropped.
///
/// # Errors
/// The JSON error when the text is not such a document.
pub fn parse_bun_lock(text: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(&strip_trailing_commas(text))
}

/// `text` without the commas, outside of strings, that only whitespace separates from a closing `}` or `]`.
fn strip_trailing_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    for (index, &c) in chars.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if c == ','
            && chars[index + 1..]
                .iter()
                .find(|next| !next.is_whitespace())
                .is_some_and(|next| matches!(next, '}' | ']'))
        {
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCK: &str = r#"{
  "lockfileVersion": 2,
  "configVersion": 1,
  "workspaces": {
    "": { "name": "root", },
  },
  "patchedDependencies": {
    "ms@2.1.2": "patches/ms@2.1.2.patch",
  },
  "packages": {
    "@s/a": ["@s/a@workspace:packages/a"],

    "@scope/tool": ["@scope/tool@1.0.0", "", { "bin": { "tool": "cli.js" } }, "sha512-x"],

    "ms": ["ms@2.1.2", "", {}, "sha512-y"],

    "other/ms": ["ms@2.1.3", "", {}, "sha512-z"],
  }
}
"#;

    #[test]
    fn packages_and_patches_are_read_and_workspace_packages_left_out() {
        let lock = BunLockfile::parse(PathBuf::from(BUN_LOCKFILE), LOCK).unwrap();
        assert_eq!(
            lock.packages.iter().map(String::as_str).collect::<Vec<_>>(),
            ["@scope/tool@1.0.0", "ms@2.1.2", "ms@2.1.3"]
        );
        assert_eq!(lock.versions_of("ms").into_iter().collect::<Vec<_>>(), ["2.1.2", "2.1.3"]);
        assert_eq!(lock.versions_of("@scope/tool").into_iter().collect::<Vec<_>>(), ["1.0.0"]);
        assert_eq!(
            lock.patched_dependencies,
            BTreeMap::from([("ms@2.1.2".to_owned(), "patches/ms@2.1.2.patch".to_owned())])
        );
    }

    #[test]
    fn trailing_commas_inside_strings_are_kept() {
        assert_eq!(
            parse_bun_lock(r#"{"a": ["x,]", "y,}",], }"#).unwrap(),
            serde_json::json!({"a": ["x,]", "y,}"]})
        );
    }

    #[test]
    fn a_lockfile_that_is_not_an_object_or_not_jsonc_is_invalid() {
        for text in ["[]", "{ \"packages\": "] {
            let error = BunLockfile::parse(PathBuf::from(BUN_LOCKFILE), text).unwrap_err();
            assert!(matches!(error, WorkspaceError::InvalidLockfile { .. }), "{text}");
        }
    }

    #[test]
    fn read_returns_none_without_a_lockfile() {
        let fs = skif_core::MemoryFs::new();
        assert_eq!(BunLockfile::read(&fs, Path::new("/r")).unwrap(), None);
        let fs = skif_core::MemoryFs::new().with_file("/r/bun.lock", LOCK);
        assert_eq!(BunLockfile::read(&fs, Path::new("/r")).unwrap().unwrap().patched_dependencies.len(), 1);
    }
}
