use super::{Issue, IssueLevel};
use crate::packages::semversion::SemVersion;
use colored::Colorize;
use std::borrow::Cow;

#[derive(Debug)]
pub struct IncompatiblePeerDependency {
    pub name: String,
    pub peer: SemVersion,
    pub dev: SemVersion,
}

#[derive(Debug)]
pub struct IncompatiblePeerDependenciesIssue {
    dependencies: Vec<IncompatiblePeerDependency>,
}

impl IncompatiblePeerDependenciesIssue {
    pub fn new(dependencies: Vec<IncompatiblePeerDependency>) -> Box<Self> {
        Box::new(Self { dependencies })
    }
}

impl Issue for IncompatiblePeerDependenciesIssue {
    fn name(&self) -> &'static str {
        "incompatible-peer-dependencies"
    }

    fn level(&self) -> IssueLevel {
        IssueLevel::Warning
    }

    fn message(&self) -> String {
        let peers = self
            .dependencies
            .iter()
            .map(|dependency| {
                format!(
                    r#"  │     "{}": "{}","#,
                    dependency.name.white(),
                    dependency.peer.to_string().blue()
                )
            })
            .collect::<Vec<String>>()
            .join("\n");

        let devs = self
            .dependencies
            .iter()
            .map(|dependency| {
                format!(
                    r#"  │     "{}": "{}","#,
                    dependency.name.white(),
                    dependency.dev.to_string().red()
                )
            })
            .collect::<Vec<String>>()
            .join("\n");

        format!(
            r#"  │ {{
  │   "{}": {{      {}
{}
  │   }},
  │   ...
  │   "{}": {{   {}
{}
  │   }}
  │ }}"#,
            "peerDependencies".white(),
            "← requires...".blue(),
            peers,
            "devDependencies".white(),
            "← but the version used here doesn't satisfy it.".red(),
            devs,
        )
        .bright_black()
        .to_string()
    }

    fn why(&self) -> Cow<'static, str> {
        Cow::Borrowed("A dependency in devDependencies should satisfy the range declared in peerDependencies.")
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test() {
        let issue = IncompatiblePeerDependenciesIssue::new(vec![IncompatiblePeerDependency {
            name: "lib".into(),
            peer: SemVersion::parse("3.45.1-build2").unwrap(),
            dev: SemVersion::parse("3.45.3-build2").unwrap(),
        }]);

        assert_eq!(issue.name(), "incompatible-peer-dependencies");
        assert_eq!(issue.level(), IssueLevel::Warning);

        colored::control::set_override(false);
        let message = issue.message();
        assert!(message.contains(r#""peerDependencies": {"#));
        assert!(message.contains(r#""lib": "3.45.1-build2","#));
        assert!(message.contains(r#""devDependencies": {"#));
        assert!(message.contains(r#""lib": "3.45.3-build2","#));
        assert_eq!(
            issue.why(),
            "A dependency in devDependencies should satisfy the range declared in peerDependencies."
        );
    }
}
