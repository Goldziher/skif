use super::{Issue, IssueLevel, PackageType, empty_dependencies::DependencyKind};
use crate::json;
use anyhow::Result;
use colored::Colorize;
use std::{borrow::Cow, fs, path::PathBuf};

const LOCAL_PATH_PROTOCOLS: [&str; 3] = ["link:", "file:", "portal:"];

/// Whether a specifier points at a path on disk instead of a registry, workspace or git source.
pub fn is_local_path_specifier(specifier: &str) -> bool {
    LOCAL_PATH_PROTOCOLS
        .iter()
        .any(|protocol| specifier.starts_with(protocol))
        || specifier.starts_with("./")
        || specifier.starts_with("../")
        || specifier.starts_with('/')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPathDependency {
    pub kind: DependencyKind,
    pub name: String,
    pub specifier: String,
    /// The specifier to use instead, when the dependency is a package of the workspace.
    pub replacement: Option<String>,
}

#[derive(Debug)]
pub struct LocalPathDependenciesIssue {
    dependencies: Vec<LocalPathDependency>,
    fixed: bool,
}

impl LocalPathDependenciesIssue {
    pub fn new(dependencies: Vec<LocalPathDependency>) -> Box<Self> {
        Box::new(Self {
            dependencies,
            fixed: false,
        })
    }
}

impl Issue for LocalPathDependenciesIssue {
    fn name(&self) -> &'static str {
        "local-path-dependencies"
    }

    fn level(&self) -> IssueLevel {
        if self.fixed {
            IssueLevel::Fixed
        } else {
            IssueLevel::Error
        }
    }

    fn message(&self) -> String {
        let lines = self
            .dependencies
            .iter()
            .map(|dependency| {
                let removed = format!(
                    r#"  {}   "{}": {{ "{}": "{}" }}"#,
                    "-".red(),
                    dependency.kind.to_string().white(),
                    dependency.name.white(),
                    dependency.specifier.white(),
                );

                match &dependency.replacement {
                    Some(replacement) => format!(
                        r#"{removed}   {}
  {}   "{}": {{ "{}": "{}" }}   {}"#,
                        "← relative path to a workspace package...".red(),
                        "+".green(),
                        dependency.kind.to_string().white(),
                        dependency.name.white(),
                        replacement.white(),
                        "← use the workspace protocol instead.".green(),
                    ),
                    None => format!(
                        "{removed}   {}",
                        "← local path outside the workspace packages.".red()
                    ),
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!("  │ {{\n{lines}\n  │ }}")
            .bright_black()
            .to_string()
    }

    fn why(&self) -> Cow<'static, str> {
        Cow::Borrowed(
            "Dependencies should not point at relative paths: link to workspace packages with the workspace protocol, and publish or vendor anything else.",
        )
    }

    fn fix(&mut self, package_type: &PackageType) -> Result<()> {
        let path = match package_type {
            PackageType::Root => PathBuf::from("package.json"),
            PackageType::Package(path) => PathBuf::from(path).join("package.json"),
            PackageType::None => return Ok(()),
        };

        let fixable = self
            .dependencies
            .iter()
            .filter_map(|dependency| {
                dependency
                    .replacement
                    .as_ref()
                    .map(|replacement| (dependency, replacement))
            })
            .collect::<Vec<_>>();

        if fixable.is_empty() {
            return Ok(());
        }

        let value = fs::read_to_string(&path)?;
        let (mut value, indent, lineending) = json::deserialize::<serde_json::Value>(&value)?;

        for (dependency, replacement) in &fixable {
            if let Some(entry) = value
                .get_mut(dependency.kind.to_string())
                .and_then(|dependencies| dependencies.get_mut(&dependency.name))
            {
                *entry = serde_json::Value::String((*replacement).clone());
            }
        }

        fs::write(path, json::serialize(&value, &indent, lineending)?)?;

        self.fixed = fixable.len() == self.dependencies.len();

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn local_path_specifiers() {
        for specifier in [
            "link:../../packages/ui",
            "file:../shared",
            "file:./vendor/lib.tgz",
            "portal:../tool",
            "../shared",
            "./local",
            "/abs/path",
        ] {
            assert!(is_local_path_specifier(specifier), "{specifier}");
        }

        for specifier in [
            "workspace:*",
            "^1.2.3",
            "catalog:",
            "npm:react@18",
            "github:owner/repo",
            "git+https://example.com/repo.git",
            "https://example.com/pkg.tgz",
            "latest",
        ] {
            assert!(!is_local_path_specifier(specifier), "{specifier}");
        }
    }

    #[test]
    fn issue_metadata() {
        colored::control::set_override(false);
        let issue = LocalPathDependenciesIssue::new(vec![
            LocalPathDependency {
                kind: DependencyKind::Dependencies,
                name: "@repo/ui".into(),
                specifier: "link:../../packages/ui".into(),
                replacement: Some("workspace:*".into()),
            },
            LocalPathDependency {
                kind: DependencyKind::DevDependencies,
                name: "vendored".into(),
                specifier: "file:./vendor/vendored.tgz".into(),
                replacement: None,
            },
        ]);

        assert_eq!(issue.name(), "local-path-dependencies");
        assert_eq!(issue.level(), IssueLevel::Error);

        let message = issue.message();
        assert!(message.contains(r#""dependencies": { "@repo/ui": "link:../../packages/ui" }"#));
        assert!(message.contains(r#""dependencies": { "@repo/ui": "workspace:*" }"#));
        assert!(message.contains("local path outside the workspace packages"));
    }

    #[test]
    fn fix_rewrites_workspace_links_only() {
        let dir = std::env::temp_dir().join(format!(
            "sherif-local-path-dependencies-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let package_json = dir.join("package.json");
        fs::write(
            &package_json,
            "{\n    \"name\": \"app\",\n    \"dependencies\": {\n        \"@repo/ui\": \"link:../ui\",\n        \"vendored\": \"file:./vendor.tgz\"\n    }\n}\n",
        )
        .unwrap();

        let mut issue = LocalPathDependenciesIssue::new(vec![
            LocalPathDependency {
                kind: DependencyKind::Dependencies,
                name: "@repo/ui".into(),
                specifier: "link:../ui".into(),
                replacement: Some("workspace:*".into()),
            },
            LocalPathDependency {
                kind: DependencyKind::Dependencies,
                name: "vendored".into(),
                specifier: "file:./vendor.tgz".into(),
                replacement: None,
            },
        ]);

        issue
            .fix(&PackageType::Package(dir.to_string_lossy().to_string()))
            .unwrap();

        assert_eq!(
            fs::read_to_string(&package_json).unwrap(),
            "{\n    \"name\": \"app\",\n    \"dependencies\": {\n        \"@repo/ui\": \"workspace:*\",\n        \"vendored\": \"file:./vendor.tgz\"\n    }\n}\n"
        );
        assert_eq!(issue.level(), IssueLevel::Error);

        fs::remove_dir_all(dir).unwrap();
    }
}
