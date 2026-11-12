//! `bun.lock`, Bun's text lockfile: the resolved packages, the graph between them and the workspaces, and the patches
//! the install applied.
//!
//! Like [`PnpmLockfile`](crate::PnpmLockfile) it is read only by the rules that need it. The binary `bun.lockb` is
//! not read.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use jsonc_parser::{ParseOptions, parse_to_serde_value};
use serde_json::{Map, Value};
use skif_core::FileSystem;

use crate::error::WorkspaceError;
use crate::lockfile::{Resolutions, Snapshot, reach, split_key};

/// The sections of a workspace or package entry whose packages are installed for it.
const INSTALLED: [&str; 3] = ["dependencies", "devDependencies", "optionalDependencies"];

/// The sections of a resolved package whose packages are installed with it, as pnpm's snapshots record them.
const PACKAGE_INSTALLED: [&str; 2] = ["dependencies", "optionalDependencies"];

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
    /// The resolved releases by their key in `packages` (`ms`, or `debug/ms` for a copy nested below `debug`, `app/ms`
    /// for one only the workspace package `app` gets), workspace packages left out. A snapshot's `dependencies` are
    /// the keys its `dependencies` and `optionalDependencies` resolve to, as Bun looks them up: below the package's
    /// own key first, then below each key it is nested in, then at the top.
    pub snapshots: BTreeMap<String, Snapshot>,
    /// The keys of `snapshots` the direct dependencies (`dependencies`, `devDependencies`, `optionalDependencies`) of
    /// each workspace resolve to, by workspace directory (`.` for the root, `packages/app`), in file order. A
    /// dependency on a workspace package is not followed: that package is an importer of its own.
    pub importers: BTreeMap<String, Vec<String>>,
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
        let document = parse_bun_lock(source).map_err(|message| WorkspaceError::InvalidLockfile {
            path: path.clone(),
            message,
        })?;
        let Value::Object(document) = document else {
            return Err(WorkspaceError::InvalidLockfile {
                path,
                message: "the lockfile is not an object".to_owned(),
            });
        };
        let empty = Map::new();
        let entries = document.get("packages").and_then(Value::as_object).unwrap_or(&empty);
        let packages = entries
            .values()
            .filter_map(release)
            .filter(|key| split_key(key).is_some_and(|(_, version, _)| !version.starts_with("workspace:")))
            .map(str::to_owned)
            .collect();
        let snapshots = entries
            .iter()
            .filter_map(|(key, entry)| {
                let (name, version, _) = split_key(release(entry)?)?;
                if version.starts_with("workspace:") {
                    return None;
                }
                let dependencies = names_in(info(entry), &PACKAGE_INSTALLED)
                    .filter_map(|dependency| resolve(entries, Some(key.as_str()), dependency))
                    .collect();
                Some((
                    key.clone(),
                    Snapshot {
                        name: name.to_owned(),
                        version: version.to_owned(),
                        patch_hash: None,
                        dependencies,
                    },
                ))
            })
            .collect();
        let importers = document
            .get("workspaces")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(dir, workspace)| {
                let scope = workspace.get("name").and_then(Value::as_str).filter(|_| !dir.is_empty());
                let direct = names_in(Some(workspace), &INSTALLED)
                    .filter_map(|dependency| resolve(entries, scope, dependency))
                    .collect();
                let dir = if dir.is_empty() { "." } else { dir.as_str() };
                (dir.to_owned(), direct)
            })
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
            snapshots,
            importers,
        })
    }

    /// The snapshots `importer` (a key of [`importers`](Self::importers)) reaches through its direct dependencies and
    /// theirs, each with a shortest chain, as [`PnpmLockfile::resolutions`](crate::PnpmLockfile::resolutions) walks
    /// them.
    #[must_use]
    pub fn resolutions(&self, importer: &str) -> Resolutions<'_> {
        reach(&self.snapshots, self.importers.get(importer).into_iter().flatten().cloned())
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

/// The `name@version` of a `packages` entry, its first element.
fn release(entry: &Value) -> Option<&str> {
    entry.as_array()?.first()?.as_str()
}

/// The metadata object of a `packages` entry (its dependencies and binaries), wherever the resolution kind puts it.
fn info(entry: &Value) -> Option<&Value> {
    entry.as_array()?.iter().skip(1).find(|element| element.is_object())
}

/// The package names the `sections` of `object` list, in file order.
fn names_in<'v>(object: Option<&'v Value>, sections: &'v [&str]) -> impl Iterator<Item = &'v str> + 'v {
    sections
        .iter()
        .filter_map(move |section| object?.get(*section)?.as_object())
        .flat_map(Map::keys)
        .map(String::as_str)
}

/// The key of the `packages` entry the dependency `name` of `from` resolves to, as Bun looks it up: `from/name`, then
/// `name` below each key `from` is nested in, then `name` at the top. `from` is a `packages` key, or the name of the
/// workspace package whose own copies are nested below it (`app/ms`); `None` looks at the top only. `None` when nothing
/// matches or the match is a workspace package, which is no snapshot.
fn resolve(entries: &Map<String, Value>, from: Option<&str>, name: &str) -> Option<String> {
    let mut scope = from;
    loop {
        let candidate = scope.map_or_else(|| name.to_owned(), |scope| format!("{scope}/{name}"));
        if let Some(entry) = entries.get(&candidate) {
            let (_, version, _) = split_key(release(entry)?)?;
            return (!version.starts_with("workspace:")).then_some(candidate);
        }
        scope = parent_key(scope?);
    }
}

