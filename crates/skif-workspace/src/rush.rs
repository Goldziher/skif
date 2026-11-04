//! `rush.json`: the workspace declaration of a Rush monorepo.
//!
//! The file is JSONC. `projects[].projectFolder` lists the member directories (root-relative, literal paths) and
//! `pnpmVersion`, `npmVersion` or `yarnVersion` names the package manager.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;
use skif_config::PackageManager;
use skif_core::{PathSeg, Span};

use crate::error::WorkspaceError;
use crate::manifest::Manifest;

/// File name of the declaration, relative to the workspace root.
pub(crate) const FILE: &str = "rush.json";

/// One `projectFolder` entry: its position in `projects`, the folder as written and where it is written.
pub(crate) type Project = (usize, String, Option<Span>);

/// The facts of a parsed `rush.json`.
pub(crate) struct Rush {
    pub projects: Vec<Project>,
    pub package_manager: Option<PackageManager>,
}

fn package_manager(value: &Value) -> Option<PackageManager> {
    [
        ("pnpmVersion", PackageManager::Pnpm),
        ("npmVersion", PackageManager::Npm),
        ("yarnVersion", PackageManager::Yarn),
    ]
    .into_iter()
    .find(|(key, _)| value.get(key).is_some_and(Value::is_string))
    .map(|(_, manager)| manager)
}

/// `folder` as a glob that selects exactly that directory: `projectFolder` is a literal path, so the characters a glob
/// would interpret are written as one-character classes.
fn literal(folder: &str) -> String {
    folder
        .chars()
        .map(|character| match character {
            '*' | '?' | '[' | ']' | '{' | '}' => format!("[{character}]"),
            other => other.to_string(),
        })
        .collect()
}

/// Parses `source`, the contents of `rush.json`.
///
/// # Errors
/// [`WorkspaceError::InvalidManifest`] when the file is not valid JSONC or not an object.
pub(crate) fn parse(source: Arc<str>) -> Result<Rush, WorkspaceError> {
    let (manifest, _) = Manifest::parse(PathBuf::from(FILE), source)?;
    let projects = manifest
        .value
        .get("projects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, project)| {
            let folder = project.get("projectFolder")?.as_str()?;
            let pointer = [
                PathSeg::Key("projects".into()),
                PathSeg::Index(index),
                PathSeg::Key("projectFolder".into()),
            ];
            Some((index, literal(folder), manifest.field_value_span(&pointer)))
        })
        .collect();
    Ok(Rush {
        projects,
        package_manager: package_manager(&manifest.value),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn comments_folders_and_manager_are_read() {
        let rush = parse(Arc::from(
            "// header\n{\n  /* pinned */ \"pnpmVersion\": \"9.1.0\",\n  \"projects\": [\n    { \"packageName\": \"a\", \"projectFolder\": \"apps/a\" },\n    { \"packageName\": \"b\" },\n    { \"projectFolder\": \"libs/c\", },\n  ],\n}\n",
        ))
        .unwrap();
        let folders: Vec<(usize, &str)> = rush.projects.iter().map(|(i, f, _)| (*i, f.as_str())).collect();
        assert_eq!(folders, [(0, "apps/a"), (2, "libs/c")]);
        assert!(rush.projects.iter().all(|(_, _, span)| span.is_some()));
        assert_eq!(rush.package_manager, Some(PackageManager::Pnpm));
    }

    #[test]
    fn folders_are_literal_paths_not_globs() {
        let rush = parse(Arc::from(
            r#"{"projects": [{"projectFolder": "apps/[a]{x}*?"}, {"projectFolder": "plain/dir"}]}"#,
        ))
        .unwrap();
        let folders: Vec<&str> = rush.projects.iter().map(|(_, folder, _)| folder.as_str()).collect();
        assert_eq!(folders, ["apps/[[]a[]][{]x[}][*][?]", "plain/dir"]);
    }

    #[test]
    fn manager_keys() {
        let manager = |key: &str| {
            parse(Arc::from(format!("{{\"{key}\": \"1.0.0\"}}")))
                .unwrap()
                .package_manager
        };
        assert_eq!(manager("npmVersion"), Some(PackageManager::Npm));
        assert_eq!(manager("yarnVersion"), Some(PackageManager::Yarn));
        assert_eq!(manager("other"), None);
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(parse(Arc::from("{\"projects\": [")).is_err());
    }
}
