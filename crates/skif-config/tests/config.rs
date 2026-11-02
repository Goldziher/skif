#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use skif_config::{
    Catalog, CatalogMode, Config, ConfigBuilder, ConfigError, ConfigOverrides, IgnoreEntry, Level, PackageManager,
    PluginConfig, Prefer, RuleConfig, Selector, SemverRange, VersionPolicy, discover, discover_with_fs,
};
use skif_core::{MemoryFs, Severity};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join(name)
}

fn full() -> Config {
    let (_, config) = discover(&fixture("full")).unwrap().unwrap();
    config
}

#[test]
fn discovers_basic_fixture() {
    let (path, config) = discover(&fixture("basic")).unwrap().unwrap();
    assert_eq!(path, fixture("basic").join("skif.toml"));
    assert!(config.lint.fail_on_warnings);
    assert_eq!(config.rule("unused-exports"), Some(&RuleConfig::level(Level::Off)));
}

#[test]
fn discovers_nothing_in_an_empty_directory() {
    let empty = tempfile::tempdir().unwrap();
    assert!(empty.path().is_dir());
    assert!(discover(empty.path()).unwrap().is_none());
}

#[test]
fn discovery_finds_a_config_that_was_not_there_before() {
    let dir = tempfile::tempdir().unwrap();
    assert!(discover(dir.path()).unwrap().is_none());
    std::fs::write(dir.path().join("skif.toml"), "[lint]\nfail-on-warnings = true\n").unwrap();
    let (path, config) = discover(dir.path()).unwrap().unwrap();
    assert_eq!(path, dir.path().join("skif.toml"));
    assert!(config.lint.fail_on_warnings);
}

#[test]
fn invalid_selectors_and_dependency_types_are_parse_errors_naming_the_value() {
    let cases = [
        ("[workspace]\nignore-packages = [\"a[\"]\n", "\"a[\""),
        ("[[ignore]]\npackages = [\"{x\"]\n", "\"{x\""),
        ("[[version-groups]]\ndependencies = [\"b[\"]\n", "\"b[\""),
        ("[[semver-groups]]\npackages = [\"c[\"]\nrange = \"^\"\n", "\"c[\""),
        ("[policy]\ndependency-types = [\"bundled\"]\n", "\"bundled\""),
    ];
    for (toml, needle) in cases {
        let error = Config::from_toml_str(toml).unwrap_err();
        assert!(matches!(error, ConfigError::Parse { .. }), "{toml}: {error:?}");
        assert!(error.to_string().contains(needle), "{toml}: {error}");
    }
}

#[test]
fn invalid_cli_ignore_patterns_leave_the_config_untouched() {
    let mut config = full();
    let overrides = ConfigOverrides {
        rules: [("version-mismatch".to_string(), Level::Error)].into_iter().collect(),
        ignore_packages: vec!["bad[".into()],
        ..ConfigOverrides::default()
    };
    let error = config.apply(&overrides).unwrap_err();
    assert!(error.to_string().contains("\"bad[\""), "{error}");
    assert_eq!(config, full());
}

#[test]
fn selectors_in_the_config_are_compiled() {
    let config = full();
    assert!(config.workspace.ignore_packages.matches("legacy-core", false));
    assert!(!config.workspace.ignore_packages.matches("core", false));
    let group = &config.version_groups[0];
    assert!(group.packages.matches("@scope/app", true) && !group.packages.matches("@scope/legacy", true));
    assert!(
        group
            .dependency_types
            .matches(skif_config::DependencyKind::Dependencies)
    );
    assert!(
        !group
            .dependency_types
            .matches(skif_config::DependencyKind::PeerDependencies)
    );
}

#[test]
fn discovers_dotfile() {
    let (path, config) = discover(&fixture("invalid-rule")).unwrap().unwrap();
    assert_eq!(path, fixture("invalid-rule").join(".skif.toml"));
    assert_eq!(config.rules.len(), 1);
}

