//! A workspace member (or the root) with its parsed manifest.

use std::path::PathBuf;
use std::sync::Arc;

use globset::Glob;
use serde_json::{Map, Value};

use crate::error::WorkspaceError;
use crate::manifest::{DependencyEntry, DependencyKind, Manifest};

const GLOB_CHARS: [char; 4] = ['*', '?', '[', '{'];

/// One `package.json` of the workspace.
#[derive(Debug, Clone)]
pub struct Package {
    /// The `name` field.
    pub name: Option<String>,
    /// The `version` field.
    pub version: Option<String>,
    /// Workspace-relative directory with `/` separators; empty for the root.
    pub dir: PathBuf,
    /// The parsed manifest.
    pub manifest: Manifest,
    /// Whether `"private": true`.
    pub private: bool,
    /// Every string dependency entry in file order, across all sections.
    pub dependencies: Vec<DependencyEntry>,
    /// Whether this is the workspace root package.
    pub is_root: bool,
}

impl Package {
    /// Parses the manifest of the package in `dir` (workspace-relative, `/`-separated, empty for the root).
    ///
    /// # Errors
    /// [`WorkspaceError::InvalidManifest`] when the JSON is invalid.
    pub fn parse(dir: PathBuf, source: Arc<str>, is_root: bool) -> Result<Self, WorkspaceError> {
        let manifest_path = if dir.as_os_str().is_empty() {
            PathBuf::from("package.json")
        } else {
            PathBuf::from(format!("{}/package.json", dir.to_string_lossy()))
        };
        let (manifest, dependencies) = Manifest::parse(manifest_path, source)?;
        let text = |field: &str| manifest.value.get(field).and_then(Value::as_str).map(str::to_owned);
        Ok(Self {
            name: text("name"),
            version: text("version"),
            private: manifest.value.get("private").and_then(Value::as_bool).unwrap_or(false),
            dir,
            manifest,
            dependencies,
            is_root,
        })
    }

    /// The name, or the directory (`.` for an unnamed root).
    #[must_use]
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .or_else(|| self.dir.to_str().filter(|dir| !dir.is_empty()))
            .unwrap_or(".")
    }

    /// The raw dependency object of one section.
    #[must_use]
    pub fn dependency_map(&self, kind: DependencyKind) -> Option<&Map<String, Value>> {
        self.manifest.value.get(kind.field())?.as_object()
    }

    /// Whether `pattern` selects this package by name or directory: an exact match, a trailing-`*` prefix (`@repo/*`,
    /// `legacy-*`), or any glob.
    #[must_use]
    pub fn matches(&self, pattern: &str) -> bool {
        let dir = self.dir.to_string_lossy();
        let candidates: Vec<&str> = [self.name.as_deref(), Some(dir.as_ref())]
            .into_iter()
            .flatten()
            .filter(|candidate| !candidate.is_empty())
            .collect();
        if let Some(prefix) = pattern.strip_suffix('*')
            && !prefix.contains(GLOB_CHARS)
        {
            return candidates.iter().any(|candidate| candidate.starts_with(prefix));
        }
        if candidates.contains(&pattern) {
            return true;
        }
        Glob::new(pattern).is_ok_and(|glob| {
            let matcher = glob.compile_matcher();
            candidates.iter().any(|candidate| matcher.is_match(candidate))
        })
    }
}
