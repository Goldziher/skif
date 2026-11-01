use crate::packages::root::RootPackage;
use crate::packages::semversion::SemVersion;
use crate::packages::{Config, Package, PackagesList};
use crate::printer::print_error;
use crate::rules::multiple_dependency_versions::MultipleDependencyVersionsIssue;
use crate::rules::non_existant_packages::NonExistantPackagesIssue;
use crate::rules::packages_without_package_json::PackagesWithoutPackageJsonIssue;
use crate::rules::types_in_dependencies::TypesInDependenciesIssue;
use crate::rules::unsync_similar_dependencies::{
    SimilarDependency, UnsyncSimilarDependenciesIssue,
};
use crate::rules::{BoxIssue, IssuesList, PackageType};
use anyhow::{Result, anyhow};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self};
use std::path::{Path, PathBuf};

const PNPM_WORKSPACE: &str = "pnpm-workspace.yaml";

#[derive(Debug, Serialize, Deserialize)]
pub struct PnpmWorkspace {
    pub packages: Vec<String>,
}

pub fn collect_packages(root: &Path) -> Result<PackagesList> {
    let root_package = RootPackage::new(root)?;
    let mut packages = Vec::new();
    let mut packages_list = root_package.get_workspaces();
    let mut excluded_paths = Vec::new();
    let mut non_existant_paths = Vec::new();
    let mut is_pnpm_workspace = false;

    if packages_list.is_none() {
        let pnpm_workspace = root.join(PNPM_WORKSPACE);

        if !pnpm_workspace.is_file() {
            return Err(anyhow!(
                "No `workspaces` field in the root `package.json`, or `pnpm-workspace.yaml` file not found in {root:?}"
            ));
        }

        let root_package = fs::read_to_string(pnpm_workspace)?;
        let workspace: PnpmWorkspace = serde_saphyr::from_str(&root_package)?;

        packages_list = Some(workspace.packages);
        is_pnpm_workspace = true;
    }

    let mut packages_issues: Vec<BoxIssue> = Vec::new();

    let mut add_package = |packages_issues: &mut Vec<BoxIssue>, path: PathBuf| {
        // Ignore hidden directories, e.g. `.npm`, `.react-email`
        if let Some(stem) = path.file_stem()
            && let Some(stem) = stem.to_str()
            && stem.starts_with('.')
        {
            return;
        }

        // Ignore empty directories, which can linger after switching branches in git, for example.
        if fs::read_dir(&path).is_ok_and(|mut dir| dir.next().is_none()) {
            return;
        }

        match Package::new(path.clone()) {
            Ok(package) => packages.push(package),
            Err(error) => {
                if error.to_string().contains("not found") {
                    packages_issues.push(PackagesWithoutPackageJsonIssue::new(
                        path.to_string_lossy().to_string(),
                    ));
                } else {
                    print_error("Failed to collect package", &error.to_string());
                    std::process::exit(1);
                }
            }
        }
    };

    if let Some(packages) = &packages_list {
        let packages = packages
            .iter()
            .filter(|package| {
                if package.starts_with('!') {
                    if package.ends_with('*') {
                        let directory = package
                            .trim_start_matches('!')
                            .trim_end_matches('*')
                            .trim_end_matches('/');
                        let directory = root.join(directory);

                        excluded_paths.push(directory.to_string_lossy().to_string());
                    } else {
                        let directory = package.trim_start_matches('!');
                        let directory = root.join(directory);

                        excluded_paths.push(directory.to_string_lossy().to_string());
                    }

                    return false;
                }

                true
            })
            .collect::<Vec<_>>();

        let mut expanded_packages = Vec::new();

        for package in packages {
            if let Some((directory, subdirectory)) = package.split_once("/*/") {
                let directory = root.join(directory);

                if let Ok(expanded_folders) = directory.read_dir() {
                    for expanded_folder in expanded_folders.flatten() {
                        let expanded_folder = expanded_folder.path();

                        if expanded_folder.is_dir() {
                            let path = expanded_folder
                                .to_string_lossy()
                                .to_string()
                                .replace(&(root.to_string_lossy().to_string() + "/"), "")
                                + "/"
                                + subdirectory;

                            expanded_packages.push(path);
                        }
                    }
                } else {
                    non_existant_paths.push(package.clone());
                }
            } else if let Some((directory, subdirectory)) = package.split_once("/**/") {
                let directory = root.join(directory);

                if let Ok(expanded_folders) = directory.read_dir() {
                    for expanded_folder in expanded_folders.flatten() {
                        let expanded_folder = expanded_folder.path();

                        if expanded_folder.is_dir() {
                            let path = expanded_folder
                                .to_string_lossy()
                                .to_string()
                                .replace(&(root.to_string_lossy().to_string() + "/"), "")
                                + "/"
                                + subdirectory;

                            expanded_packages.push(path);
                        }
                    }
                } else {
                    non_existant_paths.push(package.clone());
                }
            } else {
                expanded_packages.push(package.clone());
            }
        }

        for package in &expanded_packages {
            if package.ends_with('*') {
                let directory_match = package.trim_end_matches('*');

                let packages = if directory_match.ends_with('/') {
                    let directory = directory_match.trim_end_matches('/');
                    let directory = root.join(directory);

                    if let Ok(packages) = directory.read_dir() {
                        packages.into_iter().collect::<Result<Vec<_>, _>>()?
                    } else {
                        non_existant_paths.push(package.clone());
                        continue;
                    }
                } else {
                    let directory = root.join(directory_match);
                    let directory = directory.parent().unwrap().to_path_buf();

                    if let Ok(packages) = directory.read_dir() {
                        packages
                            .into_iter()
                            .filter(|package| {
                                if let Ok(package) = package {
                                    return package.file_type().unwrap().is_dir()
                                        && package
                                            .file_name()
                                            .to_string_lossy()
                                            .starts_with(directory_match);
                                }

                                true
                            })
                            .collect::<Result<Vec<_>, _>>()?
                    } else {
                        non_existant_paths.push(package.clone());
                        continue;
                    }
                };

                for package in packages {
                    if package.file_type()?.is_dir() {
                        let path = package.path();
                        let real_path = path.to_string_lossy().to_string();
                        let mut is_excluded = false;

                        for excluded_path in &excluded_paths {
                            if real_path.starts_with(excluded_path)
                                && !real_path.replace(excluded_path, "").contains('/')
                            {
                                is_excluded = true;
                                break;
                            }
                        }

                        if !is_excluded {
                            add_package(&mut packages_issues, path);
                        }
                    }
                }
            } else {
                let path = root.join(package);

                if path.is_dir() {
                    add_package(&mut packages_issues, path);
                } else {
                    non_existant_paths.push(package.clone());
                }
            }
        }

        if !non_existant_paths.is_empty() {
            packages_issues.push(NonExistantPackagesIssue::new(
                is_pnpm_workspace,
                packages_list.unwrap(),
                non_existant_paths,
            ));
        }
    }

    Ok(PackagesList {
        root_package,
        packages,
        packages_issues,
    })
}