#[test]
fn both_files_is_ambiguous() {
    let err = discover(&fixture("both-files")).unwrap_err();
    assert!(matches!(err, ConfigError::Ambiguous { .. }));
    let message = err.to_string();
    assert!(message.contains("skif.toml"), "{message}");
    assert!(message.contains(".skif.toml"), "{message}");
    assert!(message.contains("keep only one"), "{message}");
}

#[test]
fn unknown_key_reports_path_and_line() {
    let err = discover(&fixture("unknown-key")).unwrap_err();
    let ConfigError::Parse { line, column, .. } = &err else {
        panic!("expected parse error, got {err:?}")
    };
    assert_eq!(*line, Some(3));
    assert_eq!(*column, Some(1));
    let message = err.to_string();
    assert!(message.contains("skif.toml:3:1"), "{message}");
    assert!(message.contains("fail-on-warning"), "{message}");
}

#[test]
fn syntax_error_without_path_reports_line() {
    let err = Config::from_toml_str("[lint]\nfail-on-warnings = \n").unwrap_err();
    let ConfigError::Parse { path, line, .. } = &err else {
        panic!("expected parse error")
    };
    assert_eq!(*path, None);
    assert_eq!(*line, Some(2));
}

#[test]
fn unknown_top_level_section_fails() {
    assert!(matches!(
        Config::from_toml_str("[lnt]\n"),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn invalid_rule_suggests_closest_id() {
    let (_, config) = discover(&fixture("invalid-rule")).unwrap().unwrap();
    let err = config
        .validate_rules(&["unused-exports", "unused-files", "version-mismatch"])
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "unknown rule \"unused-exportz\"; did you mean \"unused-exports\"?"
    );
}

#[test]
fn unknown_rule_far_from_everything_has_no_suggestion() {
    let config = Config::from_toml_str("[rules]\nzzzzzzzzzz = \"warn\"\n").unwrap();
    let err = config.validate_rules(&["unused-exports"]).unwrap_err();
    assert_eq!(err.to_string(), "unknown rule \"zzzzzzzzzz\"");
}

#[test]
fn ignore_entries_are_validated_against_known_rules() {
    let config = Config::from_toml_str("[[ignore]]\nrules = [\"version-mismtach\"]\n").unwrap();
    let err = config.validate_rules(&["version-mismatch"]).unwrap_err();
    assert!(err.to_string().contains("did you mean \"version-mismatch\""), "{err}");
}

#[test]
fn known_rules_pass_validation() {
    full()
        .validate_rules(&[
            "version-mismatch",
            "unused-files",
            "member-toolchain-fields",
            "unused-exports",
            "unlisted-dependencies",
            "unused-dependencies",
        ])
        .unwrap();
}

#[test]
fn rule_levels() {
    let cases = [
        ("off", Level::Off, None),
        ("info", Level::Info, Some(Severity::Info)),
        ("warn", Level::Warn, Some(Severity::Warning)),
        ("error", Level::Error, Some(Severity::Error)),
    ];
    for (text, level, severity) in cases {
        let config = Config::from_toml_str(&format!("[rules]\nversion-mismatch = \"{text}\"\n")).unwrap();
        let rule = config.rule("version-mismatch").unwrap();
        assert_eq!(rule.severity, Some(level), "{text}");
        assert_eq!(level.severity(), severity, "{text}");
        assert!(rule.options.is_empty());
    }
}

#[test]
fn bad_rule_levels_fail_with_a_message() {
    let cases = [
        ("version-mismatch = \"fatal\"", "invalid level \"fatal\""),
        ("version-mismatch = 3", "expected a level string or a table"),
        ("version-mismatch = { severity = 1 }", "severity must be a string"),
        (
            "version-mismatch = { severity = \"warn\", level = \"warn\" }",
            "not both",
        ),
    ];
    for (line, expected) in cases {
        let err = Config::from_toml_str(&format!("[rules]\n{line}\n")).unwrap_err();
        let message = err.to_string();
        assert!(matches!(err, ConfigError::Parse { .. }), "{line}: {message}");
        assert!(message.contains(expected), "{line}: {message}");
    }
}

#[test]
fn rule_tables_keep_raw_options() {
    let config = full();
    let toolchain = config.rule("member-toolchain-fields").unwrap();
    assert_eq!(toolchain.severity, Some(Level::Error));
    assert_eq!(toolchain.options.len(), 1);
    assert_eq!(
        toolchain.options.get("mode"),
        Some(&toml::Value::String("match-root".into()))
    );
    let exports = config.rule("unused-exports").unwrap();
    assert_eq!(exports.severity, Some(Level::Warn));
    assert_eq!(
        exports.options.get("ignore-tags"),
        Some(&toml::Value::Array(vec!["public".into(), "internal".into()]))
    );
    assert_eq!(
        config.rule("unlisted-dependencies"),
        Some(&RuleConfig::level(Level::Error))
    );
}

#[test]
fn unknown_option_keys_are_not_rejected_by_config() {
    let config = Config::from_toml_str("[rules]\nsome-rule = { severity = \"info\", depth = 2 }\n").unwrap();
    let rule = config.rule("some-rule").unwrap();
    assert_eq!(rule.severity, Some(Level::Info));
    assert_eq!(rule.options.get("depth"), Some(&toml::Value::Integer(2)));
}

#[test]
fn rule_config_round_trips_through_toml() {
    let config = Config::from_toml_str("[rules]\na = \"info\"\nb = { severity = \"warn\", depth = 2 }\n").unwrap();
    let text = toml::to_string(&config).unwrap();
    assert_eq!(Config::from_toml_str(&text).unwrap(), config);
}

#[test]
fn invalid_level_lists_info() {
    let err = Config::from_toml_str("[rules]\na = \"fatal\"\n").unwrap_err();
    assert!(err.to_string().contains("\"info\""), "{err}");
}

#[test]
fn full_fixture_sections() {
    let config = full();
    assert_eq!(config.workspace.ignore_packages, ["legacy-*", "@scope/tmp"]);
    assert_eq!(config.workspace.package_manager, Some(PackageManager::Pnpm));
    assert!(config.lint.fail_on_warnings);
    assert_eq!(config.rules.keys().next().map(String::as_str), Some("version-mismatch"));

    assert_eq!(
        config.ignore,
        [IgnoreEntry {
            rules: vec!["unused-dependencies".into()],
            packages: Selector::new(["@scope/docs"]).unwrap(),
            dependencies: Selector::new(["typescript"]).unwrap(),
        }]
    );
    assert_eq!(config.policy.prefer, Prefer::Lowest);
    assert_eq!(config.policy.dependency_types, ["prod", "dev", "peer"]);

    let groups = &config.version_groups;
    assert_eq!(groups.len(), 4);
    assert_eq!(groups[0].label.as_deref(), Some("react is pinned"));
    assert_eq!(groups[0].packages, ["!@scope/legacy"]);
    assert_eq!(groups[0].dependency_types, ["prod", "!peer"]);
    assert_eq!(groups[0].specifier_types, ["exact", "range"]);
    assert_eq!(
        (groups[0].policy, groups[0].pin.as_deref()),
        (VersionPolicy::Pinned, Some("18.3.1"))
    );
    assert_eq!(groups[1].policy, VersionPolicy::Banned);
    assert_eq!(
        (groups[2].policy, groups[2].snap_to.as_slice()),
        (VersionPolicy::SnappedTo, ["@scope/core".to_string()].as_slice())
    );
    assert_eq!(
        (groups[3].policy, groups[3].prefer),
        (VersionPolicy::PreferredSemver, Some(Prefer::Lowest))
    );

    assert_eq!(config.semver_groups.len(), 2);
    assert_eq!(config.semver_groups[0].range, SemverRange::Caret);
    assert_eq!(config.semver_groups[1].range, SemverRange::Exact);

    assert_eq!(
        config.catalog,
        Catalog {
            mode: CatalogMode::Prefer,
            min_packages: 3,
            exclude: vec!["react".into()],
            report_unused_entries: true,
        }
    );
    assert!(config.workspace_protocol.internal && config.workspace_protocol.allow_local_paths);

    assert_eq!(config.dependency_families.len(), 2);
    assert!(config.dependency_families[0].enabled);
    assert!(!config.dependency_families[1].enabled);

    let js = &config.js;
    assert!(js.enabled && js.production && js.include_entry_exports);
    assert_eq!(js.entry, ["src/index.ts"]);
    assert_eq!(js.project, ["src/**/*.ts"]);
    assert_eq!(js.ignore, ["**/generated/**"]);
    assert_eq!(js.ignore_dependencies, ["@types/*"]);
    assert_eq!(js.ignore_binaries, ["tsc"]);
    assert_eq!(js.workspaces["packages/app"].entry, ["src/main.tsx"]);
    assert_eq!(js.workspaces["packages/app"].ignore_binaries, ["vite"]);
    assert_eq!(js.plugins["vitest"], PluginConfig::Enabled(true));
    assert_eq!(js.plugins["storybook"], PluginConfig::Enabled(false));
    let PluginConfig::Options(eslint) = &js.plugins["eslint"] else {
        panic!("expected plugin table")
    };
    assert_eq!(eslint.enabled, Some(true));
    assert_eq!(eslint.config, ["eslint.config.mjs"]);
    assert_eq!(eslint.entry, ["lint/*.js"]);
}

#[test]
fn full_fixture_round_trips() {
    let config = full();
    let text = config.to_toml_string().unwrap();
    assert_eq!(Config::from_toml_str(&text).unwrap(), config, "{text}");
}

#[test]
fn defaults_are_sensible() {
    let config = Config::default();
    assert_eq!(Config::from_toml_str("").unwrap(), config);
    assert!(config.js.enabled && !config.js.production);
    assert_eq!(config.catalog.mode, CatalogMode::Off);
    assert_eq!(config.catalog.min_packages, 2);
    assert_eq!(config.policy.prefer, Prefer::Highest);
    assert!(!config.lint.fail_on_warnings && config.rules.is_empty());
}

#[test]
fn inconsistent_groups_are_rejected() {
    let cases = [
        (
            "policy = \"pinned\"",
            "version-groups[0]: policy `pinned` requires `pin`",
        ),
        (
            "policy = \"snapped-to\"",
            "version-groups[0]: policy `snapped-to` requires `snap-to`",
        ),
        (
            "policy = \"banned\"\npin = \"1.0.0\"",
            "`pin` is only valid with policy `pinned`",
        ),
        (
            "policy = \"banned\"\nprefer = \"lowest\"",
            "`prefer` is only valid with policy `preferred-semver`",
        ),
        (
            "policy = \"banned\"\nsnap-to = [\"a\"]",
            "`snap-to` is only valid with policy `snapped-to`",
        ),
    ];
    for (body, expected) in cases {
        let err = Config::from_toml_str(&format!("[[version-groups]]\n{body}\n")).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { .. }), "{body}");
        assert!(err.to_string().contains(expected), "{body}: {err}");
    }
}

