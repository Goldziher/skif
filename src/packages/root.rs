use super::{Catalogs, Package, Workspaces, semversion::SemVersion};
use crate::rules::{
    BoxIssue, root_package_dependencies::RootPackageDependenciesIssue,
    root_package_manager_field::RootPackageManagerFieldIssue, root_package_private_field::RootPackagePrivateFieldIssue,
};
use anyhow::Result;
use indexmap::IndexMap;
use std::path::Path;

#[derive(Debug)]
pub struct RootPackage(Package);

impl RootPackage {
    pub fn new(path: &Path) -> Result<Self> {
        let package = Package::new(path.to_path_buf())?;

        Ok(Self(package))
    }

    #[cfg(test)]
    pub fn get_name(&self) -> String {
        self.0.get_name().cloned().unwrap_or_default()
    }

    pub fn as_package(&self) -> &Package {
        &self.0
    }

    pub fn get_path(&self) -> String {
        self.0.get_path()
    }

    pub fn get_workspaces(&self) -> Option<Vec<String>> {
        match &self.0.inner.workspaces {
            Some(workspaces) => match workspaces {
                Workspaces::Default(workspaces) => Some(workspaces.clone()),
                Workspaces::Yarn { packages, .. } => Some(packages.clone()),
            },
            None => None,
        }
    }

    pub fn check_private(&self) -> Option<BoxIssue> {
        match self.0.inner.private {
            Some(true) => None,
            _ => Some(RootPackagePrivateFieldIssue::new()),
        }
    }

    pub fn check_package_manager(&self) -> Option<BoxIssue> {
        if self.0.has_package_manager() {
            None
        } else {
            Some(RootPackageManagerFieldIssue::new())
        }
    }

    pub fn check_dependencies(&self) -> Option<BoxIssue> {
        if self.0.inner.dependencies.is_some() {
            Some(RootPackageDependenciesIssue::new())
        } else {
            self.0.check_dependencies()
        }
    }

    pub fn check_dev_dependencies(&self) -> Option<BoxIssue> {
        self.0.check_dev_dependencies()
    }

    pub fn check_peer_dependencies(&self) -> Option<BoxIssue> {
        self.0.check_peer_dependencies()
    }

    pub fn check_optional_dependencies(&self) -> Option<BoxIssue> {
        self.0.check_optional_dependencies()
    }

    pub fn check_incompatible_peer_dependencies(&self, catalogs: &Catalogs) -> Option<BoxIssue> {
        self.0.check_incompatible_peer_dependencies(catalogs)
    }

    pub fn get_catalogs(&self) -> Catalogs {
        self.0.get_catalogs()
    }

    pub fn get_dependencies(&self, catalogs: &Catalogs) -> Option<IndexMap<String, SemVersion>> {
        self.0.get_dependencies(catalogs)
    }

    pub fn get_dev_dependencies(&self, catalogs: &Catalogs) -> Option<IndexMap<String, SemVersion>> {
        self.0.get_dev_dependencies(catalogs)
    }
}
