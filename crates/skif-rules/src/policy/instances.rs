//! The dependency instances every policy rule works on.
//!
//! One pass over the workspace builds an [`Instance`] for each dependency entry of each active package, plus one for
//! each package's own `version` (the "local" instance that dependents are aligned with). Instances are grouped by
//! dependency name, which is the unit version groups, the catalog policy and the workspace protocol reason about.
//!
//! The model is shared: the version-group analysis in this module tree and the catalog rules build on the same
//! [`Instances`], so a manifest is walked once per run.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;

use skif_config::Config;
use skif_core::{DependencyKind, FixOp, JsonAction, Location, PathSeg};
use skif_specifier::Specifier;
use skif_workspace::{DependencyEntry, Package, Workspace};

use super::catalog;
use super::specifiers::alias_target;
use crate::manifest::support;

/// Whether the catalog rules, not the policy rules, report `entry` when its dependency is catalog-owned: a plain
/// version in a section that can hold a catalog reference.
fn owned_by_catalog(entry: &DependencyEntry) -> bool {
    entry.kind != DependencyKind::PeerDependencies && catalog::is_catalogable(&entry.parsed)
}

/// Index of an [`Instance`] in its [`Instances`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct InstanceId(usize);

impl InstanceId {
    /// Position in the order instances were collected: packages in workspace order, the package's own version first,
    /// then its dependency entries in manifest order.
    pub(crate) const fn index(self) -> usize {
        self.0
    }
}

/// Where an instance was read from.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Origin<'w> {
    /// An entry of a dependency section of `package.json`.
    Dependency(&'w DependencyEntry),
    /// The `version` field of a workspace package.
    OwnVersion,
}

/// One declared version of a dependency.
#[derive(Debug, Clone)]
pub(crate) struct Instance<'w> {
    pub(crate) package: &'w Package,
    pub(crate) origin: Origin<'w>,
    /// The dependency name; for an own version, the package name.
    pub(crate) name: String,
    pub(crate) specifier: Arc<Specifier>,
    /// Whether `name` is itself a package of the workspace (what `$LOCAL` selects).
    pub(crate) is_local: bool,
    /// Whether the catalog rules report this instance against its catalog, so that no version group takes it unless
    /// one names it explicitly.
    pub(crate) catalog_owned: bool,
}

impl Instance<'_> {
    /// The dependency section, or `None` for the package's own version.
    pub(crate) fn kind(&self) -> Option<DependencyKind> {
        match self.origin {
            Origin::Dependency(entry) => Some(entry.kind),
            Origin::OwnVersion => None,
        }
    }

    pub(crate) fn is_own_version(&self) -> bool {
        matches!(self.origin, Origin::OwnVersion)
    }

    /// The specifier as written in the manifest.
    pub(crate) fn raw(&self) -> &str {
        self.specifier.get_raw()
    }

    /// The package an `npm:` alias installs in place of the dependency, when that is another package.
    pub(crate) fn alias_target(&self) -> Option<&str> {
        alias_target(&self.specifier, &self.name)
    }

    /// The whole entry for a dependency, the `version` member for an own version.
    pub(crate) fn location(&self) -> Location {
        match self.origin {
            Origin::Dependency(entry) => support::entry_location(self.package, entry),
            Origin::OwnVersion => support::field_location(self.package, &[support::key("version")]),
        }
    }

    /// The JSON pointer of the entry in the manifest; `None` for an own version.
    pub(crate) fn pointer(&self) -> Option<Vec<PathSeg>> {
        match self.origin {
            Origin::Dependency(entry) => Some(vec![support::key(entry.kind.field()), support::key(&entry.name)]),
            Origin::OwnVersion => None,
        }
    }

    /// The operation writing `specifier` over this entry; `None` for an own version, which is never rewritten.
    pub(crate) fn set_specifier(&self, specifier: &str) -> Option<FixOp> {
        let pointer = self.pointer()?;
        Some(support::json_op(
            self.package,
            pointer,
            JsonAction::Set(serde_json::Value::String(specifier.to_owned())),
        ))
    }

    /// The operation removing this entry; `None` for an own version.
    pub(crate) fn remove(&self) -> Option<FixOp> {
        let pointer = self.pointer()?;
        Some(support::json_op(self.package, pointer, JsonAction::Remove))
    }
}