#[test]
fn invalid_catalog_and_families_are_rejected() {
    assert!(Config::from_toml_str("[catalog]\nmin-packages = 0\n").is_err());
    assert!(Config::from_toml_str("[[dependency-families]]\nname = \"x\"\n").is_err());
}

#[test]
fn file_errors_carry_the_path() {
    let err = Config::from_toml_file(&fixture("basic").join("nope.toml")).unwrap_err();
    assert!(matches!(err, ConfigError::Io { .. }));
    assert!(err.to_string().contains("nope.toml"));
}

#[test]
fn cli_overrides_win_over_file_and_defaults() {
    let mut config = full();
    let overrides = ConfigOverrides {
        rules: [
            ("version-mismatch".to_string(), Level::Error),
            ("brand-new".to_string(), Level::Warn),
        ]
        .into_iter()
        .collect(),
        ignore_rules: vec!["unused-exports".into()],
        ignore_packages: vec!["cli-only".into()],
        ignore_dependencies: vec!["left-pad".into()],
        fail_on_warnings: Some(false),
        production: Some(false),
        no_js: true,
    };
    config.apply(&overrides).unwrap();

    assert_eq!(config.rule("version-mismatch").unwrap().severity, Some(Level::Error));
    assert_eq!(config.rule("brand-new").unwrap().severity, Some(Level::Warn));
    let exports = config.rule("unused-exports").unwrap();
    assert_eq!(exports.severity, Some(Level::Off));
    assert!(!exports.options.is_empty(), "options survive a level override");
    assert_eq!(config.workspace.ignore_packages, ["cli-only"]);
    assert_eq!(config.ignore.len(), 2);
    assert_eq!(config.ignore[1].dependencies, ["left-pad"]);
    assert_eq!(config.ignore[1].rules, Vec::<String>::new());
    assert!(!config.lint.fail_on_warnings);
    assert!(!config.js.production);
    assert!(!config.js.enabled);
}

