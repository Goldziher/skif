//! `lerna.json`: the workspace declaration of a Lerna monorepo that does not use package-manager workspaces.
//!
//! `packages` lists the member globs, as the `workspaces` of `package.json` do, and `npmClient` names the package
//! manager. syncpack reads the same file when nothing else declares the packages; skif reads it last, after
//! `package.json`, `pnpm-workspace.yaml` and `rush.json`.

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
}

/// Parses `source`, the contents of `lerna.json`.
///
/// # Errors
/// [`WorkspaceError::InvalidManifest`] when the file is not valid JSON or not an object.
pub(crate) fn parse(source: Arc<str>) -> Result<Lerna, WorkspaceError> {
    let (manifest, _) = Manifest::parse(PathBuf::from(FILE), source)?;
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
    Ok(Lerna {
        packages,
        package_manager,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn packages_and_client_are_read() {
        let lerna = parse(Arc::from(
            r#"{"version": "independent", "npmClient": "yarn", "packages": ["packages/*", 3, "tools/**"]}"#,
        ))
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
    }

    #[test]
    fn a_file_without_packages_declares_nothing() {
        let lerna = parse(Arc::from(r#"{"version": "1.0.0", "useWorkspaces": true}"#)).unwrap();
        assert!(lerna.packages.is_none());
        assert_eq!(lerna.package_manager, None);
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(parse(Arc::from("{\"packages\": [")).is_err());
    }
}
