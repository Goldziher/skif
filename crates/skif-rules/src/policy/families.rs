//! `unsynced-dependency-family`: packages released together must be declared on one version within a package.

use std::sync::Arc;

use skif_config::{Config, Selector};
use skif_core::{Category, DependencyKind, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};
use skif_specifier::Specifier;
use skif_workspace::DependencyEntry;

use super::catalog::support::{active_packages, value_location};
use super::specifiers::{alias_target, floor_version, major};

/// Metadata of `unsynced-dependency-family`.
pub static META: RuleMeta = RuleMeta {
    id: "unsynced-dependency-family",
    category: Category::Policy,
    default: Some(Severity::Error),
    fixable: Fixability::None,
    summary: "Members of a dependency family declared in one package are on different versions.",
};

/// The Storybook packages released in lockstep with `storybook`. `@storybook/*` also holds packages on their own
/// version line (`@storybook/csf`, `@storybook/test-runner`, `@storybook/addon-styling`), so the family is a list.
const STORYBOOK: &[&str] = &[
    "storybook",
    "@storybook/addon-a11y",
    "@storybook/addon-actions",
    "@storybook/addon-backgrounds",
    "@storybook/addon-controls",
    "@storybook/addon-docs",
    "@storybook/addon-essentials",
    "@storybook/addon-highlight",
    "@storybook/addon-interactions",
    "@storybook/addon-links",
    "@storybook/addon-measure",
    "@storybook/addon-onboarding",
    "@storybook/addon-outline",
    "@storybook/addon-toolbars",
    "@storybook/addon-viewport",
    "@storybook/angular",
    "@storybook/blocks",
    "@storybook/builder-vite",
    "@storybook/builder-webpack5",
    "@storybook/channels",
    "@storybook/components",
    "@storybook/core-events",
    "@storybook/csf-plugin",
    "@storybook/html",
    "@storybook/html-vite",
    "@storybook/instrumenter",
    "@storybook/manager-api",
    "@storybook/nextjs",
    "@storybook/preact",
    "@storybook/preact-vite",
    "@storybook/preview-api",
    "@storybook/react",
    "@storybook/react-dom-shim",
    "@storybook/react-vite",
    "@storybook/react-webpack5",
    "@storybook/svelte",
    "@storybook/svelte-vite",
    "@storybook/sveltekit",
    "@storybook/test",
    "@storybook/theming",
    "@storybook/types",
    "@storybook/vue3",
    "@storybook/vue3-vite",
    "@storybook/web-components",
    "@storybook/web-components-vite",
];

/// The families that ship with skif: packages whose releases are versioned in lockstep. Configuration can disable
/// one by name (`enabled = false`) or replace its members by declaring a family of the same name.
const BUILTIN: &[(&str, &[&str])] = &[
    ("react", &["react", "react-dom", "react-test-renderer"]),
    ("trpc", &["@trpc/*"]),
    ("next", &["next", "@next/*", "eslint-config-next"]),
    ("storybook", STORYBOOK),
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
/// Versions are compared by number only, so `^18.3.1` and `~18.3.1` are in sync, and a shorthand such as `^18` counts
/// as the first release of its line. Production, development and optional dependencies take part; peer ranges state
/// a compatibility window and are left alone. A `catalog:` reference is compared by the version of its catalog entry;
/// specifiers without a version number (`workspace:*`, tags, `latest`), aliases of another package, and 0.x members
/// of a family that has reached 1.0 take no part.
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
        // A `catalog:` reference counts as the version of its catalog entry.
        let resolved = |entry: &DependencyEntry| -> Option<Arc<Specifier>> {
            if entry.parsed.is_catalog() {
                Some(Specifier::new(catalogs.resolve(&entry.name, &entry.specifier)?))
            } else {
                Some(Arc::clone(&entry.parsed))
            }
        };
        for package in active_packages(cx.workspace) {
            for family in &families {
                let members: Vec<_> = package
                    .dependencies
                    .iter()
                    .filter(|entry| entry.kind != DependencyKind::PeerDependencies)
                    .filter(|entry| family.members.matches(&entry.name, false))
                    .filter_map(|entry| {
                        let specifier = resolved(entry)?;
                        if is_floating(&specifier) || alias_target(&specifier, &entry.name).is_some() {
                            return None;
                        }
                        Some((entry, major(&specifier)?, floor_version(&specifier)?))
                    })
                    .collect();
                let Some((leader, leader_major, highest)) = members.iter().max_by(|a, b| a.2.cmp(&b.2)) else {
                    continue;
                };
                for (entry, entry_major, version) in &members {
                    // A 0.x package next to a released family is on a version line of its own.
                    if version >= highest || (*leader_major >= 1 && *entry_major == 0) {
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
                        .package_id(package.id)
                        .dependency(entry.name.clone())
                        .specifier(entry.specifier.clone())
                        .help(format!(
                            "align `{}` with the other `{}` packages",
                            entry.name, family.name
                        ))
                        .build(),
                    );
                }
            }
        }
    }
}