#[test]
fn empty_overrides_change_nothing() {
    let mut config = full();
    config.apply(&ConfigOverrides::default()).unwrap();
    assert_eq!(config, full());
}

#[test]
fn builder_matches_equivalent_toml() {
    let built = ConfigBuilder::new()
        .fail_on_warnings(true)
        .rule_level("unused-exports", Level::Off)
        .build()
        .unwrap();
    let (_, parsed) = discover(&fixture("basic")).unwrap().unwrap();
    assert_eq!(built, parsed);
}

#[test]
fn builder_validates() {
    let group = skif_config::VersionGroup {
        policy: VersionPolicy::Pinned,
        ..Default::default()
    };
    assert!(ConfigBuilder::new().version_group(group).build().is_err());
}

#[cfg(feature = "schema")]
#[test]
fn schema_lists_every_section() {
    let schema = skif_config::json_schema();
    let properties = schema["properties"].as_object().unwrap();
    for key in [
        "workspace",
        "lint",
        "rules",
        "ignore",
        "policy",
        "version-groups",
        "semver-groups",
        "catalog",
        "workspace-protocol",
        "dependency-families",
        "js",
    ] {
        assert!(properties.contains_key(key), "{key}");
    }
}

#[cfg(feature = "schema")]
#[test]
fn schema_level_enum_includes_info() {
    let schema = skif_config::json_schema().to_string();
    assert!(schema.contains("\"info\""), "{schema}");
}