fn add_for_package(
    issues: &mut IssuesList,
    config: &Config,
    package: &Package,
    package_type: PackageType,
    issue: Option<BoxIssue>,
) {
    if let Some(issue) = issue
        && !config.ignores_rule_for(issue.name(), package)
    {
        issues.add_raw(package_type, issue);
    }
}

pub fn collect_issues(config: &Config, packages_list: PackagesList) -> IssuesList<'_> {
    let mut issues = IssuesList::new(&config.ignore_rule);

    let PackagesList {
        root_package,
        packages,
        packages_issues,
    } = packages_list;

    for package_issue in packages_issues {
        issues.add_raw(PackageType::None, package_issue);
    }

    issues.add(PackageType::Root, root_package.check_private());
    issues.add(PackageType::Root, root_package.check_package_manager());
    issues.add(PackageType::Root, root_package.check_dependencies());
    issues.add(PackageType::Root, root_package.check_dev_dependencies());
    issues.add(PackageType::Root, root_package.check_peer_dependencies());
    issues.add(
        PackageType::Root,
        root_package.check_optional_dependencies(),
    );

    let mut all_dependencies = IndexMap::new();
    let mut joined_dependencies = IndexMap::new();
    let mut similar_dependencies_by_package = IndexMap::new();
    // Paths of the packages excluded from the dependency-wide rules via `ignoreRuleForPackage`.
    let mut multiple_versions_ignored_paths = HashSet::new();
    let mut unsync_similar_ignored_paths = HashSet::new();

    if config.ignores_rule_for("multiple-dependency-versions", root_package.as_package()) {
        multiple_versions_ignored_paths.insert(root_package.get_path());
    }

    if config.ignores_rule_for("unsync-similar-dependencies", root_package.as_package()) {
        unsync_similar_ignored_paths.insert(root_package.get_path());
    }

    if let Some(dependencies) = root_package.get_dependencies() {
        joined_dependencies.extend(dependencies);
    }

    if let Some(dev_dependencies) = root_package.get_dev_dependencies() {
        joined_dependencies.extend(dev_dependencies);
    }

    for (name, version) in joined_dependencies {
        if version.is_valid() {
            all_dependencies
                .entry(name)
                .or_insert_with(IndexMap::new)
                .insert(root_package.get_path(), version);
        }
    }

    for package in packages {
        if package.is_ignored(&config.ignore_package) {
            continue;
        }

        let package_type = PackageType::Package(package.get_path());

        if config.ignores_rule_for("multiple-dependency-versions", &package) {
            multiple_versions_ignored_paths.insert(package.get_path());
        }

        if config.ignores_rule_for("unsync-similar-dependencies", &package) {
            unsync_similar_ignored_paths.insert(package.get_path());
        }

        for issue in [
            package.check_dependencies(),
            package.check_dev_dependencies(),
            package.check_peer_dependencies(),
            package.check_optional_dependencies(),
            package.check_member_toolchain(
                root_package.as_package(),
                config.member_toolchain.unwrap_or_default(),
            ),
        ] {
            add_for_package(&mut issues, config, &package, package_type.clone(), issue);
        }

        let mut joined_dependencies = IndexMap::new();

        if let Some(dependencies) = package.get_dependencies() {
            if package.is_private() {
                let types_in_dependencies = dependencies
                    .iter()
                    .filter(|(name, _)| name.starts_with("@types/"))
                    .map(|(name, _)| name.clone())
                    .collect::<Vec<_>>();

                if !types_in_dependencies.is_empty() {
                    add_for_package(
                        &mut issues,
                        config,
                        &package,
                        package_type.clone(),
                        Some(TypesInDependenciesIssue::new(types_in_dependencies)),
                    );
                }
            }

            joined_dependencies.extend(dependencies);
        }

        if let Some(dev_dependencies) = package.get_dev_dependencies() {
            joined_dependencies.extend(dev_dependencies);
        }

        for (name, version) in joined_dependencies {
            if version.is_valid() {
                all_dependencies
                    .entry(name)
                    .or_insert_with(IndexMap::new)
                    .insert(package.get_path(), version);
            }
        }
    }

    for (name, versions) in all_dependencies {
        if let Ok(similar_dependency) = SimilarDependency::try_from(name.as_str()) {
            for (path, version) in &versions {
                if unsync_similar_ignored_paths.contains(path) {
                    continue;
                }

                similar_dependencies_by_package
                    .entry(path.clone())
                    .or_insert_with(
                        IndexMap::<SimilarDependency, IndexMap<SemVersion, String>>::new,
                    )
                    .entry(similar_dependency.clone())
                    .or_insert_with(IndexMap::new)
                    .insert(version.clone(), name.clone());
            }
        }

        let mut filtered_versions = versions
            .iter()
            .filter(|(path, _)| !multiple_versions_ignored_paths.contains(path.as_str()))
            .filter(|(_, version)| {
                !config
                    .ignore_dependency
                    .contains(&format!("{name}@{version}"))
            })
            .map(|(path, version)| (path.clone(), version.clone()))
            .collect::<IndexMap<_, _>>();

        if filtered_versions.len() > 1
            && !filtered_versions
                .values()
                .collect::<Vec<_>>()
                .windows(2)
                .all(|window| window[0] == window[1])
            && !config.ignore_dependency.contains(&name)
            && !config.ignore_dependency.iter().any(|dependency| {
                if dependency.ends_with('*') {
                    if dependency.starts_with('*') {
                        return name
                            .contains(dependency.trim_start_matches('*').trim_end_matches('*'));
                    }
                    return name.starts_with(dependency.trim_end_matches('*'));
                } else if dependency.starts_with('*') {
                    return name.ends_with(dependency.trim_start_matches('*'));
                }
                false
            })
        {
            filtered_versions.sort_keys();

            issues.add_raw(
                PackageType::None,
                MultipleDependencyVersionsIssue::new(
                    name,
                    filtered_versions,
                    config.select.clone(),
                ),
            );
        }
    }

    for (path, similar_dependencies) in similar_dependencies_by_package {
        for (similar_dependency, versions) in similar_dependencies {
            if versions.len() > 1 {
                issues.add_raw(
                    PackageType::Package(path.clone()),
                    UnsyncSimilarDependenciesIssue::new(similar_dependency, versions),
                );
            }
        }
    }

    issues
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::args::Args;
    use debugless_unwrap::DebuglessUnwrapErrExt;

    #[test]
    fn collect_packages_unknown_dir() {
        let root = Path::new("unknown");
        let result = collect_packages(root);

        assert!(result.is_err());
        assert_eq!(
            result.debugless_unwrap_err().to_string(),
            "Path \"unknown\" is not a directory"
        );
    }

    #[test]
    fn collect_packages_empty_dir() {
        let root = Path::new("fixtures/empty");
        let result = collect_packages(root);

        assert!(result.is_err());
        assert_eq!(
            result.debugless_unwrap_err().to_string(),
            "`package.json` not found in \"fixtures/empty\""
        );
    }

    #[test]
    fn collect_packages_basic() {
        let root = Path::new("fixtures/basic");
        let result = collect_packages(root);

        assert!(result.is_ok());
        let PackagesList {
            root_package,
            packages,
            packages_issues,
        } = result.unwrap();

        assert_eq!(root_package.get_name(), "basic");
        assert_eq!(packages.len(), 3);
        assert_eq!(packages_issues.len(), 1);
        assert_eq!(packages_issues[0].name(), "non-existant-packages");
    }

    #[test]
    fn collect_packages_pnpm() {
        let root = Path::new("fixtures/pnpm");
        let result = collect_packages(root);

        assert!(result.is_ok());
        let PackagesList {
            root_package,
            packages,
            packages_issues,
        } = result.unwrap();

        assert_eq!(root_package.get_name(), "pnpm");
        assert_eq!(packages.len(), 3);
        assert_eq!(packages_issues.len(), 1);
        assert_eq!(packages_issues[0].name(), "non-existant-packages");
    }

    #[test]
    fn collect_packages_yarn_nohoist() {
        let root = Path::new("fixtures/yarn-nohoist");
        let result = collect_packages(root);

        assert!(result.is_ok());
        let PackagesList {
            root_package,
            packages,
            packages_issues,
        } = result.unwrap();

        assert_eq!(root_package.get_name(), "yarn-nohoist");
        assert_eq!(packages.len(), 3);
        assert_eq!(packages_issues.len(), 1);
        assert_eq!(packages_issues[0].name(), "non-existant-packages");
    }

    #[test]
    fn collect_packages_no_workspace_pnpm() {
        let root = Path::new("fixtures/no-workspace-pnpm");
        let result = collect_packages(root);

        assert!(result.is_err());
        assert_eq!(
            result.debugless_unwrap_err().to_string(),
            "No `workspaces` field in the root `package.json`, or `pnpm-workspace.yaml` file not found in \"fixtures/no-workspace-pnpm\""
        );
    }

    #[test]
    fn collect_packages_without_package_json() {
        let root = Path::new("fixtures/without-package-json");
        let result = collect_packages(root);

        assert!(result.is_ok());
        let PackagesList {
            root_package,
            packages,
            packages_issues,
        } = result.unwrap();

        assert_eq!(root_package.get_name(), "without-package-json");
        assert_eq!(packages.len(), 1);
        assert_eq!(packages_issues.len(), 2);
        assert_eq!(packages_issues[0].name(), "packages-without-package-json");
        assert_eq!(packages_issues[1].name(), "packages-without-package-json");
    }

    #[test]
    fn collect_packages_skips_empty_directories() {
        let root = Path::new("fixtures/empty-package-directories");
        // Git doesn't track empty directories, so create one at test time to
        // emulate a stale directory left over after switching branches.
        let empty_dir = root.join("packages/empty");
        fs::create_dir_all(&empty_dir).unwrap();

        let result = collect_packages(root);

        let cleanup = fs::remove_dir_all(&empty_dir);

        assert!(result.is_ok());
        let PackagesList {
            root_package,
            packages,
            packages_issues,
        } = result.unwrap();

        assert_eq!(root_package.get_name(), "empty-package-directories");
        assert_eq!(packages.len(), 1);
        assert!(packages_issues.is_empty());

        cleanup.unwrap();
    }

    #[test]
    fn collect_packages_ignore_paths() {
        let root = Path::new("fixtures/ignore-paths");
        let result = collect_packages(root);

        assert!(result.is_ok());
        let PackagesList {
            root_package,
            packages,
            ..
        } = result.unwrap();

        assert_eq!(root_package.get_name(), "ignore-paths");
        assert_eq!(packages.len(), 4);

        let mut packages = packages
            .into_iter()
            .map(|package| package.get_name().cloned().unwrap())
            .collect::<Vec<_>>();
        packages.sort();

        assert_eq!(packages[0], "d");
        assert_eq!(packages[1], "docs");
        assert_eq!(packages[2], "e");
        assert_eq!(packages[3], "ghi");
    }

    #[test]
    fn collect_root_issues() {
        let args = Args {
            path: "fixtures/root-issues".into(),
            fix: false,
            select: None,
            no_install: true,
            fail_on_warnings: false,
            ignore_rule: Vec::new(),
            ignore_package: Vec::new(),
            ignore_dependency: Vec::new(),
            ignore_rule_for_package: Vec::new(),
        };

        let packages_list = collect_packages(&args.path).unwrap();
        let config = args.into();
        assert_eq!(packages_list.root_package.get_name(), "root-issues");

        let issues = collect_issues(&config, packages_list);
        assert_eq!(issues.total_len(), 4);

        let issues = issues.into_iter().collect::<IndexMap<_, _>>();
        assert_eq!(
            issues.get(&PackageType::Root).unwrap()[0].name(),
            "root-package-private-field"
        );
        assert_eq!(
            issues.get(&PackageType::Root).unwrap()[1].name(),
            "root-package-manager-field"
        );
        assert_eq!(
            issues.get(&PackageType::Root).unwrap()[2].name(),
            "root-package-dependencies"
        );
        assert_eq!(
            issues.get(&PackageType::Root).unwrap()[3].name(),
            "empty-dependencies"
        );
    }

    #[test]
    fn collect_root_issues_fixed() {
        let args = Args {
            fix: false,
            select: None,
            no_install: true,
            fail_on_warnings: false,
            path: "fixtures/root-issues-fixed".into(),
            ignore_rule: Vec::new(),
            ignore_package: Vec::new(),
            ignore_dependency: Vec::new(),
            ignore_rule_for_package: Vec::new(),
        };

        let packages_list = collect_packages(&args.path).unwrap();
        assert_eq!(packages_list.root_package.get_name(), "root-issues-fixed");

        let config = args.into();
        let issues = collect_issues(&config, packages_list);
        assert_eq!(issues.total_len(), 0);
    }

    #[test]
    fn collect_root_dev_engines() {
        let args = Args {
            path: "fixtures/dev-engines".into(),
            fix: false,
            select: None,
            no_install: true,
            fail_on_warnings: false,
            ignore_rule: Vec::new(),
            ignore_package: Vec::new(),
            ignore_dependency: Vec::new(),
            ignore_rule_for_package: Vec::new(),
        };

        let packages_list = collect_packages(&args.path).unwrap();
        assert_eq!(packages_list.root_package.get_name(), "dev-engines");

        let config = args.into();
        let issues = collect_issues(&config, packages_list);
        assert_eq!(issues.total_len(), 0);
    }

    #[test]
    fn collect_root_dev_engines_array() {
        let args = Args {
            path: "fixtures/dev-engines-array".into(),
            fix: false,
            select: None,
            no_install: true,
            fail_on_warnings: false,
            ignore_rule: Vec::new(),
            ignore_package: Vec::new(),
            ignore_dependency: Vec::new(),
            ignore_rule_for_package: Vec::new(),
        };

        let packages_list = collect_packages(&args.path).unwrap();
        assert_eq!(packages_list.root_package.get_name(), "dev-engines-array");

        let config = args.into();
        let issues = collect_issues(&config, packages_list);
        assert_eq!(issues.total_len(), 0);
    }

    #[test]
    fn collect_root_dev_engines_without_package_manager() {
        let args = Args {
            path: "fixtures/dev-engines-no-package-manager".into(),
            fix: false,
            select: None,
            no_install: true,
            fail_on_warnings: false,
            ignore_rule: Vec::new(),
            ignore_package: Vec::new(),
            ignore_dependency: Vec::new(),
            ignore_rule_for_package: Vec::new(),
        };

        let packages_list = collect_packages(&args.path).unwrap();
        assert_eq!(
            packages_list.root_package.get_name(),
            "dev-engines-no-package-manager"
        );

        let config = args.into();
        let issues = collect_issues(&config, packages_list);
        assert_eq!(issues.total_len(), 1);

        let issues = issues.into_iter().collect::<IndexMap<_, _>>();
        assert_eq!(
            issues.get(&PackageType::Root).unwrap()[0].name(),
            "root-package-manager-field"
        );
    }

    #[test]
    fn collect_dependencies() {
        let args = Args {
            path: "fixtures/dependencies".into(),
            fix: false,
            select: None,
            no_install: true,
            fail_on_warnings: false,
            ignore_rule: Vec::new(),
            ignore_package: Vec::new(),
            ignore_dependency: Vec::new(),
            ignore_rule_for_package: Vec::new(),
        };

        let packages_list = collect_packages(&args.path).unwrap();
        assert_eq!(packages_list.root_package.get_name(), "dependencies");

        let config = args.into();
        let issues = collect_issues(&config, packages_list);
        assert_eq!(issues.total_len(), 4);

        let issues = issues.into_iter().collect::<IndexMap<_, _>>();

        assert_eq!(
            issues.get(&PackageType::None).unwrap()[0].name(),
            "multiple-dependency-versions"
        );
        assert_eq!(
            issues.get(&PackageType::None).unwrap()[1].name(),
            "multiple-dependency-versions"
        );
        assert_eq!(
            issues.get(&PackageType::None).unwrap()[2].name(),
            "multiple-dependency-versions"
        );
        assert_eq!(
            issues.get(&PackageType::None).unwrap()[3].name(),
            "multiple-dependency-versions"
        );
    }

    #[test]
    fn collect_dependencies_allow() {
        let args = Args {
            path: "fixtures/dependencies".into(),
            fix: false,
            select: None,
            no_install: false,
            fail_on_warnings: false,
            ignore_rule: Vec::new(),
            ignore_package: Vec::new(),
            ignore_dependency: vec!["next@4.5.6".to_string(), "*eslint*".to_string()],
        };

        let packages_list = collect_packages(&args.path).unwrap();
        assert_eq!(packages_list.root_package.get_name(), "dependencies");

        let config = args.into();
        let issues = collect_issues(&config, packages_list);
        assert_eq!(issues.total_len(), 1);

        let issues = issues.into_iter().collect::<IndexMap<_, _>>();

        assert_eq!(
            issues.get(&PackageType::None).unwrap()[0].name(),
            "multiple-dependency-versions"
        );
    }

    #[test]
    fn collect_dependencies_without_star() {
        let args = Args {
            path: "fixtures/dependencies-star".into(),
            fix: false,
            select: None,
            no_install: true,
            fail_on_warnings: false,
            ignore_rule: Vec::new(),
            ignore_package: Vec::new(),
            ignore_dependency: Vec::new(),
            ignore_rule_for_package: Vec::new(),
        };

        let packages_list = collect_packages(&args.path).unwrap();
        assert_eq!(packages_list.root_package.get_name(), "dependencies-star");

        let config = args.into();
        let issues = collect_issues(&config, packages_list);
        assert_eq!(issues.total_len(), 2);

        let issues = issues.into_iter().collect::<IndexMap<_, _>>();

        assert_eq!(
            issues.get(&PackageType::None).unwrap()[0].name(),
            "multiple-dependency-versions"
        );
        assert_eq!(
            issues.get(&PackageType::None).unwrap()[1].name(),
            "multiple-dependency-versions"
        );
    }

    #[test]
    fn collect_dependencies_nested_star() {
        let args = Args {
            path: "fixtures/dependencies-nested-star".into(),
            fix: false,
            select: None,
            no_install: false,
            fail_on_warnings: false,
            ignore_rule: Vec::new(),
            ignore_package: Vec::new(),
            ignore_dependency: Vec::new(),
            ignore_rule_for_package: Vec::new(),
        };

        let packages_list = collect_packages(&args.path).unwrap();
        assert_eq!(
            packages_list.root_package.get_name(),
            "dependencies-nested-star"
        );

        let config = args.into();
        let issues = collect_issues(&config, packages_list);
        assert_eq!(issues.total_len(), 2);

        let issues = issues.into_iter().collect::<IndexMap<_, _>>();

        assert_eq!(
            issues.get(&PackageType::None).unwrap()[0].name(),
            "multiple-dependency-versions"
        );
        assert_eq!(
            issues.get(&PackageType::None).unwrap()[1].name(),
            "multiple-dependency-versions"
        );
    }

    #[test]
    fn collect_pnpm_glob() {
        let args = Args {
            path: "fixtures/pnpm-glob".into(),
            fix: false,
            select: None,
            no_install: true,
            fail_on_warnings: false,
            ignore_rule: Vec::new(),
            ignore_package: Vec::new(),
            ignore_dependency: Vec::new(),
            ignore_rule_for_package: Vec::new(),
        };

        let packages_list = collect_packages(&args.path).unwrap();
        assert_eq!(packages_list.root_package.get_name(), "pnpm-glob");
        assert_eq!(packages_list.packages.len(), 2);

        let config = args.into();
        let issues = collect_issues(&config, packages_list);
        assert_eq!(issues.total_len(), 0);
    }

    #[test]
    fn collect_unordered_dependencies() {
        let args = Args {
            path: "fixtures/unordered".into(),
            fix: false,
            select: None,
            no_install: false,
            fail_on_warnings: false,
            ignore_rule: Vec::new(),
            ignore_package: Vec::new(),
            ignore_dependency: Vec::new(),
            ignore_rule_for_package: Vec::new(),
        };

        let packages_list = collect_packages(&args.path).unwrap();
        assert_eq!(packages_list.root_package.get_name(), "unordered");
        assert_eq!(packages_list.packages.len(), 1);

        let config = args.into();
        let issues = collect_issues(&config, packages_list);
        assert_eq!(issues.total_len(), 2);

        let issues = issues.into_iter().collect::<IndexMap<_, _>>();

        assert_eq!(
            issues.get(&PackageType::Root).unwrap()[0].name(),
            "unordered-dependencies"
        );
        assert_eq!(
            issues
                .get(&PackageType::Package("fixtures/unordered/docs".to_string()))
                .unwrap()[0]
                .name(),
            "unordered-dependencies"
        );
    }

    #[test]
    fn collect_unsync_similar_dependencies() {
        let args = Args {
            path: "fixtures/unsync".into(),
            fix: false,
            select: None,
            no_install: false,
            fail_on_warnings: false,
            ignore_rule: Vec::new(),
            ignore_package: Vec::new(),
            ignore_dependency: Vec::new(),
            ignore_rule_for_package: Vec::new(),
        };

        let packages_list = collect_packages(&args.path).unwrap();
        assert_eq!(packages_list.root_package.get_name(), "unsync");
        assert_eq!(packages_list.packages.len(), 2);

        let config = args.into();
        let issues = collect_issues(&config, packages_list);
        assert_eq!(issues.total_len(), 2);

        let issues = issues.into_iter().collect::<IndexMap<_, _>>();

        assert_eq!(
            issues
                .get(&PackageType::Package(
                    "fixtures/unsync/packages/def".to_string()
                ))
                .unwrap()[0]
                .name(),
            "unsync-similar-dependencies"
        );
    }

    fn args_for(path: &str, ignore_package: &[&str], ignore_rule_for_package: &[&str]) -> Args {
        Args {
            path: path.into(),
            fix: false,
            select: None,
            no_install: true,
            fail_on_warnings: false,
            ignore_rule: Vec::new(),
            ignore_package: ignore_package.iter().map(|value| (*value).to_string()).collect(),
            ignore_dependency: Vec::new(),
            ignore_rule_for_package: ignore_rule_for_package
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
        }
    }

    fn issues_by_package(
        config: &Config,
        path: &str,
    ) -> IndexMap<PackageType, Vec<crate::rules::BoxIssue>> {
        let packages_list = collect_packages(Path::new(path)).unwrap();

        collect_issues(config, packages_list)
            .into_iter()
            .collect::<IndexMap<_, _>>()
    }

    fn package_type(path: &str) -> PackageType {
        PackageType::Package(path.to_string())
    }

    #[test]
    fn collect_member_toolchain_forbid() {
        let config = Config::from(args_for("fixtures/member-toolchain", &[], &[]));
        let issues = issues_by_package(&config, "fixtures/member-toolchain");

        assert_eq!(issues.len(), 2);
        for path in ["a", "b"] {
            let issue = &issues[&package_type(&format!(
                "fixtures/member-toolchain/packages/{path}"
            ))][0];

            assert_eq!(issue.name(), "member-toolchain-fields");
            assert_eq!(issue.level(), crate::rules::IssueLevel::Warning);
        }
    }

    #[test]
    fn collect_member_toolchain_match_root() {
        let mut config = Config::from(args_for("fixtures/member-toolchain", &[], &[]));
        config.member_toolchain = Some(crate::packages::MemberToolchain::MatchRoot);

        let issues = issues_by_package(&config, "fixtures/member-toolchain");

        assert_eq!(issues.len(), 1);
        assert!(issues.contains_key(&package_type("fixtures/member-toolchain/packages/a")));
    }

    #[test]
    fn collect_member_toolchain_ignore_package() {
        let config = Config::from(args_for("fixtures/member-toolchain", &["a"], &[]));
        let issues = issues_by_package(&config, "fixtures/member-toolchain");

        assert_eq!(issues.len(), 1);
        assert!(issues.contains_key(&package_type("fixtures/member-toolchain/packages/b")));
    }

    #[test]
    fn collect_ignore_rule_for_package() {
        let config = Config::from(args_for(
            "fixtures/member-toolchain",
            &[],
            &["member-toolchain-fields:a"],
        ));
        let issues = issues_by_package(&config, "fixtures/member-toolchain");

        assert_eq!(issues.len(), 1);
        assert!(issues.contains_key(&package_type("fixtures/member-toolchain/packages/b")));
    }

    #[test]
    fn collect_ignore_rule_for_package_glob_and_other_rule() {
        let config = Config::from(args_for(
            "fixtures/member-toolchain",
            &[],
            &["member-toolchain-fields:fixtures/member-toolchain/packages/*"],
        ));
        assert!(issues_by_package(&config, "fixtures/member-toolchain").is_empty());

        let config = Config::from(args_for(
            "fixtures/member-toolchain",
            &[],
            &["empty-dependencies:a"],
        ));
        assert_eq!(
            issues_by_package(&config, "fixtures/member-toolchain").len(),
            2
        );
    }

    #[test]
    fn collect_ignore_rule_for_package_unsync_similar_dependencies() {
        let config = Config::from(args_for(
            "fixtures/unsync",
            &[],
            &["unsync-similar-dependencies:def"],
        ));
        let issues = issues_by_package(&config, "fixtures/unsync");

        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[&PackageType::None][0].name(),
            "multiple-dependency-versions"
        );
    }

    #[test]
    fn collect_ignore_rule_for_package_multiple_dependency_versions() {
        let config = Config::from(args_for(
            "fixtures/unsync",
            &[],
            &["multiple-dependency-versions:abc"],
        ));
        let issues = issues_by_package(&config, "fixtures/unsync");

        assert_eq!(issues.len(), 1);
        assert_eq!(
            issues[&package_type("fixtures/unsync/packages/def")][0].name(),
            "unsync-similar-dependencies"
        );
    }

    #[test]
    fn config_merge_splits_on_first_colon() {
        let config = Config::from(args_for(
            "fixtures/basic",
            &[],
            &["rule:pkg:with:colons", "rule:b"],
        ));

        assert_eq!(
            config.ignore_rule_for_package.get("rule"),
            Some(&vec!["pkg:with:colons".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn collect_config_file_is_layered_over_package_json() {
        let path = Path::new("fixtures/config-file");
        let packages_list = collect_packages(path).unwrap();
        let mut config = packages_list.root_package.get_config().unwrap_or_default();

        assert_eq!(
            config.ignore_rule,
            vec!["root-package-dependencies".to_string()]
        );
        assert_eq!(
            collect_issues(&config, collect_packages(path).unwrap()).total_len(),
            2
        );

        config.overlay(crate::config_file::load(path).unwrap().unwrap());

        assert_eq!(
            config.ignore_rule,
            vec![
                "root-package-dependencies".to_string(),
                "empty-dependencies".to_string()
            ]
        );
        assert_eq!(collect_issues(&config, packages_list).total_len(), 0);
    }
}
