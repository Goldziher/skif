use self::semversion::SemVersion;
use crate::{
    args::{Args, AutofixSelect},
    rules::{
        BoxIssue,
        empty_dependencies::{DependencyKind, EmptyDependenciesIssue},
        member_toolchain_fields::MemberToolchainFieldsIssue,
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

/// How member packages may declare `packageManager` and `engines`.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum MemberToolchain {
    /// Member packages must not declare them at all.
    #[default]
    Forbid,
    /// Member packages may declare them only when equal to the root's values.
    MatchRoot,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub fix: bool,
    pub select: Option<AutofixSelect>,
    pub no_install: bool,
    pub fail_on_warnings: bool,
    pub ignore_dependency: Vec<String>,
    pub ignore_package: Vec<String>,
    pub ignore_rule: Vec<String>,
    pub ignore_rule_for_package: IndexMap<String, Vec<String>>,
    pub member_toolchain: Option<MemberToolchain>,
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

        if !args.ignore_dependency.is_empty() {
            self.ignore_dependency.extend(args.ignore_dependency);
        }

        if !args.ignore_package.is_empty() {
            self.ignore_package.extend(args.ignore_package);
        }

        if !args.ignore_rule.is_empty() {
            self.ignore_rule.extend(args.ignore_rule);
        }

        for entry in args.ignore_rule_for_package {
            if let Some((rule, package)) = entry.split_once(':') {
                self.ignore_rule_for_package
                    .entry(rule.to_string())
                    .or_default()
                    .push(package.to_string());
            }
        }
    }

    /// Layers `other` over `self`: flags and scalars set in `other` win, lists are appended.
    pub fn overlay(&mut self, other: Config) {
        self.fix |= other.fix;
        self.no_install |= other.no_install;
        self.fail_on_warnings |= other.fail_on_warnings;

        if other.select.is_some() {
            self.select = other.select;
        }

        if other.member_toolchain.is_some() {
            self.member_toolchain = other.member_toolchain;
        }

        self.ignore_dependency.extend(other.ignore_dependency);
        self.ignore_package.extend(other.ignore_package);
        self.ignore_rule.extend(other.ignore_rule);

        for (rule, packages) in other.ignore_rule_for_package {
            self.ignore_rule_for_package
                .entry(rule)
                .or_default()
                .extend(packages);
        }
    }

    /// Whether issues of `rule` are ignored for `package` via `ignoreRuleForPackage`.
    pub fn ignores_rule_for(&self, rule: &str, package: &Package) -> bool {
        self.ignore_rule_for_package
            .get(rule)
            .is_some_and(|packages| package.is_ignored(packages))
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
    engines: Option<serde_json::Value>,
    dev_engines: Option<DevEngines>,
    dependencies: Option<IndexMap<String, String>>,
    dev_dependencies: Option<IndexMap<String, String>>,
    peer_dependencies: Option<IndexMap<String, String>>,
    optional_dependencies: Option<IndexMap<String, String>>,
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

    pub fn check_member_toolchain(
        &self,
        root: &Package,
        member_toolchain: MemberToolchain,
    ) -> Option<BoxIssue> {
        let forbid = member_toolchain == MemberToolchain::Forbid;
        let mut fields = Vec::new();

        if let Some(package_manager) = &self.inner.package_manager
            && (forbid || root.inner.package_manager.as_ref() != Some(package_manager))
        {
            fields.push("packageManager");
        }

        if let Some(engines) = &self.inner.engines
            && (forbid || root.inner.engines.as_ref() != Some(engines))
        {
            fields.push("engines");
        }

        if fields.is_empty() {
            None
        } else {
            Some(MemberToolchainFieldsIssue::new(fields))
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

    fn get_deps(deps: Option<&IndexMap<String, String>>) -> Option<IndexMap<String, SemVersion>> {
        if let Some(dependencies) = deps {
            let mut versioned_dependencies =
                IndexMap::<String, SemVersion>::with_capacity(dependencies.len());

            for (name, version) in dependencies {
                if let Ok(version) = SemVersion::parse(version) {
                    versioned_dependencies.insert(name.clone(), version);
                }
            }

            return Some(versioned_dependencies);
        }

        None
    }

    pub fn get_dependencies(&self) -> Option<IndexMap<String, SemVersion>> {
        Self::get_deps(self.inner.dependencies.as_ref())
    }

    pub fn get_dev_dependencies(&self) -> Option<IndexMap<String, SemVersion>> {
        Self::get_deps(self.inner.dev_dependencies.as_ref())
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
