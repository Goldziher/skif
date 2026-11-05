#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Golden tests: one realistic synthetic configuration per source tool, translated and compared with a snapshot, and
//! parsed back as a `skif.toml`.

use std::fs;
use std::path::Path;

use skif_migrate::{Input, Migration, Note, NoteClass, Options, Profile, RuleInfo, Tool, ToolInput, migrate};

fn read(dir: &str, name: &str) -> Input {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(dir)
        .join(name);
    Input {
        path: name.to_owned(),
        text: fs::read_to_string(path).unwrap(),
    }
}

fn known_rules() -> Vec<RuleInfo> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rule-ids.txt");
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| RuleInfo {
            id: id.to_owned(),
            default_on: id != "circular-imports",
        })
        .collect()
}

fn options(profile: Profile) -> Options {
    Options {
        profile,
        known_rules: known_rules(),
        strict_hints: false,
    }
}

fn note<'m>(migration: &'m Migration, path: &str) -> &'m Note {
    migration
        .notes
        .iter()
        .find(|note| note.path == path)
        .unwrap_or_else(|| panic!("no note at {path}: {:#?}", migration.notes))
}

fn class(migration: &Migration, path: &str) -> NoteClass {
    note(migration, path).class
}

fn table(migration: &Migration) -> toml::Table {
    migration.toml.parse().unwrap()
}

#[test]
fn sherif_package_json_key_and_script_flags() {
    let mut input = ToolInput::new(Tool::Sherif);
    input.package_json = Some(read("sherif", "package.json"));
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    insta::assert_snapshot!("sherif", migration.toml);

    assert_eq!(class(&migration, "sherif.ignoreRule"), NoteClass::Exact);
    assert_eq!(class(&migration, "sherif.select"), NoteClass::Exact);
    assert_eq!(class(&migration, "sherif.fix"), NoteClass::Default);
    assert_eq!(
        class(
            &migration,
            "sherif.ignoreDependency, scripts.lint:monorepo: --ignore-dependency"
        ),
        NoteClass::Exact
    );
    assert!(
        migration
            .notes
            .iter()
            .any(|note| note.class == NoteClass::Approximate && note.message.contains("react@17.0.2")),
        "the name@version entry is reported as instance based"
    );
    assert!(!migration.needs_human());
    let ignored: Vec<&str> = migration
        .config
        .workspace
        .ignore_packages
        .patterns()
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(ignored, ["@demo/legacy", "./apps/legacy"]);
    assert_eq!(migration.config.ignore.len(), 1);
}

#[test]
fn sherif_exact_profile_turns_off_everything_sherif_did_not_check() {
    let mut input = ToolInput::new(Tool::Sherif);
    input.package_json = Some(read("sherif", "package.json"));
    let migration = migrate(&[input], &options(Profile::Exact)).unwrap();
    let parsed = table(&migration);
    let rules = parsed["rules"].as_table().unwrap();
    assert_eq!(
        rules.len(),
        31,
        "the two ignored sherif rules and the 29 other default rules"
    );
    assert!(!rules.contains_key("version-mismatch"), "the check sherif ran stays on");
    assert_eq!(rules["unused-files"].as_str(), Some("off"));
    assert_eq!(rules["unsorted-dependencies"].as_str(), Some("off"));
    assert_eq!(parsed["js"]["enabled"].as_bool(), Some(false));
    assert_eq!(migration.extra_checks.len(), 29);
    assert!(migration.extra_checks.iter().any(|id| id == "unused-files"));
}

#[test]
fn syncpack_yaml_with_custom_types_and_groups() {
    let mut input = ToolInput::new(Tool::Syncpack);
    input.config = Some(read("syncpack", ".syncpackrc.yaml"));
    input.package_json = Some(read("syncpack", "package.json"));
    input.workspace_patterns = vec!["packages/*".to_owned()];
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    insta::assert_snapshot!("syncpack", migration.toml);

    assert_eq!(class(&migration, "customTypes.engines"), NoteClass::Exact);
    assert_eq!(class(&migration, "customTypes.optional"), NoteClass::Exact);
    assert_eq!(class(&migration, "customTypes.localish"), NoteClass::Unsupported);
    assert_eq!(
        class(&migration, "versionGroups[5]"),
        NoteClass::Exact,
        "a group for the override sections is written"
    );
    assert_eq!(class(&migration, "dependencyGroups"), NoteClass::Unsupported);
    assert_eq!(class(&migration, "source"), NoteClass::Default);
    assert_eq!(class(&migration, "sortAz"), NoteClass::Approximate);
    assert_eq!(class(&migration, "updateGroups"), NoteClass::Approximate);
    assert_eq!(
        class(&migration, "scripts.lint:versions: --dependency-types"),
        NoteClass::Exact
    );
    assert_eq!(
        class(&migration, "scripts.lint:versions: --dependencies"),
        NoteClass::Human
    );
    assert!(migration.needs_human());
    assert_eq!(migration.config.version_groups.len(), 7);
    assert_eq!(migration.config.semver_groups.len(), 3);
}

