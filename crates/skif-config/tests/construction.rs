#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Every config model struct is `#[non_exhaustive]`, so this integration test (a separate crate) can only build them
//! through `Default`, constructors and `with_*` methods.

use skif_config::{
    Catalog, CatalogMode, Config, ConfigBuilder, DependencyFamily, DependencyTypes, IgnoreEntry, IgnoreItem, Js, Level,
    Lint, PackageManager, PluginConfig, PluginOptions, Policy, Prefer, Selector, SemverGroup, SemverRange,
    VersionGroup, VersionPolicy, Workspace, WorkspaceProtocol,
};

#[test]
fn builds_every_section_from_outside_the_crate() {
    let config = ConfigBuilder::new()
        .workspace(
            Workspace::default()
                .with_ignore_packages(Selector::new(["legacy-*"]).unwrap())
                .with_package_manager(PackageManager::Pnpm)
                .with_discover_ignore(vec!["fixtures".to_string()]),
        )
        .lint(
            Lint::default()
                .with_fail_on_warnings(true)
                .with_max_level(Level::Warn)
                .with_require_ignore_reasons(true),
        )
        .policy(
            Policy::default()
                .with_prefer(Prefer::Lowest)
                .with_dependency_types(DependencyTypes::new(["prod", "dev"]).unwrap()),
        )
        .ignore(
            IgnoreEntry::default()
                .with_rules(["unused-dependencies"])
                .with_packages(Selector::new(["@scope/docs"]).unwrap())
                .with_dependencies(Selector::new(["typescript"]).unwrap()),
        )
        .version_group(
            VersionGroup::default()
                .with_label("react is pinned")
                .with_dependencies(Selector::new(["react"]).unwrap())
                .with_packages(Selector::new(["!@scope/legacy"]).unwrap())
                .with_dependency_types(DependencyTypes::new(["prod"]).unwrap())
                .with_specifier_types(["exact"])
                .with_policy(VersionPolicy::Pinned)
                .with_pin("18.3.1"),
        )
        .version_group(
            VersionGroup::default()
                .with_policy(VersionPolicy::SnappedTo)
                .with_snap_to(["@scope/core"]),
        )
        .version_group(VersionGroup::default().with_prefer(Prefer::Lowest))
        .semver_group(
            SemverGroup::new(SemverRange::Caret)
                .with_label("carets")
                .with_dependencies(Selector::new(["lodash"]).unwrap())
                .with_packages(Selector::new(["@scope/*"]).unwrap())
                .with_dependency_types(DependencyTypes::new(["dev"]).unwrap())
                .with_specifier_types(["range"]),
        )
        .catalog(
            Catalog::default()
                .with_mode(CatalogMode::Prefer)
                .with_min_packages(3)
                .with_exclude(["react"])
                .with_report_unused_entries(true),
        )
        .workspace_protocol(
            WorkspaceProtocol::default()
                .with_internal(true)
                .with_allow_local_paths(true),
        )
        .dependency_family(DependencyFamily::new("react", ["react", "react-dom"]))
        .dependency_family(DependencyFamily::new("tooling", ["eslint"]).with_enabled(false))
        .js(Js::default()
            .with_enabled(true)
            .with_production(true)
            .with_entry(["src/index.ts"])
            .with_project(["src/**/*.ts"])
            .with_plugin("vitest", PluginConfig::Enabled(false))
            .with_plugin(
                "eslint",
                PluginConfig::Options(
                    PluginOptions::new()
                        .with_enabled(true)
                        .with_config(["eslint.config.js"])
                        .with_entry(["lint/**"])
                        .with_project(["lint/**"]),
                ),
            ))
        .build()
        .unwrap();

    assert_eq!(config.workspace.package_manager, Some(PackageManager::Pnpm));
    assert_eq!(config.workspace.discover_ignore, ["fixtures"]);
    assert!(config.lint.fail_on_warnings);
    assert_eq!(config.lint.max_level, Some(Level::Warn));
    assert!(config.lint.require_ignore_reasons);
    assert_eq!(config.policy.prefer, Prefer::Lowest);
    assert_eq!(config.ignore[0].rules, ["unused-dependencies"]);
    assert_eq!(config.version_groups[0].pin.as_deref(), Some("18.3.1"));
    assert_eq!(config.version_groups[0].specifier_types, ["exact"]);
    assert_eq!(config.version_groups[1].snap_to, ["@scope/core"]);
    assert_eq!(config.version_groups[2].prefer, Some(Prefer::Lowest));
    assert_eq!(config.semver_groups[0].range, Some(SemverRange::Caret));
    assert_eq!(config.semver_groups[0].label.as_deref(), Some("carets"));
    assert_eq!(config.semver_groups[0].specifier_types, ["range"]);
    assert_eq!(config.catalog.mode, CatalogMode::Prefer);
    assert_eq!(config.catalog.min_packages, 3);
    assert!(config.workspace_protocol.internal && config.workspace_protocol.allow_local_paths);
    assert_eq!(config.dependency_families[0].packages, ["react", "react-dom"]);
    assert!(config.dependency_families[0].enabled);
    assert!(!config.dependency_families[1].enabled);
    assert_eq!(config.js.entry, ["src/index.ts"]);
    assert_eq!(config.js.plugins["vitest"], PluginConfig::Enabled(false));
    let PluginConfig::Options(eslint) = &config.js.plugins["eslint"] else {
        panic!("eslint plugin should hold options");
    };
    assert_eq!(eslint.enabled, Some(true));
    assert_eq!(eslint.config, ["eslint.config.js"]);
}

#[test]
fn builder_output_survives_a_toml_round_trip() {
    let config = ConfigBuilder::new()
        .semver_group(SemverGroup::new(SemverRange::Tilde).with_dependencies(Selector::new(["lodash"]).unwrap()))
        .dependency_family(DependencyFamily::new("react", ["react", "react-dom"]))
        .build()
        .unwrap();
    let reparsed = Config::from_toml_str(&config.to_toml_string().unwrap()).unwrap();
    assert_eq!(config, reparsed);
}

#[test]
fn ignore_list_entries_are_built_from_strings_or_with_a_reason() {
    let js = Js::default().with_ignore_dependencies([
        IgnoreItem::from("left-pad"),
        IgnoreItem::new("some-loader").with_reason("loaded by the bundler from a config string"),
    ]);
    let config = ConfigBuilder::new().js(js).build().unwrap();
    assert_eq!(config.js.ignore_dependencies, ["left-pad", "some-loader"]);
    assert_eq!(config.js.ignore_dependencies[0].reason, None);
    assert_eq!(
        config.js.ignore_dependencies[1].reason.as_deref(),
        Some("loaded by the bundler from a config string")
    );
    let reparsed = Config::from_toml_str(&config.to_toml_string().unwrap()).unwrap();
    assert_eq!(config, reparsed);
}
