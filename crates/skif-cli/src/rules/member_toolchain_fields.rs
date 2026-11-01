use super::{Issue, IssueLevel, PackageType};
use crate::json;
use anyhow::Result;
use colored::Colorize;
use std::{borrow::Cow, fs, path::PathBuf};

#[derive(Debug)]
pub struct MemberToolchainFieldsIssue {
    fields: Vec<&'static str>,
    fixed: bool,
}

impl MemberToolchainFieldsIssue {
    pub fn new(fields: Vec<&'static str>) -> Box<Self> {
        Box::new(Self { fields, fixed: false })
    }
}

impl Issue for MemberToolchainFieldsIssue {
    fn name(&self) -> &'static str {
        "member-toolchain-fields"
    }

    fn level(&self) -> IssueLevel {
        if self.fixed {
            IssueLevel::Fixed
        } else {
            IssueLevel::Warning
        }
    }

    fn message(&self) -> String {
        let fields = self
            .fields
            .iter()
            .map(|field| {
                format!(
                    "  {}   \"{}\": ...   {}",
                    "-".red(),
                    field.white(),
                    format!("← the root package.json owns `{field}`.").red(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!("  │ {{\n{fields}\n  │ }}").bright_black().to_string()
    }

    fn why(&self) -> Cow<'static, str> {
        Cow::Borrowed(
            "The root package.json owns the toolchain: member packages should not declare `packageManager` or `engines`, since they can drift from the root.",
        )
    }

    fn fix(&mut self, package_type: &PackageType) -> Result<()> {
        if let PackageType::Package(path) = package_type {
            let path = PathBuf::from(path).join("package.json");
            let value = fs::read_to_string(&path)?;
            let (mut value, indent, lineending) = json::deserialize::<serde_json::Value>(&value)?;

            let Some(object) = value.as_object_mut() else {
                return Ok(());
            };

            let mut removed = false;

            for field in &self.fields {
                removed |= object.remove(*field).is_some();
            }

            if removed {
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
        let issue = MemberToolchainFieldsIssue::new(vec!["packageManager", "engines"]);

        assert_eq!(issue.name(), "member-toolchain-fields");
        assert_eq!(issue.level(), IssueLevel::Warning);
        assert_eq!(
            issue.why(),
            "The root package.json owns the toolchain: member packages should not declare `packageManager` or `engines`, since they can drift from the root."
        );
    }

    #[test]
    fn message_lists_each_field() {
        colored::control::set_override(false);

        let issue = MemberToolchainFieldsIssue::new(vec!["packageManager", "engines"]);
        let message = issue.message();

        assert!(message.contains(r#""packageManager": ..."#));
        assert!(message.contains(r#""engines": ..."#));
        assert!(message.contains("← the root package.json owns `engines`."));
    }

    #[test]
    fn fix_removes_fields_and_keeps_formatting() {
        let directory = std::env::temp_dir().join(format!("skif-member-toolchain-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("package.json"),
            "{\n    \"name\": \"member\",\n    \"packageManager\": \"pnpm@9.0.0\",\n    \"engines\": {\n        \"node\": \">=20\"\n    },\n    \"version\": \"1.0.0\"\n}\n",
        )
        .unwrap();

        let mut issue = MemberToolchainFieldsIssue::new(vec!["packageManager", "engines"]);
        issue
            .fix(&PackageType::Package(directory.to_string_lossy().to_string()))
            .unwrap();

        let result = fs::read_to_string(directory.join("package.json")).unwrap();
        fs::remove_dir_all(&directory).unwrap();

        assert_eq!(issue.level(), IssueLevel::Fixed);
        assert_eq!(result, "{\n    \"name\": \"member\",\n    \"version\": \"1.0.0\"\n}\n");
    }

    #[test]
    fn fix_ignores_root() {
        let mut issue = MemberToolchainFieldsIssue::new(vec!["engines"]);

        issue.fix(&PackageType::Root).unwrap();

        assert_eq!(issue.level(), IssueLevel::Warning);
    }
}
