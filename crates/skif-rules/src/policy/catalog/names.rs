//! Naming the catalogs of a workspace and the references that point at them.

use skif_workspace::Catalogs;

/// One catalog of the workspace: the default one or a named one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum CatalogName {
    Default,
    Named(String),
}

impl CatalogName {
    /// The specifier that selects this catalog: `catalog:` or `catalog:<name>`.
    pub(crate) fn reference(&self) -> String {
        match self {
            Self::Default => "catalog:".to_owned(),
            Self::Named(name) => format!("catalog:{name}"),
        }
    }

    /// A human-readable name for messages.
    pub(crate) fn display(&self) -> String {
        match self {
            Self::Default => "the default catalog".to_owned(),
            Self::Named(name) => format!("catalog `{name}`"),
        }
    }

    /// Whether this catalog defines an entry for `dependency`.
    pub(crate) fn defines(&self, catalogs: &Catalogs, dependency: &str) -> bool {
        match self {
            Self::Default => catalogs.default_catalog().contains_key(dependency),
            Self::Named(name) => catalogs
                .named()
                .get(name)
                .is_some_and(|entries| entries.contains_key(dependency)),
        }
    }
}

/// Every catalog that defines at least one entry, the default one first.
pub(crate) fn defined_catalogs(catalogs: &Catalogs) -> Vec<CatalogName> {
    let default = (!catalogs.default_catalog().is_empty()).then_some(CatalogName::Default);
    let named = catalogs
        .named()
        .iter()
        .filter(|(_, entries)| !entries.is_empty())
        .map(|(name, _)| CatalogName::Named(name.clone()));
    default.into_iter().chain(named).collect()
}

/// The catalogs that define an entry for `dependency`.
pub(crate) fn defining(catalogs: &Catalogs, dependency: &str) -> Vec<CatalogName> {
    defined_catalogs(catalogs)
        .into_iter()
        .filter(|catalog| catalog.defines(catalogs, dependency))
        .collect()
}

/// The catalog a `catalog:` specifier selects.
pub(crate) fn referenced(specifier: &str) -> Option<CatalogName> {
    let name = specifier.strip_prefix("catalog:")?.trim();
    Some(if name.is_empty() || name == "default" {
        CatalogName::Default
    } else {
        CatalogName::Named(name.to_owned())
    })
}