/// Every dependency instance of the workspace, indexed by [`InstanceId`] and by dependency name.
#[derive(Debug)]
pub(crate) struct Instances<'w> {
    workspace: &'w Workspace,
    items: Vec<Instance<'w>>,
    by_name: BTreeMap<String, Vec<InstanceId>>,
}

impl<'w> Instances<'w> {
    /// Collects the instances of every package the workspace does not ignore: every entry of every dependency
    /// section, and each package's own version, which dependents are aligned with whatever section they use.
    ///
    /// Which of them a version group governs is decided when they are assigned to groups (see
    /// [`Groups::assign`](super::groups::Groups::assign)). With `catalog_owns`, dependencies the catalog rules report
    /// (see [`catalog::owned_dependencies`]) are flagged [`catalog_owned`](Instance::catalog_owned).
    pub(crate) fn collect(workspace: &'w Workspace, config: &Config, catalog_owns: bool) -> Self {
        let local_names: HashSet<&str> = workspace
            .all_packages()
            .filter_map(|package| package.name.as_deref())
            .collect();
        let owned = if catalog_owns {
            catalog::owned_dependencies(workspace, &config.catalog)
        } else {
            BTreeSet::new()
        };
        let mut items = Vec::new();
        for package in support::active_packages(workspace) {
            if let (Some(name), Some(version)) = (&package.name, &package.version) {
                items.push(Instance {
                    package,
                    origin: Origin::OwnVersion,
                    name: name.clone(),
                    specifier: Specifier::new(version),
                    is_local: true,
                    catalog_owned: false,
                });
            }
            for entry in &package.dependencies {
                items.push(Instance {
                    package,
                    origin: Origin::Dependency(entry),
                    name: entry.name.clone(),
                    specifier: Arc::clone(&entry.parsed),
                    is_local: local_names.contains(entry.name.as_str()),
                    catalog_owned: owned.contains(entry.name.as_str()) && owned_by_catalog(entry),
                });
            }
        }
        let mut by_name: BTreeMap<String, Vec<InstanceId>> = BTreeMap::new();
        for (index, item) in items.iter().enumerate() {
            by_name.entry(item.name.clone()).or_default().push(InstanceId(index));
        }
        Self {
            workspace,
            items,
            by_name,
        }
    }