#[test]
fn syncpack_exact_profile_keeps_the_syncpack_rules() {
    let mut input = ToolInput::new(Tool::Syncpack);
    input.config = Some(read("syncpack", ".syncpackrc.yaml"));
    let migration = migrate(&[input], &options(Profile::Exact)).unwrap();
    let parsed = table(&migration);
    let rules = parsed["rules"].as_table().unwrap();
    for kept in [
        "version-mismatch",
        "semver-range-mismatch",
        "banned-dependency",
        "unsorted-dependencies",
    ] {
        assert!(!rules.contains_key(kept), "{kept} is kept");
    }
    assert_eq!(rules["unused-files"].as_str(), Some("off"));
    assert_eq!(rules["empty-dependencies"].as_str(), Some("off"));
}

#[test]
fn knip_ts_with_workspaces_plugins_and_a_non_static_value() {
    let mut input = ToolInput::new(Tool::Knip);
    input.config = Some(read("knip", "knip.ts"));
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    insta::assert_snapshot!("knip", migration.toml);

    assert!(
        migration
            .notes
            .iter()
            .any(|note| note.path == "ignoreFiles" && note.class == NoteClass::Approximate),
        "ignoreFiles widens to every rule"
    );
    assert_eq!(class(&migration, "tags[1]"), NoteClass::Unsupported);
    assert_eq!(class(&migration, "compilers"), NoteClass::Unsupported);
    assert_eq!(
        class(&migration, "workspaces[\"packages/*\"].mystery"),
        NoteClass::Unsupported
    );
    assert_eq!(
        class(&migration, "workspaces[\"packages/*\"].ignoreBinaries[0]"),
        NoteClass::Human,
        "a call that is only known at run time needs a human"
    );
    assert_eq!(
        class(&migration, "workspaces[\"packages/*\"].entry[1]"),
        NoteClass::Human
    );
    assert!(migration.needs_human());
    let parsed = table(&migration);
    assert_eq!(parsed["rules"]["unlisted-dependencies"].as_str(), Some("off"));
    assert_eq!(parsed["rules"]["stale-js-config"].as_str(), Some("warn"));
    assert_eq!(
        migration.config.js.entry,
        ["src/index.ts!", "scripts/*.ts", "src/index.ts"]
    );
}

#[test]
fn knip_script_flags_narrow_the_rules() {
    let mut input = ToolInput::new(Tool::Knip);
    input.package_json = Some(read("knip-flags", "package.json"));
    let migration = migrate(&[input], &options(Profile::Exact)).unwrap();
    let parsed = table(&migration);
    let rules = parsed["rules"].as_table().unwrap();
    for enabled in [
        "unused-dependencies",
        "unused-dev-dependencies",
        "unused-optional-peer-dependencies",
        "unlisted-dependencies",
        "unresolved-imports",
        "unused-catalog-entries",
        "unresolved-catalog-references",
    ] {
        assert_eq!(rules[enabled].as_str(), Some("error"), "{enabled}");
    }
    assert_eq!(rules["stale-js-config"].as_str(), Some("warn"));
    assert!(
        !migration.extra_checks.iter().any(|id| id == "stale-js-config"),
        "a rule that is on is not an extra check that was turned off"
    );
    assert_eq!(
        rules["unlisted-binaries"].as_str(),
        Some("off"),
        "excluded with --exclude binaries"
    );
    assert_eq!(rules["unused-files"].as_str(), Some("off"));
    assert_eq!(
        rules["unparseable-files"].as_str(),
        Some("off"),
        "knip skips a file it cannot parse without reporting it"
    );
    assert!(parsed["js"].get("production").is_none());
    assert_eq!(parsed["js"]["include-entry-exports"].as_bool(), Some(true));
    assert_eq!(migration.extra_checks.len(), 30);
}

