use std::collections::{HashMap, HashSet};

use skif_config::{Config, Selector};
use skif_core::Diagnostic;
use skif_workspace::{PackageId, Workspace};

/// `[[ignore]]` entries and `workspace.ignore-packages`, resolved against a workspace.
///
/// Packages are resolved to [`PackageId`]s once; every per-diagnostic check afterwards is an index into a vector
/// by the diagnostic's [`PackageId`], or, for diagnostics that only carry a display name, a lookup of that name.
#[derive(Debug)]
pub struct Suppressions {
    /// Display name to the packages carrying it (names are not guaranteed unique).
    by_name: HashMap<String, Vec<PackageId>>,
    /// Indexed by package id.
    ignored: Vec<bool>,
    entries: Vec<Entry>,
}

#[derive(Debug)]
struct Entry {
    rules: HashSet<String>,
    /// Indexed by package id; `None` matches every package, including diagnostics without one.
    packages: Option<Vec<bool>>,
    dependencies: Selector,
}

impl Suppressions {
    pub fn new(workspace: &Workspace, config: &Config) -> Self {
        let mut by_name: HashMap<String, Vec<PackageId>> = HashMap::new();
        for (id, package) in workspace.iter() {
            by_name.entry(package.display_name().to_owned()).or_default().push(id);
        }
        let ignored = workspace.package_ids().map(|id| workspace.is_ignored(id)).collect();
        let entries = config
            .ignore
            .iter()
            .map(|entry| Entry {
                rules: entry.rules.iter().cloned().collect(),
                packages: (!entry.packages.is_empty()).then(|| {
                    workspace
                        .iter()
                        .map(|(_, package)| package.is_selected_by(&entry.packages))
                        .collect()
                }),
                dependencies: entry.dependencies.clone(),
            })
            .collect();
        Self {
            by_name,
            ignored,
            entries,
        }
    }

    pub fn suppresses(&self, diagnostic: &Diagnostic) -> bool {
        if self.package_flag(diagnostic, &self.ignored) {
            return true;
        }
        self.entries.iter().any(|entry| self.entry_matches(entry, diagnostic))
    }

    /// Whether the diagnostic's package is flagged in `flags`. A diagnostic with a [`PackageId`] is looked up by it;
    /// one with only a display name needs every package of that name flagged, since it cannot tell them apart.
    fn package_flag(&self, diagnostic: &Diagnostic, flags: &[bool]) -> bool {
        if let Some(id) = diagnostic.package_id {
            return flags.get(id.index()).copied().unwrap_or(false);
        }
        diagnostic
            .package
            .as_deref()
            .and_then(|name| self.by_name.get(name))
            .is_some_and(|ids| !ids.is_empty() && ids.iter().all(|id| flags.get(id.index()).copied().unwrap_or(false)))
    }

    fn entry_matches(&self, entry: &Entry, diagnostic: &Diagnostic) -> bool {
        let rule_ok = entry.rules.is_empty() || entry.rules.contains(diagnostic.rule.as_str());
        let package_ok = entry
            .packages
            .as_ref()
            .is_none_or(|flags| self.package_flag(diagnostic, flags));
        let dependency_ok = entry.dependencies.is_empty()
            || diagnostic
                .dependency
                .as_deref()
                .is_some_and(|dependency| self.dependency_matches(&entry.dependencies, dependency, diagnostic));
        rule_ok && package_ok && dependency_ok
    }

    /// Matches `selector` against the dependency of `diagnostic`: its name, and `name@specifier` when the diagnostic
    /// carries the raw specifier (a dependency recorded as `name@specifier` is matched as written). The dependency
    /// is `$LOCAL` when its name is a workspace package.
    fn dependency_matches(&self, selector: &Selector, dependency: &str, diagnostic: &Diagnostic) -> bool {
        let name = dependency_name(dependency);
        let versioned = diagnostic
            .specifier
            .as_deref()
            .map(|specifier| format!("{name}@{specifier}"));
        let candidates = [dependency, name].into_iter().chain(versioned.as_deref());
        selector.matches_any(candidates, self.by_name.contains_key(name))
    }
}

