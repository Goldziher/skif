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
        class(&migration, "versionGroups[5].dependencyTypes"),
        NoteClass::Unsupported
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
    assert_eq!(migration.config.version_groups.len(), 6);
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
    assert_eq!(parsed["rules"]["stale-js-config"].as_str(), Some("error"));
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
    ] {
        assert_eq!(rules[enabled].as_str(), Some("error"), "{enabled}");
    }
    assert_eq!(rules["stale-js-config"].as_str(), Some("error"));
    assert_eq!(
        rules["unlisted-binaries"].as_str(),
        Some("off"),
        "excluded with --exclude binaries"
    );
    assert_eq!(rules["unused-files"].as_str(), Some("off"));
    assert!(!rules.contains_key("unparseable-files"));
    assert_eq!(parsed["js"]["production"].as_bool(), Some(true));
    assert_eq!(parsed["js"]["include-entry-exports"].as_bool(), Some(true));
    assert_eq!(
        class(&migration, "scripts.knip:strict: --strict"),
        NoteClass::Unsupported
    );
    assert_eq!(class(&migration, "scripts.knip:strict: --workspace"), NoteClass::Human);
    assert_eq!(migration.extra_checks.len(), 32);
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
    assert!(merged.comments_preserved);
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