#[test]
fn migrating_all_three_keeps_the_javascript_analysis() {
    let mut sherif = ToolInput::new(Tool::Sherif);
    sherif.package_json = Some(read("sherif", "package.json"));
    let mut knip = ToolInput::new(Tool::Knip);
    knip.config = Some(read("knip", "knip.ts"));
    let migration = migrate(&[sherif, knip], &options(Profile::Exact)).unwrap();
    let parsed = table(&migration);
    assert!(parsed["js"].as_table().unwrap().get("enabled").is_none());
    assert_eq!(parsed["rules"]["unused-files"].as_str(), Some("warn"));
    assert!(migration.toml.starts_with("#:schema "));
    assert!(
        migration
            .toml
            .contains("# Migrated from sherif, knip by `skif migrate`")
    );
}

#[test]
fn merging_the_output_into_itself_changes_nothing() {
    let mut input = ToolInput::new(Tool::Syncpack);
    input.config = Some(read("syncpack", ".syncpackrc.yaml"));
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    let merged = migration.merge_into(&migration.toml).unwrap();
    assert_eq!(merged.toml, migration.toml);
    assert_eq!(merged.kept_existing, [] as [std::string::String; 0]);
}

#[test]
fn merging_keeps_the_comments_of_an_existing_file() {
    let mut input = ToolInput::new(Tool::Sherif);
    input.package_json = Some(read("sherif", "package.json"));
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    let existing = "# my settings\n[js]\nenabled = false\n";
    let merged = migration.merge_into(existing).unwrap();
    assert!(merged.toml.starts_with(existing));
    assert!(merged.toml.contains("[lint]\nfail-on-warnings = true"));
}

#[test]
fn an_unreadable_configuration_is_an_error_naming_the_file() {
    let mut input = ToolInput::new(Tool::Knip);
    input.config = Some(Input {
        path: "knip.ts".to_owned(),
        text: "export default await loadConfigFromNetwork();".to_owned(),
    });
    let migration = migrate(&[input], &options(Profile::Full));
    match migration {
        Ok(migration) => assert!(migration.needs_human(), "{:#?}", migration.notes),
        Err(error) => assert!(error.to_string().contains("knip.ts"), "{error}"),
    }
}

fn text(path: &str, text: &str) -> Input {
    Input {
        path: path.to_owned(),
        text: text.to_owned(),
    }
}

fn package(scripts: &[(&str, &str)]) -> String {
    let scripts: Vec<String> = scripts
        .iter()
        .map(|(name, body)| format!("\"{name}\": \"{body}\""))
        .collect();
    format!("{{ \"name\": \"demo\", \"scripts\": {{ {} }} }}", scripts.join(", "))
}

fn syncpack(config: &str, profile: Profile) -> Migration {
    let mut input = ToolInput::new(Tool::Syncpack);
    input.config = Some(text(".syncpackrc.json", config));
    migrate(&[input], &options(profile)).unwrap()
}

fn knip(config: Option<&str>, scripts: &[(&str, &str)]) -> Migration {
    let mut input = ToolInput::new(Tool::Knip);
    input.config = config.map(|config| text("knip.json", config));
    input.package_json = Some(text("package.json", &package(scripts)));
    migrate(&[input], &options(Profile::Exact)).unwrap()
}

fn strings(value: &toml::Value) -> Vec<&str> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap())
        .collect()
}

fn has_note(migration: &Migration, class: NoteClass, fragment: &str) -> bool {
    migration
        .notes
        .iter()
        .any(|note| note.class == class && (note.message.contains(fragment) || note.path.contains(fragment)))
}

fn sherif_input(package_json: &str) -> ToolInput {
    let mut input = ToolInput::new(Tool::Sherif);
    input.package_json = Some(text("package.json", package_json));
    input
}

#[test]
fn a_bare_sherif_run_writes_and_claims_nothing() {
    let mut input = ToolInput::new(Tool::Sherif);
    input.package_json = Some(text("package.json", &package(&[("lint", "sherif")])));
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    assert!(!migration.toml.contains("ignore-packages"), "{}", migration.toml);
    assert!(!migration.toml.contains("[workspace]"), "{}", migration.toml);
    assert_eq!(migration.notes, Vec::<Note>::new());
}