#[test]
fn unknown_specifier_types_are_rejected_with_a_suggestion() {
    let cases = [
        (
            "[[version-groups]]\nspecifier-types = [\"rnge\"]\n",
            "version-groups[0]",
            "did you mean \"range\"",
        ),
        (
            "[[semver-groups]]\nrange = \"^\"\nspecifier-types = [\"exact\", \"!gitt\"]\n",
            "semver-groups[0]",
            "did you mean \"git\"",
        ),
        (
            "[[version-groups]]\nspecifier-types = [\"banana\"]\n",
            "version-groups[0]",
            "unknown specifier type \"banana\"",
        ),
    ];
    for (toml, location, expected) in cases {
        let err = Config::from_toml_str(toml).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { .. }), "{toml}: {err}");
        let message = err.to_string();
        assert!(
            message.contains(location) && message.contains(expected),
            "{toml}: {message}"
        );
    }
}

#[test]
fn every_specifier_identifier_is_accepted_with_and_without_negation() {
    for identifier in skif_specifier::CONFIG_IDENTIFIERS {
        let toml = format!("[[version-groups]]\nspecifier-types = [\"{identifier}\", \"!{identifier}\"]\n");
        Config::from_toml_str(&toml).unwrap_or_else(|error| panic!("{identifier}: {error}"));
    }
}

