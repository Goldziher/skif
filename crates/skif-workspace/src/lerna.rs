//! `lerna.json`: the workspace declaration of a Lerna monorepo that does not use package-manager workspaces.
//!
//! `packages` lists the member globs, as the `workspaces` of `package.json` do, and `npmClient` names the package
//! manager. syncpack reads the same file when nothing else declares the packages; skif reads it last, and only
//! when none of `package.json`, `pnpm-workspace.yaml` and `rush.json` does; an unreadable or invalid file then
//! declares nothing. Lerna itself reads `packages` first: when another file declares the members too, discovery
//! records what each list selects that the other does not ([`LernaFacts`]), and rules judge it.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use skif_config::PackageManager;
use skif_core::{PathSeg, Span};

use crate::error::WorkspaceError;
use crate::manifest::Manifest;

/// File name of the declaration, relative to the workspace root.
pub(crate) const FILE: &str = "lerna.json";

/// One `packages` entry: its position, the glob as written and where it is written.
pub(crate) type Glob = (usize, String, Option<Span>);

/// The facts of a parsed `lerna.json`.
pub(crate) struct Lerna {
    /// `None` when the file has no `packages` array: it then declares nothing.
    pub packages: Option<Vec<Glob>>,
    pub package_manager: Option<PackageManager>,
    /// Where `packages` is written, key through value.
    pub packages_span: Option<Span>,
    /// The value of `useWorkspaces` and where the key is written, key through value, when the file has the key.
    pub use_workspaces: Option<(bool, Option<Span>)>,
    /// Whether `version` is a string.
    pub has_version: bool,
}

/// The `lerna.json` next to the root `package.json`, when it is valid JSON, as discovery read it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LernaFacts {
    /// Path of the file relative to the workspace root.
    pub path: PathBuf,
    /// The `packages` globs as written, in order; `None` when the file has no `packages` array.
    pub packages: Option<Vec<String>>,
    /// Where `packages` is written, key through value.
    pub packages_span: Option<Span>,
    /// Whether `useWorkspaces` is `true`, when the file has the key (Lerna 7 removed it).
    pub use_workspaces: Option<bool>,
    /// Where `useWorkspaces` is written, key through value.
    pub use_workspaces_span: Option<Span>,
    /// Whether `version` is a string; Lerna 7 and later fail without one.
    pub has_version: bool,
    /// Directories with a `package.json` that `packages` selects and the workspace declaration does not, relative to
    /// the root and sorted. Empty when `packages` is what declares the members, or when the file has no `packages`.
    pub only_in_lerna: Vec<PathBuf>,
    /// Directories with a `package.json` that the workspace declaration selects and `packages` does not, relative to
    /// the root and sorted; empty as [`only_in_lerna`](Self::only_in_lerna) is.
    pub only_in_workspace: Vec<PathBuf>,
}

impl LernaFacts {
    pub(crate) fn new(lerna: &Lerna) -> Self {
        Self {
            path: PathBuf::from(FILE),
            packages: lerna
                .packages
                .as_ref()
                .map(|packages| packages.iter().map(|(_, glob, _)| glob.clone()).collect()),
            packages_span: lerna.packages_span,
            use_workspaces: lerna.use_workspaces.map(|(value, _)| value),
            use_workspaces_span: lerna.use_workspaces.and_then(|(_, span)| span),
            has_version: lerna.has_version,
            only_in_lerna: Vec::new(),
            only_in_workspace: Vec::new(),
        }
    }
}

/// Parses `source`, the contents of the `lerna.json` at `path`.
///
/// # Errors
/// [`WorkspaceError::InvalidManifest`] naming `path` when the file is not valid JSON or not an object.
pub(crate) fn parse(path: PathBuf, source: Arc<str>) -> Result<Lerna, WorkspaceError> {
    let (manifest, _) = Manifest::parse(path, source)?;
    let packages = manifest.value.get("packages").and_then(Value::as_array).map(|items| {
        items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                let glob = item.as_str()?;
                let pointer = [PathSeg::Key("packages".into()), PathSeg::Index(index)];
                Some((index, glob.to_owned(), manifest.field_value_span(&pointer)))
            })
            .collect()
    });
    let package_manager = match manifest.value.get("npmClient").and_then(Value::as_str) {
        Some("npm") => Some(PackageManager::Npm),
        Some("yarn") => Some(PackageManager::Yarn),
        Some("pnpm") => Some(PackageManager::Pnpm),
        Some("bun") => Some(PackageManager::Bun),
        _ => None,
    };
    let key = |name: &str| [PathSeg::Key(name.into())];
    let use_workspaces = manifest
        .value
        .get("useWorkspaces")
        .map(|value| (value.as_bool() == Some(true), manifest.field_span(&key("useWorkspaces"))));
    Ok(Lerna {
        packages,
        package_manager,
        packages_span: manifest.field_span(&key("packages")),
        use_workspaces,
        has_version: manifest.value.get("version").is_some_and(Value::is_string),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn packages_and_client_are_read() {
        let lerna = parse(
            PathBuf::from(FILE),
            Arc::from(r#"{"version": "independent", "npmClient": "yarn", "packages": ["packages/*", 3, "tools/**"]}"#),
        )
        .unwrap();
        let globs: Vec<(usize, &str)> = lerna
            .packages
            .iter()
            .flatten()
            .map(|(index, glob, _)| (*index, glob.as_str()))
            .collect();
        assert_eq!(globs, [(0, "packages/*"), (2, "tools/**")]);
        assert!(lerna.packages.iter().flatten().all(|(_, _, span)| span.is_some()));
        assert_eq!(lerna.package_manager, Some(PackageManager::Yarn));
        assert!(lerna.packages_span.is_some());
        assert!(lerna.has_version);
        assert!(lerna.use_workspaces.is_none());
    }

    #[test]
    fn use_workspaces_and_a_missing_version_are_read() {
        let source = r#"{"useWorkspaces": true, "npmClient": "npm"}"#;
        let lerna = parse(PathBuf::from(FILE), Arc::from(source)).unwrap();
        let (value, span) = lerna.use_workspaces.unwrap();
        assert!(value);
        let span = span.unwrap();
        assert_eq!(&source[span.start as usize..span.end as usize], r#""useWorkspaces": true"#);
        assert!(!lerna.has_version);
        let facts = LernaFacts::new(&lerna);
        assert_eq!(facts.use_workspaces, Some(true));
        assert_eq!(facts.use_workspaces_span, Some(span));
        assert_eq!(facts.packages, None);
    }

    #[test]
    fn a_false_use_workspaces_and_a_numeric_version_are_read_as_written() {
        let lerna = parse(
            PathBuf::from(FILE),
            Arc::from(r#"{"useWorkspaces": false, "version": 7}"#),
        )
        .unwrap();
        assert_eq!(lerna.use_workspaces.map(|(value, _)| value), Some(false));
        assert!(!lerna.has_version);
    }

    #[test]
    fn a_file_without_packages_declares_nothing() {
        let lerna = parse(
            PathBuf::from(FILE),
            Arc::from(r#"{"version": "1.0.0", "useWorkspaces": true}"#),
        )
        .unwrap();
        assert!(lerna.packages.is_none());
        assert_eq!(lerna.package_manager, None);
    }

    #[test]
    fn invalid_json_is_an_error_naming_the_path() {
        let path = PathBuf::from("/repo").join(FILE);
        let error = parse(path.clone(), Arc::from("{\"packages\": [")).err().unwrap();
        assert!(matches!(error, WorkspaceError::InvalidManifest { path: named, .. } if named == path));
    }
}