#[test]
fn a_bare_syncpack_run_writes_the_default_scope_only() {
    let mut input = ToolInput::new(Tool::Syncpack);
    input.package_json = Some(text("package.json", &package(&[("lint", "syncpack lint")])));
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    let parsed = table(&migration);
    assert_eq!(
        strings(&parsed["policy"]["dependency-types"]),
        [
            "prod",
            "dev",
            "peer",
            "local",
            "resolutions",
            "overrides",
            "pnpm-overrides"
        ]
    );
    assert_eq!(parsed.len(), 1, "{}", migration.toml);
    assert_eq!(migration.notes.len(), 2, "{:#?}", migration.notes);
}

#[test]
fn syncpack_overrides_are_read_the_way_syncpack_reads_them() {
    let migration = syncpack("{}", Profile::Full);
    assert_eq!(table(&migration)["policy"]["override-reading"].as_str(), Some("syncpack"));
    assert_eq!(
        migration.config.policy.override_reading,
        skif_config::OverrideReading::Syncpack
    );
    assert!(has_note(&migration, NoteClass::Exact, "override-reading"));

    let mut sherif = ToolInput::new(Tool::Sherif);
    sherif.package_json = Some(text("package.json", &package(&[])));
    let migration = migrate(&[sherif], &options(Profile::Full)).unwrap();
    assert!(table(&migration).get("policy").is_none(), "only syncpack reads overrides its own way");
}

#[test]
fn a_bare_knip_run_writes_no_js_setting() {
    let migration = knip(None, &[("knip", "knip")]);
    let parsed = table(&migration);
    assert!(parsed.get("js").is_none(), "{}", migration.toml);
    assert_eq!(parsed["rules"]["unused-files"].as_str(), Some("error"));
}

#[test]
fn sherif_runs_that_differ_are_combined_with_a_note() {
    let mut input = ToolInput::new(Tool::Sherif);
    input.package_json = Some(text(
        "package.json",
        &package(&[("a", "sherif -i react"), ("b", "sherif -i lodash --fail-on-warnings")]),
    ));
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    assert_eq!(
        class(&migration, "scripts.a, scripts.b: --ignore-dependency"),
        NoteClass::Approximate
    );
    assert_eq!(
        class(&migration, "scripts.a, scripts.b: --fail-on-warnings"),
        NoteClass::Approximate
    );
}

#[test]
fn a_rule_sherif_ignores_stays_on_while_syncpack_checks_it() {
    let sherif = sherif_input(r#"{ "sherif": { "ignoreRule": ["multiple-dependency-versions"] } }"#);
    let mut syncpack = ToolInput::new(Tool::Syncpack);
    syncpack.config = Some(text(
        ".syncpackrc.json",
        r#"{ "versionGroups": [{ "dependencies": ["react"], "pinVersion": "18.3.1" }] }"#,
    ));
    for inputs in [vec![sherif.clone(), syncpack.clone()], vec![syncpack, sherif]] {
        let migration = migrate(&inputs, &options(Profile::Exact)).unwrap();
        let parsed = table(&migration);
        assert!(
            parsed["rules"].get("version-mismatch").is_none(),
            "the pin is still enforced: {}",
            migration.toml
        );
        assert_eq!(class(&migration, "sherif.ignoreRule"), NoteClass::Human);
        assert!(migration.needs_human());
    }
}

#[test]
fn syncpack_double_star_and_override_sections() {
    let config = r#"{
      "versionGroups": [
        { "label": "all", "dependencyTypes": ["**"], "dependencies": ["react"], "pinVersion": "18.3.1" },
        { "label": "overrides", "dependencyTypes": ["resolutions", "overrides", "pnpmOverrides"],
          "dependencies": ["lodash"], "isBanned": true }
      ]
    }"#;
    let migration = syncpack(config, Profile::Full);
    let parsed = table(&migration);
    let groups = parsed["version-groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2, "{}", migration.toml);
    assert!(groups[0].get("dependency-types").is_none(), "`**` is every type");
    assert_eq!(
        strings(&groups[1]["dependency-types"]),
        ["resolutions", "overrides", "pnpm-overrides"]
    );
    assert!(
        !has_note(&migration, NoteClass::Unsupported, "dependencyTypes"),
        "{:#?}",
        migration.notes
    );
    assert_eq!(
        strings(&parsed["policy"]["dependency-types"]),
        [
            "prod",
            "dev",
            "peer",
            "local",
            "resolutions",
            "overrides",
            "pnpm-overrides"
        ]
    );
}

#[test]
fn syncpack_specifier_types_are_mapped_or_reported_never_an_internal_error() {
    let config = r#"{
      "versionGroups": [
        { "dependencies": ["a"], "specifierTypes": ["hosted-git", "exact"], "isIgnored": true },
        { "dependencies": ["b"], "specifierTypes": ["delete"], "isBanned": true },
        { "dependencies": ["c"], "specifierTypes": ["!delete", "!url"], "isIgnored": true },
        { "dependencies": ["d"], "specifierTypes": ["**"], "isIgnored": true }
      ],
      "semverGroups": [{ "dependencies": ["e"], "specifierTypes": ["nope"], "range": "^" }]
    }"#;
    let migration = syncpack(config, Profile::Full);
    let parsed = table(&migration);
    let groups = parsed["version-groups"].as_array().unwrap();
    assert_eq!(groups.len(), 3, "{}", migration.toml);
    assert_eq!(strings(&groups[0]["specifier-types"]), ["git", "exact"]);
    assert_eq!(strings(&groups[1]["specifier-types"]), ["!url"]);
    assert!(groups[2].get("specifier-types").is_none());
    assert_eq!(parsed.get("semver-groups"), None, "the group matched nothing");
    assert_eq!(
        class(&migration, "versionGroups[0].specifierTypes"),
        NoteClass::Approximate
    );
    assert_eq!(
        class(&migration, "versionGroups[1].specifierTypes"),
        NoteClass::Unsupported
    );
    assert_eq!(
        class(&migration, "semverGroups[0].specifierTypes"),
        NoteClass::Unsupported
    );
}

