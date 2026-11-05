//! Dependency catalogs from `pnpm-workspace.yaml`, `.yarnrc.yml`, the root `package.json` and bun's `workspaces` object.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde_json::Value;

type Entries = IndexMap<String, String>;

/// The default catalog (`None`) or a named one.
type CatalogKey = Option<String>;

/// The dependency catalogs of a workspace, used to resolve `catalog:` specifiers.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Catalogs {
    default: Entries,
    named: IndexMap<String, Entries>,
    source: Option<PathBuf>,
    /// The file that first declared each catalog.
    sources: IndexMap<CatalogKey, PathBuf>,
    /// Whether the default catalog is declared as `catalogs.default` instead of `catalog`.
    default_as_named: bool,
}

impl Catalogs {
    /// Builds catalogs. `catalogs.default` is an alias of the top-level `catalog`; the latter wins on conflicts.
    #[must_use]
    pub fn new(mut default: Entries, mut named: IndexMap<String, Entries>) -> Self {
        let mut default_as_named = false;
        if let Some(explicit_default) = named.shift_remove("default") {
            default_as_named = default.is_empty() && !explicit_default.is_empty();
            for (name, version) in explicit_default {
                default.entry(name).or_insert(version);
            }
        }
        Self {
            default,
            named,
            source: None,
            sources: IndexMap::new(),
            default_as_named,
        }
    }

    /// Records the file the catalogs came from.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<PathBuf>) -> Self {
        let source = source.into();
        let defined = (!self.default.is_empty())
            .then_some(None)
            .into_iter()
            .chain(
                self.named
                    .iter()
                    .filter(|(_, entries)| !entries.is_empty())
                    .map(|(name, _)| Some(name.clone())),
            );
        self.sources = defined.map(|key| (key, source.clone())).collect();
        self.source = Some(source);
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
        for (key, source) in other.sources {
            self.sources.entry(key).or_insert(source);
        }
        if self.default.is_empty() {
            self.default_as_named = other.default_as_named;
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
        let catalog = specifier.strip_prefix("catalog:")?;
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

    /// The file that first declared the catalog `name` (`None`: the default catalog), or [`Catalogs::source`] when
    /// that is not recorded.
    #[must_use]
    pub fn source_of(&self, name: Option<&str>) -> Option<&Path> {
        self.sources
            .get(&name.map(str::to_owned))
            .map(PathBuf::as_path)
            .or_else(|| self.source())
    }

    /// Whether the default catalog is declared as `catalogs.default` rather than as `catalog`, which is where new
    /// entries of it belong: pnpm rejects a workspace that declares the default catalog both ways.
    #[must_use]
    pub const fn default_declared_as_named(&self) -> bool {
        self.default_as_named
    }
}

/// Reads a `name: version` mapping; strings and whole numbers such as `18` are accepted, other value types are
/// skipped. A fractional number is skipped as well: YAML hands it over as a float, which has lost the spelling of the
/// version (`1.10` reads as `1.1`), so the entry has to be quoted.
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
        Value::Number(number) if number.is_i64() || number.is_u64() => Some(number.to_string()),
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn whole_numbers_are_versions_and_fractions_are_not_guessed() {
        let catalogs = from_json(
            Some(&json!({"major": 18, "fraction": 1.10, "text": "1.10"})),
            None,
        );
        assert_eq!(catalogs.resolve("major", "catalog:"), Some("18"));
        assert_eq!(catalogs.resolve("fraction", "catalog:"), None);
        assert_eq!(catalogs.resolve("text", "catalog:"), Some("1.10"));
    }

    #[test]
    fn a_reference_with_a_space_names_no_catalog() {
        let catalogs = from_json(Some(&json!({"react": "^18.3.1"})), Some(&json!({"legacy": {"react": "^17.0.0"}})));
        assert_eq!(catalogs.resolve("react", "catalog:"), Some("^18.3.1"));
        assert_eq!(catalogs.resolve("react", "catalog: legacy"), None);
        assert_eq!(catalogs.resolve("react", "catalog: "), None);
    }

    #[test]
    fn each_catalog_remembers_the_file_that_declared_it() {
        let yaml = from_json(Some(&json!({"react": "^18.3.1"})), None).with_source("pnpm-workspace.yaml");
        let manifest = from_json(None, Some(&json!({"legacy": {"react": "^17.0.0"}}))).with_source("package.json");
        let mut merged = yaml;
        merged.merge(manifest);
        assert_eq!(merged.source(), Some(Path::new("pnpm-workspace.yaml")));
        assert_eq!(merged.source_of(None), Some(Path::new("pnpm-workspace.yaml")));
        assert_eq!(merged.source_of(Some("legacy")), Some(Path::new("package.json")));
    }

    #[test]
    fn the_default_catalog_remembers_how_it_was_declared() {
        let named = from_json(None, Some(&json!({"default": {"react": "^18.3.1"}})));
        assert!(named.default_declared_as_named());
        assert_eq!(named.resolve("react", "catalog:"), Some("^18.3.1"));
        assert!(!from_json(Some(&json!({"react": "^18.3.1"})), None).default_declared_as_named());
        let both = from_json(
            Some(&json!({"react": "^18.3.1"})),
            Some(&json!({"default": {"zod": "^3.0.0"}})),
        );
        assert!(!both.default_declared_as_named());
    }
}
