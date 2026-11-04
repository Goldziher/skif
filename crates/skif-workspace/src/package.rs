//! A workspace member (or the root) with its parsed manifest.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};
use skif_config::Selector;
use skif_core::DependencyKind;
use skif_specifier::SpecifierCache;
pub use skif_core::PackageId;

use crate::error::WorkspaceError;
use crate::manifest::{DependencyEntry, Manifest};

/// One `package.json` of the workspace.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Package {
    /// Identifies the package in its workspace; [`PackageId::ROOT`] for a package parsed on its own.
    pub id: PackageId,
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
    /// The package gets [`PackageId::ROOT`]; discovery assigns the real id with [`Package::with_id`].
    ///
    /// # Errors
    /// [`WorkspaceError::InvalidManifest`] when the JSON is invalid.
    pub fn parse(dir: PathBuf, source: Arc<str>, is_root: bool) -> Result<Self, WorkspaceError> {
        Self::parse_with(dir, source, is_root, &SpecifierCache::new())
    }

    /// Like [`parse`](Self::parse), sharing parsed specifiers with every other package parsed through `cache`.
    ///
    /// # Errors
    /// [`WorkspaceError::InvalidManifest`] when the JSON is invalid.
    pub fn parse_with(
        dir: PathBuf,
        source: Arc<str>,
        is_root: bool,
        cache: &SpecifierCache,
    ) -> Result<Self, WorkspaceError> {
        let manifest_path = if dir.as_os_str().is_empty() {
            PathBuf::from("package.json")
        } else {
            PathBuf::from(format!("{}/package.json", dir.to_string_lossy()))
        };
        let (manifest, dependencies) = Manifest::parse_with(manifest_path, source, cache)?;
        let text = |field: &str| manifest.value.get(field).and_then(Value::as_str).map(str::to_owned);
        Ok(Self {
            id: PackageId::ROOT,
            name: text("name"),
            version: text("version"),
            private: manifest.value.get("private").and_then(Value::as_bool).unwrap_or(false),
            dir,
            manifest,
            dependencies,
            is_root,
        })
    }

    /// The same package with `id`.
    #[must_use]
    pub fn with_id(mut self, id: PackageId) -> Self {
        self.id = id;
        self
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

    /// Whether `selector` selects this package by its name or its directory. A workspace package is `$LOCAL`.
    #[must_use]
    pub fn is_selected_by(&self, selector: &Selector) -> bool {
        let dir = self.dir.to_string_lossy();
        let names = self
            .name
            .as_deref()
            .into_iter()
            .chain(Some(&*dir))
            .filter(|name| !name.is_empty());
        selector.matches_any(names, true)
    }
}
