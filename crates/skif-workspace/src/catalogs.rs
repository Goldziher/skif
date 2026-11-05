//! Dependency catalogs from `pnpm-workspace.yaml`, `.yarnrc.yml`, the root `package.json` and bun's `workspaces` object.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde_json::Value;

type Entries = IndexMap<String, String>;

/// The dependency catalogs of a workspace, used to resolve `catalog:` specifiers.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Catalogs {
    default: Entries,
    named: IndexMap<String, Entries>,
    source: Option<PathBuf>,
}

impl Catalogs {
    /// Builds catalogs. `catalogs.default` is an alias of the top-level `catalog`; the latter wins on conflicts.
    #[must_use]
    pub fn new(mut default: Entries, mut named: IndexMap<String, Entries>) -> Self {
        if let Some(explicit_default) = named.shift_remove("default") {
            for (name, version) in explicit_default {
                default.entry(name).or_insert(version);
            }
        }
        Self {
            default,
            named,
            source: None,
        }
    }

    /// Records the file the catalogs came from.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<PathBuf>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Whether no catalog defines any entry.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.default.is_empty() && self.named.values().all(IndexMap::is_empty)
    }

    /// Adds the entries of `other` that are not already defined. The source stays the first non-empty one.
    pub fn merge(&mut self, other: Self) {
        if other.is_empty() {
            return;
        }
        if self.is_empty() || self.source.is_none() {
            self.source = other.source;
        }
        for (name, version) in other.default {
            self.default.entry(name).or_insert(version);
        }
        for (catalog, entries) in other.named {
            let existing = self.named.entry(catalog).or_default();
            for (name, version) in entries {
                existing.entry(name).or_insert(version);
            }
        }
    }

    /// Resolves a `catalog:`, `catalog:default` or `catalog:<name>` specifier of `dependency` to its version.
    /// Returns `None` for other specifiers and for references missing from the catalogs.
    #[must_use]
    pub fn resolve(&self, dependency: &str, specifier: &str) -> Option<&str> {
        let catalog = specifier.strip_prefix("catalog:")?.trim();
        let entries = if catalog.is_empty() || catalog == "default" {
            &self.default
        } else {
            self.named.get(catalog)?
        };
        entries.get(dependency).map(String::as_str)
    }

    /// The default catalog.
    #[must_use]
    pub fn default_catalog(&self) -> &IndexMap<String, String> {
        &self.default
    }

    /// The named catalogs.
    #[must_use]
    pub fn named(&self) -> &IndexMap<String, IndexMap<String, String>> {
        &self.named
    }

    /// The file the catalogs came from.
    #[must_use]
    pub fn source(&self) -> Option<&Path> {
        self.source.as_deref()
    }
}

/// Reads a `name: version` mapping; YAML numbers such as `18` are accepted, other value types are skipped.
pub(crate) fn entries_from_json(value: &Value) -> Entries {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter_map(|(name, version)| scalar(version).map(|version| (name.clone(), version)))
        .collect()
}

fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// Builds catalogs from a `catalog` value and a `catalogs` value.
pub(crate) fn from_json(catalog: Option<&Value>, catalogs: Option<&Value>) -> Catalogs {
    let default = catalog.map(entries_from_json).unwrap_or_default();
    let named = catalogs
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, entries)| (name.clone(), entries_from_json(entries)))
        .collect();
    Catalogs::new(default, named)
}

/// Catalogs declared in a root `package.json`: `catalog`/`catalogs` at the top level and, for bun, inside the
/// object form of `workspaces`.
pub(crate) fn from_package_json(manifest: &Value, source: &Path) -> Catalogs {
    let mut catalogs = from_json(manifest.get("catalog"), manifest.get("catalogs"));
    if let Some(workspaces) = manifest.get("workspaces").filter(|value| value.is_object()) {
        catalogs.merge(from_json(workspaces.get("catalog"), workspaces.get("catalogs")));
    }
    if catalogs.is_empty() {
        Catalogs::default()
    } else {
        catalogs.with_source(source)
    }
}

/// Catalogs declared in `.yarnrc.yml` (Yarn 4.10+): `catalog` is the default catalog, `catalogs` holds the named
/// ones.
///
/// # Errors
/// The parser's message when the text is not valid YAML; the file then declares no catalogs.
pub(crate) fn from_yarnrc(text: &str, source: &Path) -> Result<Catalogs, String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let document = serde_saphyr::from_str::<Value>(text).map_err(|error| error.to_string())?;
    let catalogs = from_json(document.get("catalog"), document.get("catalogs"));
    Ok(if catalogs.is_empty() {
        Catalogs::default()
    } else {
        catalogs.with_source(source)
    })
}
