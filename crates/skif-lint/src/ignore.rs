use std::collections::HashSet;

use skif_config::Config;
use skif_core::Diagnostic;
use skif_workspace::Workspace;

/// `[[ignore]]` entries and `workspace.ignore-packages`, resolved against a workspace.
#[derive(Debug)]
pub struct Suppressions {
    ignored_packages: HashSet<String>,
    entries: Vec<Entry>,
}

#[derive(Debug)]
struct Entry {
    rules: HashSet<String>,
    /// `None` matches every package, including diagnostics without one.
    packages: Option<HashSet<String>>,
    dependencies: Vec<String>,
}

impl Suppressions {
    pub fn new(workspace: &Workspace, config: &Config) -> Self {
        let ignored_packages = workspace
            .all_packages()
            .filter(|package| workspace.is_ignored(package))
            .map(|package| package.display_name().to_owned())
            .collect();
        let entries = config
            .ignore
            .iter()
            .map(|entry| Entry {
                rules: entry.rules.iter().cloned().collect(),
                packages: (!entry.packages.is_empty()).then(|| {
                    workspace
                        .all_packages()
                        .filter(|package| entry.packages.iter().any(|pattern| package.matches(pattern)))
                        .map(|package| package.display_name().to_owned())
                        .collect()
                }),
                dependencies: entry.dependencies.clone(),
            })
            .collect();
        Self {
            ignored_packages,
            entries,
        }
    }

    pub fn suppresses(&self, diagnostic: &Diagnostic) -> bool {
        if diagnostic
            .package
            .as_ref()
            .is_some_and(|package| self.ignored_packages.contains(package))
        {
            return true;
        }
        self.entries.iter().any(|entry| entry.matches(diagnostic))
    }
}

impl Entry {
    fn matches(&self, diagnostic: &Diagnostic) -> bool {
        let rule_ok = self.rules.is_empty() || self.rules.contains(diagnostic.rule.as_str());
        let package_ok = self.packages.as_ref().is_none_or(|packages| {
            diagnostic
                .package
                .as_ref()
                .is_some_and(|package| packages.contains(package))
        });
        let dependency_ok = self.dependencies.is_empty()
            || diagnostic.dependency.as_deref().is_some_and(|dependency| {
                self.dependencies
                    .iter()
                    .any(|pattern| dependency_matches(pattern, dependency))
            });
        rule_ok && package_ok && dependency_ok
    }
}

/// Matches `pattern` against a dependency written as `name` or `name@specifier`.
fn dependency_matches(pattern: &str, dependency: &str) -> bool {
    glob_matches(pattern, dependency) || glob_matches(pattern, dependency_name(dependency))
}

fn dependency_name(dependency: &str) -> &str {
    match dependency.rfind('@') {
        Some(index) if index > 0 => &dependency[..index],
        _ => dependency,
    }
}

/// `*x*` is a substring match, `x*` a prefix match, `*x` a suffix match; anything else is exact.
fn glob_matches(pattern: &str, text: &str) -> bool {
    match (pattern.strip_prefix('*'), pattern.strip_suffix('*')) {
        (Some(_), Some(_)) => text.contains(pattern.trim_matches('*')),
        (Some(suffix), None) => text.ends_with(suffix),
        (None, Some(prefix)) => text.starts_with(prefix),
        (None, None) => pattern == text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_table() {
        let cases = [
            ("react", "react", true),
            ("react", "react-dom", false),
            ("react*", "react-dom", true),
            ("react*", "preact", false),
            ("*act", "preact", true),
            ("*act", "react-dom", false),
            ("*eac*", "preact", true),
            ("*eac*", "lodash", false),
            ("*", "anything", true),
            ("**", "anything", true),
        ];
        for (pattern, text, expected) in cases {
            assert_eq!(glob_matches(pattern, text), expected, "{pattern} vs {text}");
        }
    }

    #[test]
    fn dependency_name_table() {
        let cases = [
            ("react", "react"),
            ("react@18", "react"),
            ("@scope/x", "@scope/x"),
            ("@scope/x@1.0.0", "@scope/x"),
        ];
        for (dependency, name) in cases {
            assert_eq!(dependency_name(dependency), name, "{dependency}");
        }
    }
}