/// The key a nested `packages` key lies below: `app` for `app/debug`, `@s/web` for `@s/web/debug`, `app` for
/// `app/@types/ms`; `None` for a key at the top (`debug`, `@s/web`).
fn parent_key(key: &str) -> Option<&str> {
    let (rest, _) = key.rsplit_once('/')?;
    let (above, segment) = rest.rsplit_once('/').unwrap_or(("", rest));
    let parent = if segment.starts_with('@') { above } else { rest };
    (!parent.is_empty()).then_some(parent)
}

/// Parses the text of a `bun.lock`: JSON with the trailing commas Bun writes before `}` and `]`, after a byte order
/// mark if there is one. Empty text is `null`.
///
/// # Errors
/// The parser's message when the text is not such a document.
pub fn parse_bun_lock(text: &str) -> Result<Value, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    parse_to_serde_value(text, &ParseOptions::default()).map_err(|error| error.to_string())
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
        assert_eq!(
            lock.versions_of("ms").into_iter().collect::<Vec<_>>(),
            ["2.1.2", "2.1.3"]
        );
        assert_eq!(
            lock.versions_of("@scope/tool").into_iter().collect::<Vec<_>>(),
            ["1.0.0"]
        );
        assert_eq!(
            lock.patched_dependencies,
            BTreeMap::from([("ms@2.1.2".to_owned(), "patches/ms@2.1.2.patch".to_owned())])
        );
    }

    /// What `bun install` (1.4.4) wrote for a root on `ms@2.1.3`, a member `app` on `debug@2.6.9`, `ms@2.0.0` and the
    /// member `@s/web`, and `@s/web` on `debug@4.3.4`, whose `ms@2.1.2` is nested below it.
    const GRAPH: &str = r#"{
  "lockfileVersion": 2,
  "configVersion": 1,
  "workspaces": {
    "": {
      "name": "root",
      "dependencies": {
        "ms": "2.1.3",
      },
    },
    "packages/app": {
      "name": "app",
      "dependencies": {
        "@s/web": "workspace:*",
        "debug": "2.6.9",
        "ms": "2.0.0",
      },
      "devDependencies": {
        "@types/ms": "0.7.34",
      },
    },
    "packages/web": {
      "name": "@s/web",
      "dependencies": {
        "debug": "4.3.4",
      },
    },
  },
  "packages": {
    "@s/web": ["@s/web@workspace:packages/web"],

    "@types/ms": ["@types/ms@0.7.34", "", {}, "sha512-a"],

    "app": ["app@workspace:packages/app"],

    "debug": ["debug@4.3.4", "", { "dependencies": { "ms": "2.1.2" }, "peerDependencies": { "supports-color": "*" }, "optionalPeers": ["supports-color"] }, "sha512-b"],

    "ms": ["ms@2.1.3", "", {}, "sha512-c"],

    "app/debug": ["debug@2.6.9", "", { "dependencies": { "ms": "2.0.0" } }, "sha512-d"],

    "app/ms": ["ms@2.0.0", "", {}, "sha512-e"],

    "debug/ms": ["ms@2.1.2", "", {}, "sha512-f"],
  }
}
"#;

    #[test]
    fn importers_reach_the_copies_bun_installs_for_them() {
        let lock = BunLockfile::parse(PathBuf::from(BUN_LOCKFILE), GRAPH).unwrap();
        assert_eq!(
            lock.importers,
            BTreeMap::from([
                (".".to_owned(), vec!["ms".to_owned()]),
                (
                    "packages/app".to_owned(),
                    vec!["app/debug".to_owned(), "app/ms".to_owned(), "@types/ms".to_owned()]
                ),
                ("packages/web".to_owned(), vec!["debug".to_owned()]),
            ])
        );
        assert_eq!(lock.snapshots["app/debug"].dependencies, ["app/ms"]);
        assert_eq!(lock.snapshots["debug"].dependencies, ["debug/ms"]);
        assert_eq!(
            (lock.snapshots["debug/ms"].name.as_str(), lock.snapshots["debug/ms"].version.as_str()),
            ("ms", "2.1.2")
        );
        assert!(!lock.snapshots.contains_key("app") && !lock.snapshots.contains_key("@s/web"));
        let reached = |importer: &str| lock.resolutions(importer).keys().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(reached("packages/app"), ["@types/ms", "app/debug", "app/ms"]);
        assert_eq!(reached("packages/web"), ["debug", "debug/ms"]);
        assert_eq!(
            lock.resolutions("packages/web").chain("debug/ms"),
            Some(vec!["debug", "debug/ms"])
        );
        assert_eq!(reached("."), ["ms"]);
    }

    #[test]
    fn a_nested_key_lies_below_the_key_of_the_package_holding_it() {
        assert_eq!(parent_key("app/debug"), Some("app"));
        assert_eq!(parent_key("@s/web/debug"), Some("@s/web"));
        assert_eq!(parent_key("app/@types/ms"), Some("app"));
        assert_eq!(parent_key("debug"), None);
        assert_eq!(parent_key("@s/web"), None);
    }

    #[test]
    fn a_byte_order_mark_is_skipped() {
        assert_eq!(
            parse_bun_lock("\u{feff}{\"lockfileVersion\": 1,}").unwrap(),
            serde_json::json!({"lockfileVersion": 1})
        );
        let lock = BunLockfile::parse(PathBuf::from(BUN_LOCKFILE), &format!("\u{feff}{LOCK}")).unwrap();
        assert_eq!(lock, BunLockfile::parse(PathBuf::from(BUN_LOCKFILE), LOCK).unwrap());
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
        assert_eq!(
            BunLockfile::read(&fs, Path::new("/r"))
                .unwrap()
                .unwrap()
                .patched_dependencies,
            BTreeMap::from([("ms@2.1.2".to_owned(), "patches/ms@2.1.2.patch".to_owned())])
        );
    }
}
