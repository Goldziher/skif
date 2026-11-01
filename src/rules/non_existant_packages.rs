use super::{Issue, IssueLevel, PackageType};
use crate::json;
use anyhow::{Result, anyhow};
use colored::Colorize;
use std::{borrow::Cow, fs, path::PathBuf};

#[derive(Debug)]
pub struct NonExistantPackagesIssue {
    pnpm_workspace: bool,
    packages_list: Vec<String>,
    paths: Vec<String>,
    fixed: bool,
}

impl NonExistantPackagesIssue {
    pub fn new(pnpm_workspace: bool, packages_list: Vec<String>, paths: Vec<String>) -> Box<Self> {
        Box::new(Self {
            pnpm_workspace,
            packages_list,
            paths,
            fixed: false,
        })
    }

    /// Drops the non-existent paths from the `field` array of the workspace definition `value`.
    fn retain_existing(&self, value: &mut serde_json::Value, field: &str) -> Result<()> {
        let packages = value
            .get_mut(field)
            .and_then(serde_json::Value::as_array_mut)
            .ok_or_else(|| anyhow!("Expected `{field}` to be an array"))?;

        packages.retain(|package| {
            package
                .as_str()
                .is_none_or(|package| !self.paths.iter().any(|path| path == package))
        });

        Ok(())
    }

    fn pnpm_message(&self) -> String {
        let workspaces = self
            .packages_list
            .iter()
            .map(|package| {
                if self.paths.contains(package) {
                    format!(
                        "  {}  - '{}'   {}",
                        "-".red(),
                        package.white(),
                        "← but this one doesn't match any package".red(),
                    )
                } else {
                    format!("  │  - '{package}'")
                }
            })
            .collect::<Vec<String>>()
            .join("\n");

        format!(
            r"  │ packages:   {}
{}",
            "← Workspace has paths defined...".blue(),
            workspaces,
        )
        .bright_black()
        .to_string()
    }

    fn package_message(&self) -> String {
        let workspaces = self
            .packages_list
            .iter()
            .map(|package| {
                if self.paths.contains(package) {
                    format!(
                        r#"  {}     "{}",   {}"#,
                        "-".red(),
                        package.white(),
                        "← but this one doesn't match any package".red(),
                    )
                } else {
                    format!(r#"  │     "{package}","#)
                }
            })
            .collect::<Vec<String>>()
            .join("\n");

        format!(
            r#"  │ {{
  │   "workspaces": [   {}
{}
  │   ],
  │ }}"#,
            "← Workspace has paths defined...".blue(),
            workspaces,
        )
        .bright_black()
        .to_string()
    }
}

impl Issue for NonExistantPackagesIssue {
    fn name(&self) -> &'static str {
        "non-existant-packages"
    }

    fn level(&self) -> IssueLevel {
        if self.fixed {
            IssueLevel::Fixed
        } else {
            IssueLevel::Warning
        }
    }

    fn message(&self) -> String {
        if self.pnpm_workspace {
            self.pnpm_message()
        } else {
            self.package_message()
        }
    }

    fn why(&self) -> Cow<'static, str> {
        Cow::Borrowed("All paths defined in the workspace should match at least one package.")
    }

    fn fix(&mut self, package_type: &PackageType) -> Result<()> {
        if let PackageType::None = package_type {
            if self.pnpm_workspace {
                let path = PathBuf::from("pnpm-workspace.yaml");
                let value = fs::read_to_string(&path)?;
                let mut value = serde_saphyr::from_str::<serde_json::Value>(&value)?;

                self.retain_existing(&mut value, "packages")?;

                let value = serde_saphyr::to_string(&value)?;
                fs::write(path, value)?;

                self.fixed = true;
            } else {
                let path = PathBuf::from("package.json");
                let value = fs::read_to_string(&path)?;
                let (mut value, indent, lineending) =
                    json::deserialize::<serde_json::Value>(&value)?;

                self.retain_existing(&mut value, "workspaces")?;

                let value = json::serialize(&value, &indent, lineending)?;
                fs::write(path, value)?;

                self.fixed = true;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test() {
        let issue = NonExistantPackagesIssue::new(
            true,
            vec![
                "apps/*".into(),
                "packages/*".into(),
                "empty/*".into(),
                "docs".into(),
            ],
            vec!["empty/*".into(), "docs".into()],
        );

        assert_eq!(issue.name(), "non-existant-packages");
        assert_eq!(issue.level(), IssueLevel::Warning);
        assert_eq!(
            issue.why(),
            "All paths defined in the workspace should match at least one package."
        );
    }

    #[test]
    fn test_pnpm_workspace() {
        let issue = NonExistantPackagesIssue::new(
            true,
            vec![
                "apps/*".into(),
                "packages/*".into(),
                "empty/*".into(),
                "docs".into(),
            ],
            vec!["empty/*".into(), "docs".into()],
        );

        colored::control::set_override(false);
        insta::assert_snapshot!(issue.message());
    }

    #[test]
    fn test_package_workspace() {
        let issue = NonExistantPackagesIssue::new(
            false,
            vec![
                "apps/*".into(),
                "packages/*".into(),
                "empty/*".into(),
                "docs".into(),
            ],
            vec!["empty/*".into(), "docs".into()],
        );

        colored::control::set_override(false);
        insta::assert_snapshot!(issue.message());
    }
}
