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

use rayon::prelude::*;
use skif_config::{CustomType, DependencyType, OverrideReading};
use skif_core::{DependencyKind, FixOp, JsonAction, Location, PathSeg};
use skif_specifier::Specifier;
use skif_workspace::{DependencyEntry, Package, Workspace};

use super::catalog;
use super::fields::{self, Field};
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
#[derive(Debug, Clone)]
pub(crate) enum Origin<'w> {
    /// An entry of a dependency section of `package.json`.
    Dependency(&'w DependencyEntry),
    /// The `version` field of a workspace package.
    OwnVersion,
    /// A version in an override section or a custom type, in `package.json` or `pnpm-workspace.yaml`. Boxed: such
    /// versions are few, and every instance of a large workspace would otherwise carry room for one.
    Field(Box<Field>),
}

/// One declared version of a dependency.
#[derive(Debug, Clone)]
pub(crate) struct Instance<'w> {
    pub(crate) package: &'w Package,
    pub(crate) origin: Origin<'w>,
    /// Where the version is declared: a dependency section, the package's own version, an override section or a
    /// custom type.
    pub(crate) ty: DependencyType,
    /// The dependency name; for an own version, the package name.
    pub(crate) name: String,
    pub(crate) specifier: Arc<Specifier>,
    /// Whether `name` is itself a package of the workspace (what `$LOCAL` selects).
    pub(crate) is_local: bool,
    /// Whether the catalog rules report this instance against its catalog, so that the catch-all group does not take
    /// it. A configured version group whose selectors match it still governs it.
    pub(crate) catalog_owned: bool,
}

impl<'w> Instance<'w> {
    /// The instance of a dependency section entry.
    pub(crate) fn of_entry(
        package: &'w Package,
        entry: &'w DependencyEntry,
        is_local: bool,
        catalog_owned: bool,
    ) -> Self {
        Self {
            package,
            origin: Origin::Dependency(entry),
            ty: DependencyType::Section(entry.kind),
            name: entry.name.clone(),
            specifier: Arc::clone(&entry.parsed),
            is_local,
            catalog_owned,
        }
    }
}

impl<'w> Instance<'w> {
    /// The instance of a version in an override section or a custom type.
    pub(crate) fn of_field(package: &'w Package, found: fields::Found, is_local: bool) -> Self {
        Self {
            package,
            origin: Origin::Field(Box::new(found.field)),
            ty: found.ty,
            is_local,
            specifier: Specifier::new(&found.version),
            name: found.name,
            catalog_owned: false,
        }
    }
}