    /// The workspace the instances were read from.
    #[allow(
        dead_code,
        reason = "the entry point of the catalog rules, which resolve catalogs against the workspace"
    )]
    pub(crate) const fn workspace(&self) -> &'w Workspace {
        self.workspace
    }

    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// The instance behind `id`; ids come from this collection.
    pub(crate) fn get(&self, id: InstanceId) -> &Instance<'w> {
        &self.items[id.0]
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = (InstanceId, &Instance<'w>)> {
        self.items
            .iter()
            .enumerate()
            .map(|(index, item)| (InstanceId(index), item))
    }

    /// Dependency names in byte order, each with its instances in collection order.
    #[allow(dead_code, reason = "the catalog rules walk dependencies by name")]
    pub(crate) fn names(&self) -> impl Iterator<Item = (&str, &[InstanceId])> {
        self.by_name.iter().map(|(name, ids)| (name.as_str(), ids.as_slice()))
    }

    /// The instances of dependency `name`.
    pub(crate) fn named(&self, name: &str) -> &[InstanceId] {
        self.by_name.get(name).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{config, workspace};

    const ROOT: &str =
        r#"{"name":"root","version":"0.0.0","private":true,"packageManager":"pnpm@9.0.0","workspaces":["packages/*"]}"#;
    const APP: &str = r#"{"name":"app","version":"1.0.0","dependencies":{"lib":"1.0.0","react":"^18.0.0"},"peerDependencies":{"react":"^18.0.0"}}"#;
    const LIB: &str = r#"{"name":"lib","version":"1.0.0"}"#;
    const NO_VERSION: &str = r#"{"name":"bare"}"#;

    fn files() -> [(&'static str, &'static str); 4] {
        [
            ("package.json", ROOT),
            ("packages/app/package.json", APP),
            ("packages/lib/package.json", LIB),
            ("packages/bare/package.json", NO_VERSION),
        ]
    }

    #[test]
    fn collects_dependencies_and_own_versions_grouped_by_name() {
        let config = config("");
        let workspace = workspace(&files(), &config);
        let instances = Instances::collect(&workspace, &config, true);
        let summary: Vec<(&str, usize)> = instances.names().map(|(name, ids)| (name, ids.len())).collect();
        assert_eq!(
            summary,
            [("app", 1), ("lib", 2), ("react", 2), ("root", 1)],
            "bare has no version and so no own-version instance"
        );
    }

    #[test]
    fn own_versions_and_local_dependencies_are_flagged() {
        let config = config("");
        let workspace = workspace(&files(), &config);
        let instances = Instances::collect(&workspace, &config, true);
        let lib: Vec<(bool, bool, Option<DependencyKind>)> = instances
            .named("lib")
            .iter()
            .map(|&id| {
                let instance = instances.get(id);
                (instance.is_own_version(), instance.is_local, instance.kind())
            })
            .collect();
        assert_eq!(
            lib,
            [(false, true, Some(DependencyKind::Dependencies)), (true, true, None)]
        );
        assert!(instances.named("react").iter().all(|&id| !instances.get(id).is_local));
    }

    #[test]
    fn every_dependency_section_is_collected_whatever_the_policy_dependency_types() {
        for toml in [
            "",
            "[policy]\ndependency-types = [\"dev\"]\n",
            "[policy]\ndependency-types = [\"prod\", \"peer\"]\n",
        ] {
            let config = config(toml);
            let workspace = workspace(&files(), &config);
            let instances = Instances::collect(&workspace, &config, true);
            assert_eq!(instances.named("react").len(), 2, "{toml}");
            assert_eq!(instances.named("lib").len(), 2, "{toml}");
        }
    }

    #[test]
    fn ignored_packages_contribute_nothing() {
        let config = config("[workspace]\nignore-packages = [\"app\"]\n");
        let workspace = workspace(&files(), &config);
        let instances = Instances::collect(&workspace, &config, true);
        assert_eq!(instances.named("react"), []);
        assert_eq!(instances.named("app"), []);
        assert_eq!(instances.named("lib").len(), 1);
    }

    #[test]
    fn edits_address_the_entry_and_never_the_own_version() {
        let config = config("");
        let workspace = workspace(&files(), &config);
        let instances = Instances::collect(&workspace, &config, true);
        let dependency = instances.get(instances.named("react")[1]);
        assert_eq!(
            dependency.pointer(),
            Some(vec![
                PathSeg::Key("peerDependencies".to_owned()),
                PathSeg::Key("react".to_owned())
            ])
        );
        assert!(dependency.set_specifier("^18.3.1").is_some() && dependency.remove().is_some());
        let own = instances.get(instances.named("app")[0]);
        assert!(own.pointer().is_none() && own.set_specifier("2.0.0").is_none() && own.remove().is_none());
    }

    #[test]
    fn locations_cover_the_entry_and_the_version_member() {
        let config = config("");
        let workspace = workspace(&files(), &config);
        let instances = Instances::collect(&workspace, &config, true);
        let slice = |location: &Location| {
            let span = location.span.unwrap();
            APP[span.start as usize..span.end as usize].to_owned()
        };
        let dependency = instances.get(instances.named("react")[0]);
        assert_eq!(slice(&dependency.location()), r#""react":"^18.0.0""#);
        let own = instances.get(instances.named("app")[0]);
        assert_eq!(slice(&own.location()), r#""version":"1.0.0""#);
    }
}
