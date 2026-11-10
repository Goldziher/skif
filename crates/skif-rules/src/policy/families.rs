//! `unsynced-dependency-family`: packages released together must be declared on one version within a package.

use std::collections::HashMap;
use std::sync::Arc;

use skif_config::{Config, Selector};
use skif_core::{Category, DependencyKind, Fixability, RuleMeta, Severity, diagnostic};
use skif_lint::{Emitter, LintContext, Rule};
use skif_specifier::Specifier;
use skif_workspace::DependencyEntry;

use super::catalog::support::{active_packages, value_location};
use super::specifiers::{alias_target, floor_version, major};

/// Metadata of `unsynced-dependency-family`.
pub static META: RuleMeta = RuleMeta::new(
    "unsynced-dependency-family",
    Category::Policy,
    Some(Severity::Error),
    Fixability::None,
    "Members of a dependency family declared in one package are on different versions.",
)
.version_scoped();

/// The Storybook packages released in lockstep with `storybook`. `@storybook/*` also holds packages on their own
/// version line (`@storybook/csf`, `@storybook/test-runner`, `@storybook/addon-styling`), so the family is a list.
const STORYBOOK: &[&str] = &[
    "@storybook/addon-a11y",
    "@storybook/addon-actions",
    "@storybook/addon-backgrounds",
    "@storybook/addon-controls",
    "@storybook/addon-docs",
    "@storybook/addon-essentials",
    "@storybook/addon-highlight",
    "@storybook/addon-interactions",
    "@storybook/addon-jest",
    "@storybook/addon-links",
    "@storybook/addon-mdx-gfm",
    "@storybook/addon-measure",
    "@storybook/addon-onboarding",
    "@storybook/addon-ondevice-actions",
    "@storybook/addon-ondevice-backgrounds",
    "@storybook/addon-ondevice-controls",
    "@storybook/addon-ondevice-notes",
    "@storybook/addon-outline",
    "@storybook/addon-storysource",
    "@storybook/addon-themes",
    "@storybook/addon-toolbars",
    "@storybook/addon-viewport",
    "@storybook/angular",
    "@storybook/blocks",
    "@storybook/builder-manager",
    "@storybook/builder-vite",
    "@storybook/builder-webpack5",
    "@storybook/channels",
    "@storybook/cli",
    "@storybook/client-logger",
    "@storybook/codemod",
    "@storybook/components",
    "@storybook/core",
    "@storybook/core-common",
    "@storybook/core-events",
    "@storybook/core-server",
    "@storybook/core-webpack",
    "@storybook/csf-plugin",
    "@storybook/csf-tools",
    "@storybook/docs-tools",
    "@storybook/ember",
    "@storybook/experimental-addon-test",
    "@storybook/experimental-nextjs-vite",
    "@storybook/html",
    "@storybook/html-vite",
    "@storybook/html-webpack5",
    "@storybook/instrumenter",
    "@storybook/manager",
    "@storybook/manager-api",
    "@storybook/nextjs",
    "@storybook/node-logger",
    "@storybook/preact",
    "@storybook/preact-vite",
    "@storybook/preact-webpack5",
    "@storybook/preset-create-react-app",
    "@storybook/preset-html-webpack",
    "@storybook/preset-preact-webpack",
    "@storybook/preset-react-webpack",
    "@storybook/preset-server-webpack",
    "@storybook/preset-svelte-webpack",
    "@storybook/preview",
    "@storybook/preview-api",
    "@storybook/react",
    "@storybook/react-dom-shim",
    "@storybook/react-native",
    "@storybook/react-native-theming",
    "@storybook/react-native-ui",
    "@storybook/react-native-web-vite",
    "@storybook/react-vite",
    "@storybook/react-webpack5",
    "@storybook/router",
    "@storybook/server",
    "@storybook/server-webpack5",
    "@storybook/source-loader",
    "@storybook/svelte",
    "@storybook/svelte-vite",
    "@storybook/svelte-webpack5",
    "@storybook/sveltekit",
    "@storybook/telemetry",
    "@storybook/test",
    "@storybook/theming",
    "@storybook/types",
    "@storybook/vue3",
    "@storybook/vue3-vite",
    "@storybook/web-components",
    "@storybook/web-components-vite",
    "@storybook/web-components-webpack5",
    "create-storybook",
    "sb",
    "storybook",
];