fn dependency_name(dependency: &str) -> &str {
    match dependency.rfind('@') {
        Some(index) if index > 0 => &dependency[..index],
        _ => dependency,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::Path;

    use skif_core::{Category, Fixability, Location, RuleMeta, Severity, diagnostic};
    use skif_workspace::MemoryFs;

    use super::*;

    const META: RuleMeta = RuleMeta {
        id: "demo",
        category: Category::Manifest,
        default: Some(Severity::Warning),
        fixable: Fixability::None,
        summary: "demo",
    };

    fn workspace(config: &Config) -> Workspace {
        let fs = MemoryFs::new()
            .with_file("/ws/package.json", r#"{"name":"root","workspaces":["p/*"]}"#)
            .with_file("/ws/p/a/package.json", r#"{"name":"a"}"#)
            .with_file("/ws/p/b/package.json", r#"{"name":"b"}"#)
            .with_file("/ws/p/dup1/package.json", r#"{"name":"dup"}"#)
            .with_file("/ws/p/dup2/package.json", r#"{"name":"dup"}"#);
        Workspace::discover_with_fs(Path::new("/ws"), config, &fs).unwrap()
    }

    fn make(package: Option<&str>, dependency: Option<&str>) -> Diagnostic {
        let mut builder = diagnostic(
            &META,
            "m",
            Location {
                path: "package.json".into(),
                span: None,
            },
        );
        if let Some(package) = package {
            builder = builder.package(package);
        }
        if let Some(dependency) = dependency {
            builder = builder.dependency(dependency);
        }
        builder.build()
    }

    fn suppresses(toml: &str, package: Option<&str>, dependency: Option<&str>) -> bool {
        let config = Config::from_toml_str(toml).unwrap();
        Suppressions::new(&workspace(&config), &config).suppresses(&make(package, dependency))
    }

    #[test]
    fn dependency_selector_table() {
        let cases = [
            ("react", "react", true),
            ("react", "react@18", true),
            ("react@18", "react@18", true),
            ("react@17", "react@18", false),
            ("react", "react-dom", false),
            ("react*", "react-dom", true),
            ("react*", "preact", false),
            ("*act", "preact", true),
            ("*act", "react-dom", false),
            ("*eac*", "preact@1", true),
            ("*eac*", "lodash", false),
            ("*", "anything", true),
            ("**", "anything", true),
            ("@scope/*", "@scope/x@1.0.0", true),
            ("!react", "react@18", false),
            ("!react", "lodash", true),
            ("$LOCAL", "a", true),
            ("$LOCAL", "a@workspace:*", true),
            ("$LOCAL", "lodash", false),
            ("!$LOCAL", "a", false),
            ("!$LOCAL", "lodash", true),
        ];
        for (pattern, dependency, expected) in cases {
            let toml = format!("[[ignore]]\ndependencies = [\"{pattern}\"]\n");
            assert_eq!(
                suppresses(&toml, None, Some(dependency)),
                expected,
                "{pattern} vs {dependency}"
            );
        }
    }

    fn suppresses_instance(toml: &str, name: &str, specifier: &str) -> bool {
        let config = Config::from_toml_str(toml).unwrap();
        let mut diagnostic = make(None, Some(name));
        diagnostic.specifier = Some(specifier.to_owned());
        Suppressions::new(&workspace(&config), &config).suppresses(&diagnostic)
    }

    #[test]
    fn name_at_specifier_matches_the_recorded_specifier() {
        let cases = [
            ("react@^18.0.0", "react", "^18.0.0", true),
            ("react@^18.0.0", "react", "^17.0.0", false),
            ("react@^18.0.0", "react-dom", "^18.0.0", false),
            ("react", "react", "^17.0.0", true),
            ("react@*", "react", "^17.0.0", true),
            ("react@^1*", "react", "^17.0.0", true),
            ("!react@^17.0.0", "react", "^17.0.0", false),
            ("!react@^17.0.0", "react", "^18.0.0", true),
            ("@scope/pkg@^1.0.0", "@scope/pkg", "^1.0.0", true),
            ("@scope/pkg@^1.0.0", "@scope/pkg", "^2.0.0", false),
            ("@scope/pkg", "@scope/pkg", "^2.0.0", true),
            ("@scope/*@^1.0.0", "@scope/pkg", "^1.0.0", true),
            ("@scope/*@^1.0.0", "@other/pkg", "^1.0.0", false),
            ("$LOCAL", "a", "workspace:*", true),
        ];
        for (pattern, name, specifier, expected) in cases {
            let toml = format!("[[ignore]]\ndependencies = [\"{pattern}\"]\n");
            assert_eq!(
                suppresses_instance(&toml, name, specifier),
                expected,
                "{pattern} vs {name}@{specifier}"
            );
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

    #[test]
    fn package_entries_resolve_by_id_not_by_pattern_at_check_time() {
        let toml = "[[ignore]]\npackages = [\"p/*\", \"!b\"]\n";
        assert!(suppresses(toml, Some("a"), None));
        assert!(!suppresses(toml, Some("b"), None));
        assert!(!suppresses(toml, Some("root"), None));
        assert!(!suppresses(toml, Some("unknown"), None));
        assert!(!suppresses(toml, None, None));
    }

    #[test]
    fn workspace_ignore_packages_flags_ids() {
        let toml = "[workspace]\nignore-packages = [\"a\", \"p/b\"]\n";
        assert!(suppresses(toml, Some("a"), None));
        assert!(suppresses(toml, Some("b"), None));
        assert!(!suppresses(toml, Some("root"), None));
        assert!(!suppresses(toml, None, None));
    }

    #[test]
    fn a_name_shared_by_packages_is_suppressed_only_when_all_of_them_are() {
        assert!(!suppresses(
            "[workspace]\nignore-packages = [\"p/dup1\"]\n",
            Some("dup"),
            None
        ));
        assert!(suppresses(
            "[workspace]\nignore-packages = [\"dup\"]\n",
            Some("dup"),
            None
        ));
        assert!(suppresses(
            "[workspace]\nignore-packages = [\"p/dup*\"]\n",
            Some("dup"),
            None
        ));
    }

    #[test]
    fn package_id_tells_apart_packages_sharing_a_name() {
        let config = Config::from_toml_str("[workspace]\nignore-packages = [\"p/dup1\"]\n").unwrap();
        let workspace = workspace(&config);
        let suppressions = Suppressions::new(&workspace, &config);
        let id_of = |dir: &str| {
            workspace
                .iter()
                .find(|(_, package)| package.dir == Path::new(dir))
                .map(|(id, _)| id)
                .unwrap()
        };
        let with_id = |id| {
            let mut diagnostic = make(Some("dup"), None);
            diagnostic.package_id = Some(id);
            diagnostic
        };
        assert!(suppressions.suppresses(&with_id(id_of("p/dup1"))));
        assert!(!suppressions.suppresses(&with_id(id_of("p/dup2"))));
        assert!(
            !suppressions.suppresses(&make(Some("dup"), None)),
            "name alone stays ambiguous"
        );
    }

    #[test]
    fn package_id_wins_over_a_conflicting_display_name() {
        let config = Config::from_toml_str("[workspace]\nignore-packages = [\"a\"]\n").unwrap();
        let workspace = workspace(&config);
        let suppressions = Suppressions::new(&workspace, &config);
        let (id, _) = workspace
            .iter()
            .find(|(_, package)| package.display_name() == "a")
            .unwrap();
        let mut diagnostic = make(Some("something-else"), None);
        diagnostic.package_id = Some(id);
        assert!(suppressions.suppresses(&diagnostic));
    }

    #[test]
    fn rule_dependency_and_package_must_all_match() {
        let toml = "[[ignore]]\nrules = [\"demo\"]\npackages = [\"a\"]\ndependencies = [\"react\"]\n";
        assert!(suppresses(toml, Some("a"), Some("react@18")));
        assert!(!suppresses(toml, Some("b"), Some("react@18")));
        assert!(!suppresses(toml, Some("a"), Some("lodash")));
        assert!(!suppresses(toml, Some("a"), None));
        assert!(!suppresses("[[ignore]]\nrules = [\"other\"]\n", Some("a"), None));
    }
}