#[test]
fn a_custom_type_named_like_a_built_in_is_renamed() {
    let config = r#"{
      "customTypes": { "dev": { "path": "tooling.version", "strategy": "version" } },
      "versionGroups": [{ "dependencyTypes": ["dev"], "isIgnored": true }]
    }"#;
    let migration = syncpack(config, Profile::Full);
    let parsed = table(&migration);
    assert_eq!(parsed["custom-types"][0]["name"].as_str(), Some("dev-custom"));
    assert_eq!(
        strings(&parsed["version-groups"][0]["dependency-types"]),
        ["dev"],
        "the group keeps naming the built-in type"
    );
    assert!(strings(&parsed["policy"]["dependency-types"]).contains(&"dev-custom"));
    assert_eq!(class(&migration, "customTypes.dev"), NoteClass::Approximate);
}

#[test]
fn a_catalog_policy_group_does_not_widen_to_every_dependency() {
    let narrow = syncpack(
        r#"{ "versionGroups": [{ "dependencies": ["react"], "policy": "catalog" }] }"#,
        Profile::Full,
    );
    let parsed = table(&narrow);
    assert!(parsed.get("catalog").is_none(), "{}", narrow.toml);
    assert!(parsed.get("version-groups").is_none(), "{}", narrow.toml);
    assert_eq!(class(&narrow, "versionGroups[0].policy"), NoteClass::Human);
    assert!(narrow.notes.iter().any(|note| note.message.contains("react")));

    let global = syncpack(r#"{ "versionGroups": [{ "policy": "catalog" }] }"#, Profile::Full);
    assert_eq!(table(&global)["catalog"]["mode"].as_str(), Some("prefer"));
    assert_eq!(class(&global, "versionGroups[0].policy"), NoteClass::Exact);
}

#[test]
fn a_group_that_only_covers_a_type_skif_cannot_read_is_not_written() {
    let config = r#"{
      "customTypes": { "odd": { "path": "name", "namePath": "x", "strategy": "name~version" } },
      "versionGroups": [{ "dependencyTypes": ["odd"], "isBanned": true }]
    }"#;
    let migration = syncpack(config, Profile::Full);
    assert!(table(&migration).get("version-groups").is_none());
    assert_eq!(
        class(&migration, "versionGroups[0].dependencyTypes"),
        NoteClass::Unsupported
    );
}

#[test]
fn syncpack_runs_that_lint_different_types_are_combined_with_a_note() {
    let mut input = ToolInput::new(Tool::Syncpack);
    input.package_json = Some(text(
        "package.json",
        &package(&[
            ("a", "syncpack lint --dependency-types prod"),
            ("b", "syncpack lint --dependency-types dev"),
        ]),
    ));
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    assert_eq!(
        strings(&table(&migration)["policy"]["dependency-types"]),
        ["prod", "dev"]
    );
    assert_eq!(
        class(&migration, "scripts.a, scripts.b: --dependency-types"),
        NoteClass::Approximate
    );
}