impl Instance<'_> {
    /// The dependency section, or `None` for the package's own version and for the versions of override sections and
    /// custom types.
    pub(crate) fn kind(&self) -> Option<DependencyKind> {
        match &self.origin {
            Origin::Dependency(entry) => Some(entry.kind),
            Origin::OwnVersion | Origin::Field(_) => None,
        }
    }

    pub(crate) const fn ty(&self) -> &DependencyType {
        &self.ty
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

    /// The whole entry for a dependency or a field, the `version` member for an own version.
    pub(crate) fn location(&self) -> Location {
        match &self.origin {
            Origin::Dependency(entry) => support::entry_location(self.package, entry),
            Origin::OwnVersion => support::field_location(self.package, &[support::key("version")]),
            Origin::Field(field) => field.location(),
        }
    }

    /// The JSON pointer of the entry in the manifest; `None` for an own version and for a version in a YAML file.
    pub(crate) fn pointer(&self) -> Option<Vec<PathSeg>> {
        match &self.origin {
            Origin::Dependency(entry) => Some(vec![support::key(entry.kind.field()), support::key(&entry.name)]),
            Origin::OwnVersion => None,
            Origin::Field(field) => field.json_pointer(),
        }
    }

    /// The operation writing `specifier` over this entry; `None` for an own version, which is never rewritten.
    pub(crate) fn set_specifier(&self, specifier: &str) -> Option<FixOp> {
        if let Origin::Field(field) = &self.origin {
            return Some(field.set(self.package, specifier));
        }
        let pointer = self.pointer()?;
        Some(support::json_op(
            self.package,
            pointer,
            JsonAction::Set(serde_json::Value::String(specifier.to_owned())),
        ))
    }

    /// The operation removing this entry; `None` for an own version.
    pub(crate) fn remove(&self) -> Option<FixOp> {
        if let Origin::Field(field) = &self.origin {
            return Some(field.remove(self.package));
        }
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
    /// [`Groups::assign`](super::groups::Groups::assign)). Versions in the override sections and in the fields `custom`
    /// names are collected too. Dependencies named in `owned` (see
    /// [`catalog::owned_dependencies`]) are flagged [`catalog_owned`](Instance::catalog_owned): the catalog rules
    /// report them.
    pub(crate) fn collect_with(
        workspace: &'w Workspace,
        owned: &BTreeSet<&str>,
        custom: &[CustomType],
        reading: OverrideReading,
    ) -> Self {
        let local_names: HashSet<&str> = workspace
            .all_packages()
            .filter_map(|package| package.name.as_deref())
            .collect();
        let of_package = |package: &'w Package| {
            let mut items = Vec::with_capacity(package.dependencies.len() + 1);
            if let (Some(name), Some(version)) = (&package.name, &package.version) {
                items.push(Instance {
                    package,
                    origin: Origin::OwnVersion,
                    ty: DependencyType::Local,
                    name: name.clone(),
                    specifier: Specifier::new(version),
                    is_local: true,
                    catalog_owned: false,
                });
            }
            for entry in &package.dependencies {
                items.push(Instance::of_entry(
                    package,
                    entry,
                    local_names.contains(entry.name.as_str()),
                    owned.contains(entry.name.as_str()) && owned_by_catalog(entry),
                ));
            }
            for found in fields::collect(workspace, package, custom, reading) {
                let is_local = local_names.contains(found.name.as_str());
                items.push(Instance::of_field(package, found, is_local));
            }
            items
        };
        // Packages are read in parallel, and their instances kept in package order.
        let packages: Vec<&'w Package> = support::active_packages(workspace).collect();
        let per_package: Vec<Vec<Instance<'w>>> = packages.into_par_iter().map(of_package).collect();
        let mut items = Vec::with_capacity(per_package.iter().map(Vec::len).sum());
        for instances in per_package {
            items.extend(instances);
        }
        let mut by_name: BTreeMap<String, Vec<InstanceId>> = BTreeMap::new();
        for (index, item) in items.iter().enumerate() {
            // The name is copied once per dependency, not once per instance.
            if let Some(ids) = by_name.get_mut(item.name.as_str()) {
                ids.push(InstanceId(index));
            } else {
                by_name.insert(item.name.clone(), vec![InstanceId(index)]);
            }
        }
        Self {
            workspace,
            items,
            by_name,
        }
    }

    /// [`collect_with`](Self::collect_with) for the dependencies the catalog rules own under `config`, or none.
    #[cfg(test)]
    pub(crate) fn collect(workspace: &'w Workspace, config: &skif_config::Config, catalog_owns: bool) -> Self {
        let usages = catalog::UsageIndex::build(workspace, config);
        let owned = if catalog_owns {
            catalog::owned_dependencies(&usages, workspace, &config.catalog)
        } else {
            BTreeSet::new()
        };
        Self::collect_with(workspace, &owned, &config.custom_types, config.policy.override_reading)
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
    use skif_core::YamlAction;

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

    const WITH_FIELDS: &str = r#"{"name":"root","version":"0.0.0","private":true,"packageManager":"pnpm@9.0.0","workspaces":["packages/*"],"resolutions":{"**/lodash":"4.17.20"},"overrides":{"react":{".":"18.0.0","react-dom":"18.2.0"},"left":"1.0.0","nested":{"a":1}},"pnpm":{"overrides":{"react@<18":"18.0.0"}}}"#;
    const MANAGER: &str =
        "[[custom-types]]\nname = \"manager\"\npath = \"packageManager\"\nstrategy = \"name@version\"\n";

    fn field_instances<'a, 'w>(instances: &'a Instances<'w>) -> Vec<&'a Instance<'w>> {
        instances
            .iter()
            .map(|(_, instance)| instance)
            .filter(|instance| !instance.is_own_version() && instance.kind().is_none())
            .collect()
    }

    #[test]
    fn override_sections_and_custom_types_become_instances() {
        let config = config(MANAGER);
        let files = [("package.json", WITH_FIELDS), ("packages/app/package.json", APP)];
        let workspace = workspace(&files, &config);
        let instances = Instances::collect(&workspace, &config, true);
        let found: Vec<(String, String, String)> = field_instances(&instances)
            .into_iter()
            .map(|instance| {
                (
                    instance.ty().token().to_owned(),
                    instance.name.clone(),
                    instance.raw().to_owned(),
                )
            })
            .collect();
        let expected = [
            ("resolutions", "lodash", "4.17.20"),
            ("overrides", "react", "18.0.0"),
            ("overrides", "react-dom", "18.2.0"),
            ("overrides", "left", "1.0.0"),
            ("pnpm-overrides", "react", "18.0.0"),
            ("manager", "pnpm", "9.0.0"),
        ];
        assert_eq!(
            found,
            expected.map(|(ty, name, raw)| (ty.to_owned(), name.to_owned(), raw.to_owned()))
        );
        assert_eq!(
            instances.named("react").len(),
            4,
            "two sections of app and two field instances"
        );
    }

    #[test]
    fn syncpack_reading_takes_the_key_as_the_name_and_skips_what_syncpack_does_not_read() {
        let yaml = "packages:\n  - \"packages/*\"\noverrides:\n  \"parent@4>child\": ^6.5.0\n  hono: ^4.13.7\n";
        let files = [
            ("package.json", WITH_FIELDS),
            ("pnpm-workspace.yaml", yaml),
            ("packages/app/package.json", APP),
        ];
        let found = |text: &str| {
            let config = config(text);
            let workspace = workspace(&files, &config);
            let instances = Instances::collect(&workspace, &config, true);
            field_instances(&instances)
                .into_iter()
                .map(|instance| (instance.ty().token().to_owned(), instance.name.clone()))
                .collect::<Vec<_>>()
        };
        let pair = |ty: &str, name: &str| (ty.to_owned(), name.to_owned());
        assert_eq!(
            found("[policy]\noverride-reading = \"syncpack\"\n"),
            [
                pair("resolutions", "**/lodash"),
                pair("overrides", "left"),
                pair("pnpm-overrides", "parent@4>child"),
                pair("pnpm-overrides", "hono"),
            ],
            "nested objects and `pnpm.overrides` of a manifest are not read, and a selector is its own name"
        );
        let skif = found("");
        assert!(skif.contains(&pair("pnpm-overrides", "child")), "{skif:?}");
        assert!(skif.contains(&pair("pnpm-overrides", "react")), "{skif:?}");
    }

    #[test]
    fn field_edits_address_the_value_and_keep_the_name_prefix() {
        let config = config(MANAGER);
        let files = [("package.json", WITH_FIELDS), ("packages/app/package.json", APP)];
        let workspace = workspace(&files, &config);
        let instances = Instances::collect(&workspace, &config, true);
        let field = |token: &str, name: &str| {
            *field_instances(&instances)
                .iter()
                .find(|instance| instance.ty().token() == token && instance.name == name)
                .unwrap()
        };
        let set = |instance: &Instance<'_>, pointer: Vec<&str>, value: &str| {
            Some(support::json_op(
                instance.package,
                pointer.into_iter().map(support::key).collect(),
                JsonAction::Set(serde_json::Value::String(value.to_owned())),
            ))
        };
        let nested = field("overrides", "react");
        assert_eq!(
            nested.set_specifier("^18.3.1"),
            set(nested, vec!["overrides", "react", "."], "^18.3.1")
        );
        let pnpm = field("pnpm-overrides", "react");
        assert_eq!(
            pnpm.set_specifier("^18.3.1"),
            set(pnpm, vec!["pnpm", "overrides", "react@<18"], "^18.3.1")
        );
        let manager = field("manager", "pnpm");
        assert_eq!(
            manager.set_specifier("9.1.0"),
            set(manager, vec!["packageManager"], "pnpm@9.1.0")
        );
        assert_eq!(
            manager.remove(),
            Some(support::json_op(
                manager.package,
                vec![support::key("packageManager")],
                JsonAction::Remove
            ))
        );
        let slice = |instance: &Instance<'_>| {
            let span = instance.location().span.unwrap();
            WITH_FIELDS[span.start as usize..span.end as usize].to_owned()
        };
        assert_eq!(slice(pnpm), r#""react@<18":"18.0.0""#);
        assert_eq!(slice(field("resolutions", "lodash")), r#""**/lodash":"4.17.20""#);
        assert!(
            slice(nested).starts_with(r#""react":{"#),
            "a `.` entry is located at its parent"
        );
    }

    #[test]
    fn overrides_of_pnpm_workspace_are_edited_in_the_yaml_file() {
        let config = config("");
        let yaml = "packages:\n  - \"packages/*\"\noverrides:\n  lodash: 4.17.20\n";
        let files = [
            ("package.json", ROOT),
            ("pnpm-workspace.yaml", yaml),
            ("packages/app/package.json", APP),
        ];
        let workspace = workspace(&files, &config);
        let instances = Instances::collect(&workspace, &config, true);
        let lodash = field_instances(&instances)
            .into_iter()
            .find(|instance| instance.name == "lodash")
            .unwrap();
        assert_eq!(lodash.ty(), &DependencyType::PnpmOverrides);
        assert_eq!(lodash.pointer(), None);
        assert_eq!(
            lodash.set_specifier("^4.17.21"),
            Some(FixOp::yaml(
                "pnpm-workspace.yaml",
                vec![support::key("overrides"), support::key("lodash")],
                YamlAction::Set(serde_json::Value::String("^4.17.21".to_owned()))
            ))
        );
        let location = lodash.location();
        let span = location.span.unwrap();
        assert_eq!(&yaml[span.start as usize..span.end as usize], "lodash: 4.17.20");
    }
}
