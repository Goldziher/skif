use self::semversion::SemVersion;
use crate::{
    args::{Args, AutofixSelect, Format},
    rules::{
        BoxIssue,
        empty_dependencies::{DependencyKind, EmptyDependenciesIssue},
        incompatible_peer_dependencies::{
            IncompatiblePeerDependenciesIssue, IncompatiblePeerDependency,
        },
        unordered_dependencies::UnorderedDependenciesIssue,
    },
};
use anyhow::{Result, anyhow};
use indexmap::IndexMap;
use root::RootPackage;
use serde::Deserialize;
use std::{fs, path::PathBuf};

pub mod root;
pub mod semversion;

pub struct PackagesList {
    pub root_package: RootPackage,
    pub packages: Vec<Package>,
    pub packages_issues: Vec<BoxIssue>,
    pub catalogs: Catalogs,
}

type CatalogEntries = IndexMap<String, String>;

/// The dependency catalogs of a workspace, used to resolve `catalog:` specifiers.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Catalogs {
    default: CatalogEntries,
    named: IndexMap<String, CatalogEntries>,
}

impl Catalogs {
    /// `catalogs.default` is an alias of the top-level `catalog`; the latter wins on conflicts.
    pub fn new(mut default: CatalogEntries, mut named: IndexMap<String, CatalogEntries>) -> Self {
        if let Some(explicit_default) = named.shift_remove("default") {
            for (name, version) in explicit_default {
                default.entry(name).or_insert(version);
            }
        }

        Self { default, named }
    }