#[test]
fn knip_dependencies_include_the_catalog_types_and_the_dev_dependencies() {
    let migration = knip(None, &[("k", "knip --dependencies")]);
    let parsed = table(&migration);
    let rules = parsed["rules"].as_table().unwrap();
    for enabled in [
        "unused-dependencies",
        "unused-dev-dependencies",
        "unused-optional-peer-dependencies",
        "unlisted-dependencies",
        "unlisted-binaries",
        "unresolved-imports",
        "unused-catalog-entries",
        "unresolved-catalog-references",
    ] {
        assert_eq!(rules[enabled].as_str(), Some("error"), "{enabled}");
    }
    assert_eq!(rules["unused-files"].as_str(), Some("off"));
}

#[test]
fn knip_namespace_only_includes_add_the_defaults() {
    let migration = knip(None, &[("k", "knip --include nsExports")]);
    let rules = table(&migration)["rules"].clone();
    assert_eq!(rules["unused-files"].as_str(), Some("error"));
    assert_eq!(rules["unused-namespace-exports"].as_str(), Some("error"));
}

#[test]
fn knip_production_leaves_out_dev_dependencies_and_the_catalog() {
    let migration = knip(None, &[("k", "knip --production")]);
    let parsed = table(&migration);
    let rules = parsed["rules"].as_table().unwrap();
    for left_out in [
        "unused-dev-dependencies",
        "unused-catalog-entries",
        "unresolved-catalog-references",
    ] {
        assert_eq!(rules[left_out].as_str(), Some("off"), "{left_out}");
    }
    assert_eq!(rules["unused-dependencies"].as_str(), Some("error"));
    assert_eq!(parsed["js"]["production"].as_bool(), Some(true));
    assert_eq!(class(&migration, "scripts.k: --production"), NoteClass::Exact);
}