#[test]
fn ignore_entries_need_a_filter() {
    let err = Config::from_toml_str("[[ignore]]\n").unwrap_err();
    assert!(matches!(err, ConfigError::Invalid { .. }), "{err}");
    assert!(err.to_string().contains("ignore[0]"), "{err}");
    let err =
        Config::from_toml_str("[[ignore]]\nrules = [\"a\"]\n\n[[ignore]]\nrules = []\npackages = []\n").unwrap_err();
    assert!(err.to_string().contains("ignore[1]"), "{err}");
    for filter in ["rules = [\"a\"]", "packages = [\"p\"]", "dependencies = [\"d\"]"] {
        Config::from_toml_str(&format!("[[ignore]]\n{filter}\n")).unwrap();
    }
}

#[test]
fn level_is_the_canonical_key_and_severity_an_alias() {
    let level = Config::from_toml_str("[rules]\na = { level = \"warn\", depth = 2 }\n").unwrap();
    let alias = Config::from_toml_str("[rules]\na = { severity = \"warn\", depth = 2 }\n").unwrap();
    assert_eq!(level, alias);
    assert_eq!(level.rule("a").unwrap().severity, Some(Level::Warn));
    assert!(!level.rule("a").unwrap().options.contains_key("level"));
    let written = level.to_toml_string().unwrap();
    assert!(
        written.contains("level = \"warn\"") && !written.contains("severity"),
        "{written}"
    );
    assert_eq!(Config::from_toml_str(&written).unwrap(), level);
}

#[test]
fn a_non_string_level_names_the_key_that_was_used() {
    for key in ["level", "severity"] {
        let err = Config::from_toml_str(&format!("[rules]\na = {{ {key} = 3 }}\n")).unwrap_err();
        assert!(err.to_string().contains(&format!("{key} must be a string")), "{err}");
    }
}

#[test]
fn discovery_goes_through_the_given_file_system() {
    let fs = MemoryFs::new().with_file("/ws/.skif.toml", "[lint]\nfail-on-warnings = true\n");
    let (path, config) = discover_with_fs(Path::new("/ws"), &fs).unwrap().unwrap();
    assert_eq!(path, Path::new("/ws/.skif.toml"));
    assert!(config.lint.fail_on_warnings);
    assert!(discover_with_fs(Path::new("/elsewhere"), &fs).unwrap().is_none());

    let both = fs.with_file("/ws/skif.toml", "");
    assert!(matches!(
        discover_with_fs(Path::new("/ws"), &both),
        Err(ConfigError::Ambiguous { .. })
    ));
    let err = Config::from_toml_file_with_fs(Path::new("/ws/missing.toml"), &both).unwrap_err();
    assert!(matches!(err, ConfigError::Io { .. }), "{err}");
}

#[test]
fn error_columns_count_characters_after_multibyte_text() {
    let err = Config::from_toml_str("[lint]\n# héllo wörld\nfail-on-warning = true\n").unwrap_err();
    let ConfigError::Parse { line, column, .. } = err else {
        panic!("expected a parse error")
    };
    assert_eq!((line, column), (Some(3), Some(1)));
    let err = Config::from_toml_str("[lint]\nfail-on-warnings = \"é\" x\n").unwrap_err();
    let ConfigError::Parse { line, column, span, .. } = err else {
        panic!("expected a parse error")
    };
    assert_eq!(line, Some(2));
    let start = span.unwrap().start;
    let expected = "[lint]\nfail-on-warnings = \"é\" x\n"[..start]
        .rsplit('\n')
        .next()
        .unwrap()
        .chars()
        .count()
        + 1;
    assert_eq!(column, Some(expected));
}