    /// Adds the entries of `other` that are not already defined.
    pub fn merge(&mut self, other: Self) {
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

    /// Resolves a `catalog:` or `catalog:<name>` specifier of `dependency` to its version.
    /// Returns `None` for other specifiers and for references missing from the catalogs.
    pub fn resolve(&self, dependency: &str, specifier: &str) -> Option<&str> {
        let catalog = specifier.strip_prefix("catalog:")?.trim();
        let entries = if catalog.is_empty() || catalog == "default" {
            &self.default
        } else {
            self.named.get(catalog)?
        };

        entries.get(dependency).map(String::as_str)
    }
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum Workspaces {
    Default(Vec<String>),
    /// <https://classic.yarnpkg.com/blog/2018/02/15/nohoist>
    Yarn {
        packages: Vec<String>,
    },
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub fix: bool,
    pub select: Option<AutofixSelect>,
    pub no_install: bool,
    pub fail_on_warnings: bool,
    pub format: Format,
    pub ignore_dependency: Vec<String>,
    pub ignore_package: Vec<String>,
    pub ignore_rule: Vec<String>,
}

impl Config {
    pub fn merge(&mut self, args: Args) {
        if args.fix {
            self.fix = true;
        }

        if args.no_install {
            self.no_install = true;
        }

        if let Some(select) = args.select {
            self.select = Some(select);
        }

        if args.fail_on_warnings {
            self.fail_on_warnings = true;
        }

        if let Some(format) = args.format {
            self.format = format;
        }

        if !args.ignore_dependency.is_empty() {
            self.ignore_dependency.extend(args.ignore_dependency);
        }

        if !args.ignore_package.is_empty() {
            self.ignore_package.extend(args.ignore_package);
        }

        if !args.ignore_rule.is_empty() {
            self.ignore_rule.extend(args.ignore_rule);
        }
    }
}

#[derive(Deserialize, Debug)]
struct DevEngineDependency {
    #[allow(dead_code)]
    name: String,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
#[allow(dead_code)]
enum DevEngineField {
    Single(DevEngineDependency),
    Multiple(Vec<DevEngineDependency>),
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct DevEngines {
    package_manager: Option<DevEngineField>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct PackageInner {
    name: Option<String>,
    private: Option<bool>,
    workspaces: Option<Workspaces>,
    package_manager: Option<String>,
    dev_engines: Option<DevEngines>,
    dependencies: Option<IndexMap<String, String>>,
    dev_dependencies: Option<IndexMap<String, String>>,
    peer_dependencies: Option<IndexMap<String, String>>,
    optional_dependencies: Option<IndexMap<String, String>>,
    catalog: Option<CatalogEntries>,
    catalogs: Option<IndexMap<String, CatalogEntries>>,
    sherif: Option<Config>,
}

#[derive(Debug)]
pub struct Package {
    path: PathBuf,
    inner: PackageInner,
}

impl Package {
    pub fn new(path: PathBuf) -> Result<Self> {
        if !path.is_dir() {
            return Err(anyhow!("Path {path:?} is not a directory"));
        }

        let package_path = path.join("package.json");

        if !package_path.is_file() {
            return Err(anyhow!("`package.json` not found in {path:?}"));
        }

        let root_package = fs::read_to_string(&package_path)?;
        let package: PackageInner = match serde_json::from_str(&root_package) {
            Ok(package) => package,
            Err(err) => return Err(anyhow!("Error while parsing {package_path:?}: {err}")),
        };

        Ok(Self {
            path,
            inner: package,
        })
    }

    pub fn get_name(&self) -> Option<&String> {
        self.inner.name.as_ref()
    }

    pub fn get_path(&self) -> String {
        self.path.to_string_lossy().to_string()
    }

    pub fn is_private(&self) -> bool {
        self.inner.private.unwrap_or(false)
    }

    pub fn has_package_manager(&self) -> bool {
        if self.inner.package_manager.is_some() {
            return true;
        }

        match &self.inner.dev_engines {
            Some(dev_engines) => dev_engines.package_manager.is_some(),
            None => false,
        }
    }

    fn check_deps(
        deps: Option<&IndexMap<String, String>>,
        dependency_kind: DependencyKind,
    ) -> Option<BoxIssue> {
        if let Some(dependencies) = deps {
            if dependencies.is_empty() {
                return Some(EmptyDependenciesIssue::new(dependency_kind));
            }

            let mut sorted_dependencies = dependencies.clone();
            sorted_dependencies.sort_keys();

            if sorted_dependencies.keys().ne(dependencies.keys()) {
                return Some(UnorderedDependenciesIssue::new(dependency_kind));
            }
        }

        None
    }

    pub fn check_dependencies(&self) -> Option<BoxIssue> {
        Self::check_deps(
            self.inner.dependencies.as_ref(),
            DependencyKind::Dependencies,
        )
    }

    pub fn check_dev_dependencies(&self) -> Option<BoxIssue> {
        Self::check_deps(
            self.inner.dev_dependencies.as_ref(),
            DependencyKind::DevDependencies,
        )
    }

    pub fn check_peer_dependencies(&self) -> Option<BoxIssue> {
        Self::check_deps(
            self.inner.peer_dependencies.as_ref(),
            DependencyKind::PeerDependencies,
        )
    }

    pub fn check_optional_dependencies(&self) -> Option<BoxIssue> {
        Self::check_deps(
            self.inner.optional_dependencies.as_ref(),
            DependencyKind::OptionalDependencies,
        )
    }

    pub fn get_catalogs(&self) -> Catalogs {
        Catalogs::new(
            self.inner.catalog.clone().unwrap_or_default(),
            self.inner.catalogs.clone().unwrap_or_default(),
        )
    }

    fn resolve_version<'a>(catalogs: &'a Catalogs, name: &str, version: &'a str) -> &'a str {
        catalogs.resolve(name, version).unwrap_or(version)
    }

    fn get_deps(
        deps: Option<&IndexMap<String, String>>,
        catalogs: &Catalogs,
    ) -> Option<IndexMap<String, SemVersion>> {
        deps.map(|dependencies| {
            dependencies
                .iter()
                .filter_map(|(name, version)| {
                    let version = Self::resolve_version(catalogs, name, version);

                    SemVersion::parse(version)
                        .ok()
                        .map(|version| (name.clone(), version))
                })
                .collect()
        })
    }

    pub fn get_dependencies(&self, catalogs: &Catalogs) -> Option<IndexMap<String, SemVersion>> {
        Self::get_deps(self.inner.dependencies.as_ref(), catalogs)
    }

    /// The `devDependencies` versions, without the ones also declared in `peerDependencies`:
    /// those test the lowest supported peer range and are not meant to match other packages.
    pub fn get_dev_dependencies(
        &self,
        catalogs: &Catalogs,
    ) -> Option<IndexMap<String, SemVersion>> {
        let peer_dependencies = self.inner.peer_dependencies.as_ref();

        Self::get_deps(self.inner.dev_dependencies.as_ref(), catalogs).map(|mut dependencies| {
            dependencies
                .retain(|name, _| !peer_dependencies.is_some_and(|peers| peers.contains_key(name)));
            dependencies
        })
    }

    pub fn check_incompatible_peer_dependencies(&self, catalogs: &Catalogs) -> Option<BoxIssue> {
        let peer_dependencies = self.inner.peer_dependencies.as_ref()?;
        let dev_dependencies = self.inner.dev_dependencies.as_ref()?;

        let incompatible = peer_dependencies
            .iter()
            .filter_map(|(name, peer)| {
                let dev = dev_dependencies.get(name)?;
                let peer = SemVersion::parse(Self::resolve_version(catalogs, name, peer)).ok()?;
                let dev = SemVersion::parse(Self::resolve_version(catalogs, name, dev)).ok()?;

                (dev.satisfies(&peer) == Some(false)).then(|| IncompatiblePeerDependency {
                    name: name.clone(),
                    peer,
                    dev,
                })
            })
            .collect::<Vec<_>>();

        if incompatible.is_empty() {
            None
        } else {
            Some(IncompatiblePeerDependenciesIssue::new(incompatible))
        }
    }

    pub fn is_ignored(&self, ignored_packages: &[String]) -> bool {
        match self.get_name() {
            Some(name) => ignored_packages.iter().any(|ignored_package| {
                if ignored_package.ends_with('*') {
                    let ignored_package = ignored_package.trim_end_matches('*');

                    name.starts_with(ignored_package)
                        || self.get_path().starts_with(ignored_package)
                } else {
                    ignored_package == name || ignored_package == &self.get_path()
                }
            }),
            None => false,
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn entries(pairs: &[(&str, &str)]) -> CatalogEntries {
        pairs
            .iter()
            .map(|(name, version)| ((*name).to_string(), (*version).to_string()))
            .collect()
    }

    fn catalogs() -> Catalogs {
        Catalogs::new(
            entries(&[("typescript", "5.9.3")]),
            IndexMap::from([
                ("ts6".to_string(), entries(&[("typescript", "6.0.3")])),
                ("default".to_string(), entries(&[("react", "19.0.0")])),
            ]),
        )
    }

    #[test]
    fn resolve_default_catalog() {
        let catalogs = catalogs();

        assert_eq!(catalogs.resolve("typescript", "catalog:"), Some("5.9.3"));
        assert_eq!(
            catalogs.resolve("typescript", "catalog:default"),
            Some("5.9.3")
        );
        assert_eq!(catalogs.resolve("react", "catalog:"), Some("19.0.0"));
    }

    #[test]
    fn resolve_named_catalog() {
        assert_eq!(
            catalogs().resolve("typescript", "catalog:ts6"),
            Some("6.0.3")
        );
    }

    #[test]
    fn resolve_unresolvable() {
        let catalogs = catalogs();

        assert_eq!(catalogs.resolve("typescript", "catalog:missing"), None);
        assert_eq!(catalogs.resolve("vite", "catalog:"), None);
        assert_eq!(catalogs.resolve("typescript", "5.8.0"), None);
        assert_eq!(Catalogs::default().resolve("typescript", "catalog:"), None);
    }

    #[test]
    fn merge_keeps_existing_entries() {
        let mut catalogs = catalogs();
        catalogs.merge(Catalogs::new(
            entries(&[("typescript", "1.0.0"), ("vite", "7.0.0")]),
            IndexMap::from([("ts6".to_string(), entries(&[("vite", "6.0.0")]))]),
        ));

        assert_eq!(catalogs.resolve("typescript", "catalog:"), Some("5.9.3"));
        assert_eq!(catalogs.resolve("vite", "catalog:"), Some("7.0.0"));
        assert_eq!(catalogs.resolve("typescript", "catalog:ts6"), Some("6.0.3"));
        assert_eq!(catalogs.resolve("vite", "catalog:ts6"), Some("6.0.0"));
    }
}