/// The families that ship with skif: packages whose releases are versioned in lockstep. Configuration can disable
/// one by name (`enabled = false`) or replace its members by declaring a family of the same name.
const BUILTIN: &[(&str, &[&str])] = &[
    ("react", &["react", "react-dom", "react-test-renderer"]),
    ("trpc", &["@trpc/*"]),
    ("next", &["next", "@next/*", "eslint-config-next"]),
    ("storybook", STORYBOOK),
    (
        "prisma",
        &[
            "prisma",
            "@prisma/client",
            "@prisma/instrumentation",
            "@prisma/adapter-*",
            "@prisma/pg-worker",
        ],
    ),
    ("typescript-eslint", &["typescript-eslint", "@typescript-eslint/*"]),
    ("playwright", &["playwright", "playwright-core", "@playwright/test"]),
    (
        "turborepo",
        &[
            "turbo",
            "turbo-ignore",
            "@turbo/gen",
            "eslint-config-turbo",
            "eslint-plugin-turbo",
        ],
    ),
    ("eslint-stylistic", &["@stylistic/eslint-plugin*"]),
    ("lexical", &["lexical", "@lexical/*"]),
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
/// as the first release of its line. Production, development and optional dependencies take part, or those of them
/// `[policy] dependency-types` names when it is set; peer ranges state a compatibility window and are left alone. A
/// dependency listed in two sections of a package is not compared with itself, only with the other members. A
/// `catalog:` reference is compared by the version of its catalog entry;
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
        let scope = &cx.config.policy.dependency_types;
        let catalogs = &cx.workspace.catalogs;
        // A `catalog:` reference counts as the version of its catalog entry.
        let resolved = |entry: &DependencyEntry| -> Option<Arc<Specifier>> {
            if entry.parsed.is_catalog() {
                Some(Specifier::new(catalogs.resolve(&entry.name, &entry.specifier)?))
            } else {
                Some(Arc::clone(&entry.parsed))
            }
        };
        let member = |entry: &'_ DependencyEntry| {
            let specifier = resolved(entry)?;
            if is_floating(&specifier) || alias_target(&specifier, &entry.name).is_some() {
                return None;
            }
            Some((major(&specifier)?, floor_version(&specifier)?))
        };
        // Each name's families, matched once per name: large workspaces repeat a few hundred names thousands of times.
        let mut memberships: HashMap<&str, Vec<usize>> = HashMap::new();
        for package in active_packages(cx.workspace) {
            let mut members_of: Vec<Vec<_>> = families.iter().map(|_| Vec::new()).collect();
            for entry in package
                .dependencies
                .iter()
                .filter(|entry| {
                    entry.kind != DependencyKind::PeerDependencies && (scope.is_empty() || scope.matches_kind(entry.kind))
                })
            {
                let of = memberships.entry(entry.name.as_str()).or_insert_with(|| {
                    families
                        .iter()
                        .enumerate()
                        .filter(|(_, family)| family.members.matches(&entry.name, false))
                        .map(|(index, _)| index)
                        .collect()
                });
                if of.is_empty() {
                    continue;
                }
                let Some((entry_major, version)) = member(entry) else {
                    continue;
                };
                for &index in of.iter() {
                    members_of[index].push((entry, entry_major, version.clone()));
                }
            }
            for (family, members) in families.iter().zip(&members_of) {
                for (entry, entry_major, version) in members {
                    // The highest other member: a name in two sections is no family of its own.
                    let Some((leader, leader_major, highest)) = members
                        .iter()
                        .filter(|other| other.0.name != entry.name)
                        .max_by(|a, b| a.2.cmp(&b.2))
                    else {
                        continue;
                    };
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

    /// Every package of the family lists of sherif 1.13 (`unsync_similar_dependencies.rs`), by family.
    const SHERIF: &[(&str, &[&str])] = &[
        (
            "trpc",
            &["@trpc/client", "@trpc/server", "@trpc/next", "@trpc/react-query"],
        ),
        ("react", &["react", "react-dom"]),
        (
            "next",
            &[
                "eslint-config-next",
                "@next/eslint-plugin-next",
                "@next/font",
                "@next/bundle-analyzer",
                "@next/mdx",
                "next",
                "@next/third-parties",
            ],
        ),
        (
            "turborepo",
            &[
                "eslint-config-turbo",
                "eslint-plugin-turbo",
                "@turbo/gen",
                "turbo-ignore",
                "turbo",
            ],
        ),
        (
            "storybook",
            &[
                "sb",
                "storybook",
                "@storybook/codemod",
                "@storybook/cli",
                "@storybook/channels",
                "@storybook/addon-actions",
                "@storybook/addon-links",
                "@storybook/react",
                "@storybook/react-native",
                "@storybook/components",
                "@storybook/addon-backgrounds",
                "@storybook/addon-viewport",
                "@storybook/angular",
                "@storybook/addon-a11y",
                "@storybook/addon-jest",
                "@storybook/client-logger",
                "@storybook/node-logger",
                "@storybook/core",
                "@storybook/addon-storysource",
                "@storybook/html",
                "@storybook/core-events",
                "@storybook/svelte",
                "@storybook/ember",
                "@storybook/addon-ondevice-backgrounds",
                "@storybook/addon-ondevice-notes",
                "@storybook/preact",
                "@storybook/theming",
                "@storybook/router",
                "@storybook/addon-docs",
                "@storybook/addon-ondevice-actions",
                "@storybook/source-loader",
                "@storybook/preset-create-react-app",
                "@storybook/web-components",
                "@storybook/addon-essentials",
                "@storybook/server",
                "@storybook/addon-toolbars",
                "@storybook/addon-controls",
                "@storybook/core-common",
                "@storybook/builder-webpack5",
                "@storybook/core-server",
                "@storybook/csf-tools",
                "@storybook/addon-measure",
                "@storybook/addon-outline",
                "@storybook/addon-ondevice-controls",
                "@storybook/instrumenter",
                "@storybook/addon-interactions",
                "@storybook/docs-tools",
                "@storybook/builder-vite",
                "@storybook/telemetry",
                "@storybook/core-webpack",
                "@storybook/preset-html-webpack",
                "@storybook/preset-preact-webpack",
                "@storybook/preset-svelte-webpack",
                "@storybook/preset-react-webpack",
                "@storybook/html-webpack5",
                "@storybook/preact-webpack5",
                "@storybook/svelte-webpack5",
                "@storybook/web-components-webpack5",
                "@storybook/preset-server-webpack",
                "@storybook/react-webpack5",
                "@storybook/server-webpack5",
                "@storybook/addon-highlight",
                "@storybook/blocks",
                "@storybook/builder-manager",
                "@storybook/react-vite",
                "@storybook/svelte-vite",
                "@storybook/web-components-vite",
                "@storybook/nextjs",
                "@storybook/types",
                "@storybook/manager",
                "@storybook/csf-plugin",
                "@storybook/preview",
                "@storybook/manager-api",
                "@storybook/preview-api",
                "@storybook/html-vite",
                "@storybook/sveltekit",
                "@storybook/preact-vite",
                "@storybook/addon-mdx-gfm",
                "@storybook/react-dom-shim",
                "create-storybook",
                "@storybook/addon-onboarding",
                "@storybook/react-native-theming",
                "@storybook/addon-themes",
                "@storybook/test",
                "@storybook/react-native-ui",
                "@storybook/experimental-nextjs-vite",
                "@storybook/experimental-addon-test",
                "@storybook/react-native-web-vite",
            ],
        ),
        (
            "prisma",
            &[
                "prisma",
                "@prisma/client",
                "@prisma/instrumentation",
                "@prisma/adapter-pg",
                "@prisma/adapter-neon",
                "@prisma/adapter-planetscale",
                "@prisma/adapter-d1",
                "@prisma/adapter-libsql",
                "@prisma/adapter-pg-worker",
                "@prisma/pg-worker",
            ],
        ),
        (
            "typescript-eslint",
            &[
                "typescript-eslint",
                "@typescript-eslint/eslint-plugin",
                "@typescript-eslint/parser",
            ],
        ),
        (
            "eslint-stylistic",
            &[
                "@stylistic/eslint-plugin-js",
                "@stylistic/eslint-plugin-ts",
                "@stylistic/eslint-plugin-migrate",
                "@stylistic/eslint-plugin",
                "@stylistic/eslint-plugin-jsx",
                "@stylistic/eslint-plugin-plus",
            ],
        ),
        ("playwright", &["playwright", "@playwright/test"]),
        (
            "lexical",
            &[
                "lexical",
                "@lexical/clipboard",
                "@lexical/code",
                "@lexical/devtools-core",
                "@lexical/dragon",
                "@lexical/eslint-plugin",
                "@lexical/file",
                "@lexical/hashtag",
                "@lexical/headless",
                "@lexical/history",
                "@lexical/html",
                "@lexical/link",
                "@lexical/list",
                "@lexical/mark",
                "@lexical/markdown",
                "@lexical/offset",
                "@lexical/overflow",
                "@lexical/plain-text",
                "@lexical/react",
                "@lexical/rich-text",
                "@lexical/selection",
                "@lexical/table",
                "@lexical/text",
                "@lexical/utils",
                "@lexical/yjs",
            ],
        ),
    ];

    #[test]
    fn every_family_of_sherif_is_covered_member_for_member() {
        let config = Config::from_toml_str("").unwrap();
        let families = effective_families(&config);
        for (name, members) in SHERIF {
            let family = families
                .iter()
                .find(|family| family.name == *name)
                .unwrap_or_else(|| panic!("missing family {name}"));
            for member in *members {
                assert!(
                    family.members.matches(member, false),
                    "{member} is not in family {name}"
                );
            }
        }
    }

    #[test]
    fn new_builtin_families_report_mismatches() {
        let turbo = r#""devDependencies":{"turbo":"^2.1.0","eslint-config-turbo":"^2.0.0"}"#;
        assert_eq!(flagged("", turbo), ["eslint-config-turbo"]);
        let stylistic =
            r#""devDependencies":{"@stylistic/eslint-plugin":"^2.8.0","@stylistic/eslint-plugin-js":"^2.1.0"}"#;
        assert_eq!(flagged("", stylistic), ["@stylistic/eslint-plugin-js"]);
        let lexical = r#""dependencies":{"lexical":"^0.20.0","@lexical/react":"^0.19.0"}"#;
        assert_eq!(flagged("", lexical), ["@lexical/react"]);
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
    fn a_dependency_in_two_sections_is_not_compared_with_itself() {
        let alone = r#""dependencies":{"react":"^18.0.0"},"devDependencies":{"react":"^18.2.0"}"#;
        assert_eq!(flagged("", alone), Vec::<String>::new());
        let with_member =
            r#""dependencies":{"react":"^18.0.0","react-dom":"^18.2.0"},"devDependencies":{"react":"^18.2.0"}"#;
        assert_eq!(flagged("", with_member), ["react"], "the older entry is behind react-dom");
    }

    #[test]
    fn the_policy_scope_selects_the_sections_that_take_part() {
        let sections = r#""dependencies":{"react":"^18.2.0"},"optionalDependencies":{"react-dom":"^17.0.0"}"#;
        assert_eq!(flagged("", sections), ["react-dom"]);
        assert_eq!(
            flagged("[policy]\ndependency-types = [\"prod\", \"dev\"]\n", sections),
            Vec::<String>::new()
        );
        assert_eq!(
            flagged("[policy]\ndependency-types = [\"!dev\"]\n", sections),
            ["react-dom"]
        );
    }

    #[test]
    fn floating_specifiers_and_aliases_take_no_part() {
        let sections =
            r#""dependencies":{"react":"latest","react-dom":"^18.3.1","react-test-renderer":"npm:other@^19.0.0"}"#;
        assert_eq!(flagged("", sections), Vec::<String>::new());
    }
}