#[test]
fn knip_flags_override_the_configuration_lists() {
    let migration = knip(Some(r#"{ "exclude": ["files"] }"#), &[("k", "knip --include files")]);
    assert_eq!(table(&migration)["rules"]["unused-files"].as_str(), Some("error"));
}

#[test]
fn knip_runs_that_disagree_are_noted_and_production_is_not_written() {
    let migration = knip(None, &[("a", "knip --production"), ("b", "knip")]);
    let parsed = table(&migration);
    assert!(parsed.get("js").is_none_or(|js| js.get("production").is_none()));
    assert_eq!(class(&migration, "scripts.a: --production"), NoteClass::Human);
    assert_eq!(
        class(&migration, "scripts.a, scripts.b: issue types"),
        NoteClass::Approximate
    );
    assert_eq!(
        parsed["rules"]["unused-dev-dependencies"].as_str(),
        Some("error"),
        "the run without --production reports them"
    );

    let agreeing = knip(None, &[("a", "knip --production"), ("b", "knip --strict")]);
    assert_eq!(table(&agreeing)["js"]["production"].as_bool(), Some(true));
}

#[test]
fn knip_class_members_is_not_an_issue_type() {
    let migration = knip(Some(r#"{ "ignoreIssues": { "a.ts": ["classMembers"] } }"#), &[]);
    assert!(
        has_note(&migration, NoteClass::Unsupported, "not a knip issue type"),
        "{:#?}",
        migration.notes
    );
}

#[test]
fn knip_scripts_behind_wrappers_are_read() {
    let migration = knip(
        None,
        &[
            ("lint", "npm run knip -- --include files"),
            ("ci", "CI=1 cross-env A=b dotenv -e .env -- knip --include files"),
        ],
    );
    assert!(
        !migration
            .notes
            .iter()
            .any(|note| note.message.contains("not translated")),
        "`--` is not a flag: {:#?}",
        migration.notes
    );
    assert_eq!(table(&migration)["rules"]["unused-files"].as_str(), Some("error"));
    assert_eq!(table(&migration)["rules"]["unused-dependencies"].as_str(), Some("off"));
}

#[test]
fn a_script_that_names_the_tool_in_an_unknown_wrapper_is_reported() {
    let migration = knip(None, &[("x", "tsx run.ts knip --files")]);
    assert_eq!(class(&migration, "scripts.x"), NoteClass::Human);
}

#[test]
fn knip_workspace_settings_stay_in_their_workspace() {
    let config = r#"{
      "ignoreMembers": ["Root"],
      "workspaces": {
        "apps/web": { "ignoreExportsUsedInFile": true, "ignoreMembers": ["Widget"] },
        "packages/*": { "ignoreMembers": ["Other"] }
      }
    }"#;
    let migration = knip(Some(config), &[]);
    let js = &table(&migration)["js"];
    assert_eq!(strings(&js["ignore-members"]), ["Root"]);
    assert!(js.get("ignore-exports-used-in-file").is_none(), "{}", migration.toml);
    let web = &js["workspaces"]["apps/web"];
    assert_eq!(web["ignore-exports-used-in-file"].as_bool(), Some(true));
    assert_eq!(strings(&web["ignore-members"]), ["Widget"]);
    assert_eq!(strings(&js["workspaces"]["packages/*"]["ignore-members"]), ["Other"]);
    assert!(
        !migration
            .notes
            .iter()
            .any(|note| note.message.contains("every workspace")),
        "{:#?}",
        migration.notes
    );
}

#[test]
fn stale_js_config_is_never_both_on_and_turned_off() {
    let migration = knip(None, &[("k", "knip --treat-config-hints-as-errors")]);
    assert_eq!(table(&migration)["rules"]["stale-js-config"].as_str(), Some("warn"));
    assert!(!migration.extra_checks.iter().any(|id| id == "stale-js-config"));
    let other = knip(None, &[("k", "knip")]);
    assert_eq!(table(&other)["rules"]["stale-js-config"].as_str(), Some("off"));
    assert!(other.extra_checks.iter().any(|id| id == "stale-js-config"));
}

#[test]
fn config_hints_as_errors_are_warnings_unless_strict_hints_asks_for_error() {
    let migration = knip(None, &[("k", "knip --treat-config-hints-as-errors")]);
    assert!(
        has_note(&migration, NoteClass::Approximate, "stale-js-config is warn"),
        "{:#?}",
        migration.notes
    );

    let mut input = ToolInput::new(Tool::Knip);
    input.package_json = Some(text(
        "package.json",
        &package(&[("k", "knip --treat-config-hints-as-errors")]),
    ));
    let strict = Options {
        strict_hints: true,
        ..options(Profile::Exact)
    };
    let migration = migrate(&[input], &strict).unwrap();
    assert_eq!(table(&migration)["rules"]["stale-js-config"].as_str(), Some("error"));
    assert!(!has_note(&migration, NoteClass::Approximate, "stale-js-config is warn"));
}

#[test]
fn no_config_hints_turns_the_rule_off() {
    let migration = knip(None, &[("k", "knip --no-config-hints")]);
    assert_eq!(table(&migration)["rules"]["stale-js-config"].as_str(), Some("off"));
}

fn off_rules(migration: &Migration) -> Vec<String> {
    table(migration)["rules"]
        .as_table()
        .unwrap()
        .iter()
        .filter(|(_, level)| level.as_str() == Some("off"))
        .map(|(id, _)| id.clone())
        .collect()
}

#[test]
fn syncpack_without_a_catalog_policy_does_not_run_the_catalog_rules_that_need_one() {
    let migration = syncpack("{}", Profile::Exact);
    let off = off_rules(&migration);
    for id in ["catalog-not-used", "catalog-missing-entry", "unresolved-catalog-references"] {
        assert!(off.iter().any(|known| known == id), "{id} is off: {off:?}");
    }
    for id in ["catalog-mismatch", "invalid-local-version", "version-mismatch", "policy-conflict"] {
        assert!(!off.iter().any(|known| known == id), "{id} stays on: {off:?}");
    }
}

#[test]
fn syncpack_catalog_policy_keeps_the_catalog_rules() {
    let migration = syncpack(r#"{ "versionGroups": [{ "policy": "catalog" }] }"#, Profile::Exact);
    let off = off_rules(&migration);
    for id in ["catalog-not-used", "catalog-missing-entry", "unresolved-catalog-references"] {
        assert!(!off.iter().any(|known| known == id), "{id} stays on: {off:?}");
    }
    assert_eq!(table(&migration)["catalog"]["mode"].as_str(), Some("prefer"));
}

#[test]
fn a_narrow_catalog_policy_group_does_not_turn_the_catalog_rules_on() {
    let migration = syncpack(
        r#"{ "versionGroups": [{ "policy": "catalog", "dependencies": ["react"] }] }"#,
        Profile::Exact,
    );
    assert!(off_rules(&migration).iter().any(|id| id == "catalog-not-used"));
}

#[test]
fn every_rule_a_tool_does_not_run_is_off_for_that_tool() {
    let table_of = |tool: Tool| {
        let mut input = ToolInput::new(tool);
        if tool == Tool::Syncpack {
            input.config = Some(text(".syncpackrc.json", "{}"));
        }
        off_rules(&migrate(&[input], &options(Profile::Exact)).unwrap())
    };
    let sherif = table_of(Tool::Sherif);
    for id in [
        "catalog-mismatch",
        "catalog-not-used",
        "invalid-local-version",
        "unparseable-files",
        "workspace-protocol",
    ] {
        assert!(sherif.iter().any(|known| known == id), "sherif does not run {id}");
    }
    for id in ["unmatched-workspace-patterns", "missing-package-manifest", "unsynced-dependency-family"] {
        assert!(!sherif.iter().any(|known| known == id), "sherif runs {id}");
    }
    let syncpack = table_of(Tool::Syncpack);
    for id in ["unmatched-workspace-patterns", "missing-package-manifest", "unsynced-dependency-family"] {
        assert!(syncpack.iter().any(|known| known == id), "syncpack does not run {id}");
    }
}

fn effective(migration: &Migration) -> (toml::Value, Option<bool>) {
    let parsed = table(migration);
    let rules = parsed
        .get("rules")
        .cloned()
        .unwrap_or_else(|| toml::Value::Table(toml::Table::new()));
    let enabled = parsed
        .get("js")
        .and_then(|js| js.get("enabled"))
        .and_then(toml::Value::as_bool);
    (rules, enabled)
}

fn all_three() -> Vec<ToolInput> {
    let mut sherif = ToolInput::new(Tool::Sherif);
    sherif.package_json = Some(text("package.json", &package(&[("lint", "sherif --fail-on-warnings")])));
    let mut syncpack = ToolInput::new(Tool::Syncpack);
    syncpack.config = Some(read("syncpack", ".syncpackrc.yaml"));
    let mut knip = ToolInput::new(Tool::Knip);
    knip.config = Some(read("knip", "knip.ts"));
    vec![sherif, syncpack, knip]
}

#[test]
fn migrating_the_tools_one_after_the_other_checks_what_one_run_checks() {
    let inputs = all_three();
    let expected = effective(&migrate(&inputs, &options(Profile::Exact)).unwrap());
    assert_eq!(expected.1, None, "knip keeps the JavaScript analysis on");
    for order in [[0, 1, 2], [2, 1, 0], [1, 2, 0], [2, 0, 1]] {
        let mut merged: Option<String> = None;
        for index in order {
            let step = migrate(&[inputs[index].clone()], &options(Profile::Exact)).unwrap();
            merged = Some(match &merged {
                None => step.toml,
                Some(existing) => step.merge_into(existing).unwrap().toml,
            });
        }
        let merged = merged.unwrap();
        let parsed: toml::Table = merged.parse().unwrap();
        assert_eq!(parsed.get("rules"), Some(&expected.0), "{order:?}\n{merged}");
        assert_eq!(
            parsed.get("js").and_then(|js| js.get("enabled")),
            None,
            "{order:?}: the JavaScript analysis is on\n{merged}"
        );
        assert!(
            merged.contains("sources = [") && merged.matches("sources = [").count() == 1,
            "{merged}"
        );
    }
}

#[test]
fn merging_adds_new_tables_after_the_existing_ones() {
    let migration = migrate(
        &[sherif_input(&package(&[("l", "sherif --fail-on-warnings")]))],
        &options(Profile::Full),
    )
    .unwrap();
    let existing = "[js]\nentry = [\"a\"]\n\n[rules]\nunused-files = \"warn\"\n\n[[version-groups]]\nlabel = \"x\"\n";
    let merged = migration.merge_into(existing).unwrap();
    assert!(merged.toml.starts_with(existing), "{}", merged.toml);
    assert_eq!(merged.toml.matches("#:schema").count(), 0, "{}", merged.toml);
    assert_eq!(
        merged.toml,
        format!(
            "{existing}\n# Added by `skif migrate --merge` from sherif (profile: full).\n\n[lint]\nfail-on-warnings = true\n# skif migrate: sources = [\"sherif\"]\n"
        )
    );
}
