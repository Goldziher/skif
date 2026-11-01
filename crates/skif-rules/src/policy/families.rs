//! `unsynced-dependency-family`: packages released together must be declared on one version within a package.

use skif_config::{Config, Selector};
use skif_core::{Category, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};
use skif_specifier::Specifier;
use skif_workspace::DependencyEntry;

use super::catalog::support::{active_packages, value_location};

/// Metadata of `unsynced-dependency-family`.
pub static META: RuleMeta = RuleMeta {
    id: "unsynced-dependency-family",
    category: Category::Policy,
    default: Some(Severity::Error),
    fixable: Fixability::None,
    summary: "Members of a dependency family declared in one package are on different versions.",
};

/// The families that ship with skif: packages whose releases are versioned in lockstep. Configuration can disable
/// one by name (`enabled = false`) or replace its members by declaring a family of the same name.
const BUILTIN: &[(&str, &[&str])] = &[
    ("react", &["react", "react-dom", "react-test-renderer"]),
    ("trpc", &["@trpc/*"]),
    ("next", &["next", "@next/*", "eslint-config-next"]),
    ("storybook", &["storybook", "@storybook/*"]),
    ("prisma", &["prisma", "@prisma/client"]),
    ("typescript-eslint", &["typescript-eslint", "@typescript-eslint/*"]),
    ("playwright", &["playwright", "playwright-core", "@playwright/test"]),
];

#[derive(Debug)]
struct Family {
    name: String,
    members: Selector,
}

/// The built-in families adjusted by `[[dependency-families]]`, followed by the configured families of other names.
fn effective_families(config: &Config) -> Vec<Family> {
    let compile = |name: &str, patterns: Vec<String>| {
        Selector::new(patterns).ok().map(|members| Family {
            name: name.to_owned(),
            members,
        })
    };
    let configured = |name: &str| config.dependency_families.iter().rfind(|family| family.name == name);
    let builtin = BUILTIN.iter().filter_map(|(name, patterns)| match configured(name) {
        Some(family) if !family.enabled => None,
        Some(family) if !family.packages.is_empty() => compile(name, family.packages.clone()),
        _ => compile(name, patterns.iter().map(|pattern| (*pattern).to_owned()).collect()),
    });
    let custom = config
        .dependency_families
        .iter()
        .filter(|family| family.enabled && BUILTIN.iter().all(|(name, _)| *name != family.name))
        .filter_map(|family| compile(&family.name, family.packages.clone()));
    builtin.chain(custom).collect()
}

/// Reports family members that are on an older version than another member of the same package.
///
/// Versions are compared by number only, so `^18.3.1` and `~18.3.1` are in sync. A `catalog:` reference is compared
/// by the version of its catalog entry; specifiers without a version number (`workspace:*`, tags) take no part.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnsyncedDependencyFamily;

/// The rules of this module.
pub(crate) fn rules() -> Vec<Box<dyn Rule>> {
    vec![Box::new(UnsyncedDependencyFamily)]
}

impl Rule for UnsyncedDependencyFamily {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        if !cx.enabled.contains(META.id) {
            return;
        }
        let families = effective_families(cx.config);
        let catalogs = &cx.workspace.catalogs;
        let version_of = |entry: &DependencyEntry| {
            if entry.parsed.is_catalog() {
                Specifier::new(catalogs.resolve(&entry.name, &entry.specifier)?).get_node_version()
            } else {
                entry.parsed.get_node_version()
            }
        };
        for package in active_packages(cx.workspace) {
            for family in &families {
                let members: Vec<_> = package
                    .dependencies
                    .iter()
                    .filter(|entry| family.members.matches(&entry.name, false))
                    .filter_map(|entry| Some((entry, version_of(entry)?)))
                    .collect();
                let Some((leader, highest)) = members.iter().max_by(|a, b| a.1.cmp(&b.1)) else {
                    continue;
                };
                for (entry, version) in &members {
                    if version >= highest {
                        continue;
                    }
                    out.emit(
                        diagnostic(
                            &META,
                            format!(
                                "`{}` is on `{}` but `{}` of the `{}` family is on `{}`",
                                entry.name, entry.specifier, leader.name, family.name, leader.specifier
                            ),
                            value_location(package, entry),
                        )
                        .package(package.display_name())
                        .dependency(entry.name.clone())
                        .help(format!("align `{}` with the other `{}` packages", entry.name, family.name))
                        .build(),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn names(toml: &str) -> Vec<String> {
        let config = Config::from_toml_str(toml).unwrap();
        effective_families(&config).into_iter().map(|family| family.name).collect()
    }

    #[test]
    fn builtin_families_are_adjusted_by_configuration() {
        assert!(names("").contains(&"react".to_owned()));
        let disabled = names("[[dependency-families]]\nname = \"react\"\nenabled = false\n");
        assert!(!disabled.contains(&"react".to_owned()));
        assert!(disabled.contains(&"trpc".to_owned()));
        let custom = names("[[dependency-families]]\nname = \"babel\"\npackages = [\"@babel/*\"]\n");
        assert_eq!(custom.last().map(String::as_str), Some("babel"));
        let replaced = names("[[dependency-families]]\nname = \"react\"\npackages = [\"react\"]\n");
        assert_eq!(replaced.iter().filter(|name| *name == "react").count(), 1);
    }
}