/// Whether the specifier names a moving target (`latest`, `*`, a dist-tag) rather than a version.
fn is_floating(specifier: &Specifier) -> bool {
    matches!(specifier, Specifier::Latest(_) | Specifier::Tag(_))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testing::{config, lint};

    const ROOT: &str = r#"{"name":"monorepo","version":"0.0.0","private":true,"packageManager":"pnpm@9.0.0","workspaces":["packages/*"]}"#;

    /// The dependencies reported for a package declaring `sections` (the members of its manifest after `private`).
    fn flagged(toml: &str, sections: &str) -> Vec<String> {
        let app = format!(r#"{{"name":"app","version":"1.0.0","private":true,{sections}}}"#);
        let files = [("package.json", ROOT), ("packages/app/package.json", app.as_str())];
        lint(META.id, &files, &config(toml))
            .into_iter()
            .filter_map(|diagnostic| diagnostic.dependency)
            .collect()
    }

    fn names(toml: &str) -> Vec<String> {
        let config = Config::from_toml_str(toml).unwrap();
        effective_families(&config)
            .into_iter()
            .map(|family| family.name)
            .collect()
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

    #[test]
    fn peer_ranges_are_not_compared_with_installed_versions() {
        let sections =
            r#""peerDependencies":{"react":">=17"},"devDependencies":{"react":"^18.3.1","react-dom":"^18.3.1"}"#;
        assert_eq!(flagged("", sections), Vec::<String>::new());
        let installed = r#""dependencies":{"react":"^17.0.0"},"devDependencies":{"react-dom":"^18.3.1"}"#;
        assert_eq!(flagged("", installed), ["react"]);
    }

    #[test]
    fn storybook_packages_on_their_own_version_line_are_not_members() {
        let lockstep = r#""devDependencies":{"storybook":"^8.4.0","@storybook/react":"^8.3.0","@storybook/csf":"^0.1.11","@storybook/test-runner":"^0.19.0","@storybook/addon-styling":"^1.3.7"}"#;
        assert_eq!(flagged("", lockstep), ["@storybook/react"]);
    }

    #[test]
    fn a_zero_x_member_next_to_a_released_family_is_skipped() {
        let toml = "[[dependency-families]]\nname = \"babel\"\npackages = [\"@babel/*\"]\n";
        let mixed = r#""devDependencies":{"@babel/core":"^7.24.0","@babel/experimental":"^0.5.0","@babel/preset-env":"^7.23.0"}"#;
        assert_eq!(flagged(toml, mixed), ["@babel/preset-env"]);
        let all_zero = r#""devDependencies":{"@babel/a":"^0.5.0","@babel/b":"^0.4.0"}"#;
        assert_eq!(flagged(toml, all_zero), ["@babel/b"]);
    }

    #[test]
    fn a_shorthand_ranks_at_the_first_release_of_its_line() {
        let sections = r#""dependencies":{"react":"^18","react-dom":"^18.3.1"}"#;
        assert_eq!(flagged("", sections), ["react"]);
        let sections = r#""dependencies":{"react":"^18","react-dom":"^18.0.0"}"#;
        assert_eq!(flagged("", sections), Vec::<String>::new());
    }

    #[test]
    fn floating_specifiers_and_aliases_take_no_part() {
        let sections =
            r#""dependencies":{"react":"latest","react-dom":"^18.3.1","react-test-renderer":"npm:other@^19.0.0"}"#;
        assert_eq!(flagged("", sections), Vec::<String>::new());
    }
}
