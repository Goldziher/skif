use super::{Issue, IssueLevel, PackageType};
use anyhow::{Result, anyhow};
use std::{
    borrow::Cow,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub struct PackagesWithoutPackageJsonIssue {
    package: String,
    fixed: bool,
}

impl PackagesWithoutPackageJsonIssue {
    pub fn new(package: String) -> Box<Self> {
        Box::new(Self {
            package,
            fixed: false,
        })
    }
}

impl Issue for PackagesWithoutPackageJsonIssue {
    fn name(&self) -> &'static str {
        "packages-without-package-json"
    }

    fn level(&self) -> IssueLevel {
        if self.fixed {
            IssueLevel::Fixed
        } else {
            IssueLevel::Warning
        }
    }

    fn message(&self) -> String {
        format!("   {}/package.json doesn't exist.", self.package)
    }

    fn why(&self) -> Cow<'static, str> {
        Cow::Borrowed("All packages matching the workspace should have a package.json file.")
    }

    fn fix(&mut self, _package_type: &PackageType) -> Result<()> {
        let path = PathBuf::from(&self.package).join("package.json");
        let package_name = Path::new(&self.package)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("Cannot derive a package name from {:?}", self.package))?;

        let value = serde_json::json!({
            "name": package_name,
            "version": "0.0.0",
            "private": true,
        });

        let value = serde_json::to_string_pretty(&value)?;
        fs::write(path, value)?;

        self.fixed = true;

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test() {
        let issue = PackagesWithoutPackageJsonIssue::new("test".to_string());

        assert_eq!(issue.name(), "packages-without-package-json");
        assert_eq!(issue.level(), IssueLevel::Warning);

        colored::control::set_override(false);
        assert_eq!(issue.message(), "   test/package.json doesn't exist.");
        assert_eq!(
            issue.why(),
            "All packages matching the workspace should have a package.json file."
        );
    }
}
