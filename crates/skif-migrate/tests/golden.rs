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
    Input::new(name, fs::read_to_string(path).unwrap())
}

fn known_rules() -> Vec<RuleInfo> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rule-ids.txt");
    fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| RuleInfo::new(id, !["circular-imports", "package-json-format"].contains(&id)))
        .collect()
}

fn options(profile: Profile) -> Options {
    Options::new(profile).with_known_rules(known_rules())
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
    assert_eq!(ignored, ["@demo/legacy", "apps/legacy"]);
    assert_eq!(migration.config.ignore.len(), 1);
}

/// A project whose only sherif configuration is the short flags of a root script, as the GitHub Action translates it
/// (`--profile full`) when the project has no `skif.toml` yet.
#[test]
fn sherif_short_flags_of_a_root_script_alone() {
    let mut input = ToolInput::new(Tool::Sherif);
    input.package_json = Some(read("sherif-script", "package.json"));
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    insta::assert_snapshot!("sherif-script", migration.toml);

    assert_eq!(class(&migration, "scripts.lint:deps: --ignore-rule"), NoteClass::Exact);
    assert_eq!(
        class(&migration, "scripts.lint:deps: --fail-on-warnings"),
        NoteClass::Exact
    );
    assert!(has_note(&migration, NoteClass::Approximate, "react@17.0.2"));
    assert!(!migration.needs_human());
    assert!(migration.config.lint.fail_on_warnings);
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
        40,
        "the two ignored sherif rules and the 38 other default rules"
    );
    assert!(!rules.contains_key("version-mismatch"), "the check sherif ran stays on");
    assert!(
        !rules.contains_key("invalid-workspace-file"),
        "sherif stops on an invalid manifest, so the rule that reports one stays on"
    );
    assert_eq!(rules["unused-files"].as_str(), Some("off"));
    assert_eq!(rules["unsorted-dependencies"].as_str(), Some("off"));
    assert_eq!(parsed["js"]["enabled"].as_bool(), Some(false));
    assert_eq!(migration.extra_checks.len(), 38);
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
    assert_eq!(class(&migration, "customTypes.localish"), NoteClass::Exact);
    for dropped in ["versionGroups[5]", "versionGroups[6]", "semverGroups[1]"] {
        assert!(
            migration
                .notes
                .iter()
                .any(|note| note.path == dropped && note.message.contains("outside --dependency-types prod,dev")),
            "{dropped} covers only types the lint script leaves out: {:#?}",
            migration.notes
        );
    }
    assert_eq!(class(&migration, "dependencyGroups"), NoteClass::Unsupported);
    assert_eq!(class(&migration, "source"), NoteClass::Default);
    assert_eq!(class(&migration, "sortAz"), NoteClass::Approximate);
    assert_eq!(class(&migration, "updateGroups[0]"), NoteClass::Exact);
    assert_eq!(class(&migration, "updateGroups[1]"), NoteClass::Exact);
    assert_eq!(class(&migration, "updateGroups[2].target"), NoteClass::Human);
    assert_eq!(class(&migration, "minimumReleaseAge"), NoteClass::Exact);
    assert_eq!(migration.config.update_groups.len(), 2);
    assert_eq!(migration.config.update.minimum_release_age, Some(2880));
    assert_eq!(migration.config.update.max_concurrent_requests, 4);
    assert_eq!(
        class(&migration, "scripts.lint:versions: --dependency-types"),
        NoteClass::Exact
    );
    assert_eq!(
        class(&migration, "scripts.lint:versions: --dependencies"),
        NoteClass::Human
    );
    assert!(
        note(&migration, "scripts.lint:versions: --dependencies")
            .suggestion
            .as_deref()
            .is_some_and(|suggestion| suggestion.contains("`--dependency`")),
        "syncpack's --dependencies is skif's --dependency"
    );
    assert!(migration.needs_human());
    assert_eq!(migration.config.version_groups.len(), 6);
    assert_eq!(
        migration.config.version_groups[4].dependency_types,
        ["prod", "dev"],
        "`!local` within prod,dev"
    );
    assert_eq!(
        migration.config.version_groups[5].policy,
        skif_config::VersionPolicy::SemverRangeOnly,
        "semverRangeOnly keeps its range check"
    );
    assert_eq!(migration.config.semver_groups.len(), 2);
    assert_eq!(migration.config.semver_groups[1].dependency_types, ["prod"]);
}

#[test]
fn syncpack_exact_profile_keeps_the_syncpack_rules() {
    let mut input = ToolInput::new(Tool::Syncpack);
    input.config = Some(read("syncpack", ".syncpackrc.yaml"));
    let migration = migrate(&[input], &options(Profile::Exact)).unwrap();
    let parsed = table(&migration);
    let rules = parsed["rules"].as_table().unwrap();
    for kept in ["version-mismatch", "semver-range-mismatch", "banned-dependency"] {
        assert!(!rules.contains_key(kept), "{kept} is kept");
    }
    assert_eq!(
        rules["unsorted-dependencies"].as_str(),
        Some("off"),
        "sortAz only matters to `syncpack format`, which no script runs"
    );
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
            .any(|note| note.path == "ignoreFiles" && note.class == NoteClass::Exact),
        "ignoreFiles maps to ignore-files"
    );
    assert!(
        migration.notes.iter().any(|note| note.path == "tags"
            && note.class == NoteClass::Approximate
            && note.message.contains("only exports")),
        "+public limits unused-exports to tagged exports, which knip would never report"
    );
    assert_eq!(class(&migration, "treatTagHintsAsErrors"), NoteClass::Approximate);
    assert_eq!(class(&migration, "ignoreGlobalBinaries"), NoteClass::Exact);
    assert_eq!(class(&migration, "cycles.allow"), NoteClass::Approximate);
    assert_eq!(class(&migration, "cycles.dynamicImports"), NoteClass::Exact);
    assert_eq!(class(&migration, "compilers.mdx"), NoteClass::Approximate);
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
        ["src/index.ts"],
        "workspaces[\".\"] replaces the top-level entry, as in knip"
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
    assert_eq!(
        rules["stale-mocks"].as_str(),
        Some("off"),
        "knip does not read the specifiers of test-runner mocks"
    );
    assert_eq!(migration.extra_checks.len(), 40);
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
    input.config = Some(Input::new("knip.ts", "export default await loadConfigFromNetwork();"));
    let migration = migrate(&[input], &options(Profile::Full));
    match migration {
        Ok(migration) => assert!(migration.needs_human(), "{:#?}", migration.notes),
        Err(error) => assert!(error.to_string().contains("knip.ts"), "{error}"),
    }
}

fn text(path: &str, text: &str) -> Input {
    Input::new(path, text)
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
fn a_bare_sherif_run_writes_only_the_scope_sherif_compares() {
    let mut input = ToolInput::new(Tool::Sherif);
    input.package_json = Some(text("package.json", &package(&[("lint", "sherif")])));
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    assert!(!migration.toml.contains("ignore-packages"), "{}", migration.toml);
    assert!(!migration.toml.contains("[workspace]"), "{}", migration.toml);
    let parsed = table(&migration);
    assert_eq!(parsed.len(), 1, "{}", migration.toml);
    assert_eq!(strings(&parsed["policy"]["dependency-types"]), ["prod", "dev"]);
    assert_eq!(parsed["policy"].as_table().unwrap().len(), 1, "{}", migration.toml);
    let paths: Vec<&str> = migration.notes.iter().map(|note| note.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "multiple-dependency-versions",
            "multiple-dependency-versions: dependencies and devDependencies",
            "unsync-similar-dependencies",
            "multiple-dependency-versions: specifiers"
        ],
        "{:#?}",
        migration.notes
    );
    assert_eq!(class(&migration, "multiple-dependency-versions"), NoteClass::Exact);
    assert_eq!(
        class(
            &migration,
            "multiple-dependency-versions: dependencies and devDependencies"
        ),
        NoteClass::Approximate,
        "sherif merged the two sections of a package, skif compares both entries"
    );
}

#[test]
fn sherif_leaves_the_own_versions_out_of_the_scope_and_syncpack_brings_them_back() {
    let sherif = || sherif_input(&package(&[("lint", "sherif")]));
    let alone = migrate(&[sherif()], &options(Profile::Exact)).unwrap();
    assert_eq!(alone.config.policy.dependency_types, ["prod", "dev"]);

    let mut syncpack = ToolInput::new(Tool::Syncpack);
    syncpack.package_json = Some(text("package.json", &package(&[("lint", "syncpack lint")])));
    let default_scope = [
        "prod",
        "dev",
        "peer",
        "local",
        "resolutions",
        "overrides",
        "pnpm-overrides",
    ];
    for inputs in [[sherif(), syncpack.clone()], [syncpack.clone(), sherif()]] {
        let both = migrate(&inputs, &options(Profile::Exact)).unwrap();
        let both_table = table(&both);
        let mut scope = strings(&both_table["policy"]["dependency-types"]);
        scope.sort_unstable();
        let mut expected = default_scope.to_vec();
        expected.sort_unstable();
        assert_eq!(scope, expected, "{}", both.toml);
    }

    let mut narrowed = ToolInput::new(Tool::Syncpack);
    narrowed.package_json = Some(text(
        "package.json",
        &package(&[("lint", "syncpack lint --dependency-types peer")]),
    ));
    let united = migrate(&[narrowed, sherif()], &options(Profile::Exact)).unwrap();
    assert_eq!(united.config.policy.dependency_types, ["peer", "prod", "dev"]);
}

#[test]
fn sherif_package_paths_lose_their_dot_slash_prefix() {
    let migration = migrate(
        &[sherif_input(
            r#"{ "name": "demo", "sherif": { "ignorePackage": ["./apps/c", "./integrations/*", "@s/*", "legacy"] },
                 "scripts": { "lint": "sherif -p ./tools/x" } }"#,
        )],
        &options(Profile::Full),
    )
    .unwrap();
    assert_eq!(
        strings(&table(&migration)["workspace"]["ignore-packages"]),
        ["apps/c", "integrations/*", "@s/*", "legacy", "tools/x"]
    );
    assert!(!has_note(&migration, NoteClass::Approximate, "look like paths"));

    let bare = migrate(
        &[sherif_input(
            r#"{ "name": "demo", "sherif": { "ignorePackage": ["apps/c"] } }"#,
        )],
        &options(Profile::Full),
    )
    .unwrap();
    assert_eq!(strings(&table(&bare)["workspace"]["ignore-packages"]), ["apps/c"]);
    assert!(has_note(&bare, NoteClass::Approximate, "apps/c look like paths"));
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
    assert_eq!(
        table(&migration)["policy"]["override-reading"].as_str(),
        Some("syncpack")
    );
    assert_eq!(
        migration.config.policy.override_reading,
        skif_config::OverrideReading::Syncpack
    );
    assert!(has_note(&migration, NoteClass::Exact, "override-reading"));

    let mut sherif = ToolInput::new(Tool::Sherif);
    sherif.package_json = Some(text("package.json", &package(&[])));
    let migration = migrate(&[sherif], &options(Profile::Full)).unwrap();
    assert!(
        table(&migration)["policy"].get("override-reading").is_none(),
        "only syncpack reads overrides its own way"
    );
}

#[test]
fn a_bare_knip_run_writes_no_js_setting() {
    let migration = knip(None, &[("knip", "knip")]);
    let parsed = table(&migration);
    assert!(parsed.get("js").is_none(), "{}", migration.toml);
    assert_eq!(parsed["rules"]["unused-files"].as_str(), Some("error"));
}

#[test]
fn sherif_short_flags_with_attached_values_are_translated() {
    let migration = migrate(
        &[sherif_input(&package(&[("lint", "sherif -p=@x/y -ireact -i=lodash")]))],
        &options(Profile::Exact),
    )
    .unwrap();
    let parsed = table(&migration);
    assert_eq!(strings(&parsed["workspace"]["ignore-packages"]), ["@x/y"]);
    assert_eq!(strings(&parsed["ignore"][0]["dependencies"]), ["react", "lodash"]);
    assert!(!migration.needs_human(), "{:#?}", migration.notes);
}

#[test]
fn sherif_values_are_not_split_at_commas() {
    let migration = migrate(
        &[sherif_input(&package(&[(
            "lint",
            "sherif -i react,vue -i lodash -p a,b",
        )]))],
        &options(Profile::Exact),
    )
    .unwrap();
    let parsed = table(&migration);
    assert_eq!(strings(&parsed["ignore"][0]["dependencies"]), ["lodash"]);
    assert!(parsed.get("workspace").is_none(), "{}", migration.toml);
    let joined = note(&migration, "scripts.lint: --ignore-dependency");
    assert_eq!(joined.class, NoteClass::Human);
    assert!(joined.message.contains("`react,vue` matched nothing"), "{joined:#?}");
    assert_eq!(class(&migration, "scripts.lint: --ignore-package"), NoteClass::Human);
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
fn sherif_writes_no_scope_when_it_ignores_its_version_check() {
    for package_json in [
        r#"{ "name": "demo", "sherif": { "ignoreRule": ["multiple-dependency-versions"] } }"#,
        r#"{ "name": "demo", "scripts": { "lint": "sherif -r multiple-dependency-versions" } }"#,
    ] {
        let migration = migrate(&[sherif_input(package_json)], &options(Profile::Full)).unwrap();
        let parsed = table(&migration);
        assert!(parsed.get("policy").is_none(), "{}", migration.toml);
        assert!(
            !migration
                .notes
                .iter()
                .any(|note| note.path.starts_with("multiple-dependency-versions")),
            "{:#?}",
            migration.notes
        );
        assert_eq!(
            parsed["rules"]["version-mismatch"].as_str(),
            Some("off"),
            "{}",
            migration.toml
        );
    }
}

/// The specifier types sherif's semver parser rejects, so `multiple-dependency-versions` never compared them.
const SHERIF_UNPARSED: [&str; 11] = [
    "alias",
    "file",
    "git",
    "latest",
    "link",
    "missing",
    "range-complex",
    "tag",
    "unsupported",
    "url",
    "workspace-protocol",
];

#[test]
fn sherif_exact_leaves_out_the_specifiers_sherif_does_not_compare() {
    let bare = || sherif_input(&package(&[("lint", "sherif")]));
    let migration = migrate(&[bare()], &options(Profile::Exact)).unwrap();
    let parsed = table(&migration);
    let groups = parsed["version-groups"].as_array().unwrap();
    assert_eq!(groups.len(), 1, "{}", migration.toml);
    assert_eq!(strings(&groups[0]["specifier-types"]), SHERIF_UNPARSED);
    assert_eq!(groups[0]["policy"].as_str(), Some("ignored"));
    assert!(
        groups[0].get("dependency-types").is_none(),
        "the group keeps sherif's scope"
    );
    assert_eq!(
        migration.config.version_groups[0].policy,
        skif_config::VersionPolicy::Ignored
    );
    assert_eq!(
        parsed["rules"]["policy-conflict"].as_str(),
        Some("off"),
        "nothing reports the group when the workspace has no such specifier"
    );
    assert_eq!(
        class(&migration, "multiple-dependency-versions: specifiers"),
        NoteClass::Approximate
    );
    assert!(
        note(&migration, "multiple-dependency-versions: specifiers")
            .message
            .contains("`>= 1.0.0`")
    );

    let full = migrate(&[bare()], &options(Profile::Full)).unwrap();
    assert!(table(&full).get("version-groups").is_none(), "{}", full.toml);
    let hint = note(&full, "multiple-dependency-versions: specifiers");
    assert_eq!(hint.class, NoteClass::Approximate);
    assert!(
        hint.suggestion.as_deref().is_some_and(
            |suggestion| suggestion.contains("\"range-complex\"") && suggestion.contains("policy = \"ignored\"")
        ),
        "{hint:#?}"
    );

    let mut syncpack = ToolInput::new(Tool::Syncpack);
    syncpack.package_json = Some(text("package.json", &package(&[("lint", "syncpack lint")])));
    for inputs in [[bare(), syncpack.clone()], [syncpack.clone(), bare()]] {
        let both = migrate(&inputs, &options(Profile::Exact)).unwrap();
        assert!(
            both.config.version_groups.is_empty(),
            "syncpack compared every specifier: {}",
            both.toml
        );
        assert!(
            note(&both, "multiple-dependency-versions: specifiers")
                .message
                .contains("syncpack")
        );
    }
}

#[test]
fn sherif_family_differences_are_noted() {
    let scoped = migrate(
        &[sherif_input(&package(&[("lint", "sherif")]))],
        &options(Profile::Exact),
    )
    .unwrap();
    let noted = note(&scoped, "unsync-similar-dependencies");
    assert_eq!(noted.class, NoteClass::Approximate);
    assert!(noted.message.contains("`catalog:`"), "{noted:#?}");
    assert!(
        !noted.message.contains("optionalDependencies"),
        "the scope keeps them out: {noted:#?}"
    );

    let unscoped = migrate(
        &[sherif_input(&package(&[(
            "lint",
            "sherif -r multiple-dependency-versions",
        )]))],
        &options(Profile::Exact),
    )
    .unwrap();
    assert!(
        note(&unscoped, "unsync-similar-dependencies")
            .message
            .contains("optionalDependencies take part")
    );

    let ignored = migrate(
        &[sherif_input(&package(&[(
            "lint",
            "sherif -r unsync-similar-dependencies",
        )]))],
        &options(Profile::Exact),
    )
    .unwrap();
    assert!(
        !ignored
            .notes
            .iter()
            .any(|note| note.path == "unsync-similar-dependencies")
    );
}

#[test]
fn an_empty_root_dependencies_object_is_noted_for_sherif() {
    let empty = migrate(
        &[sherif_input(
            r#"{ "name": "demo", "private": true, "dependencies": {}, "scripts": { "lint": "sherif" } }"#,
        )],
        &options(Profile::Exact),
    )
    .unwrap();
    let noted = note(&empty, "dependencies");
    assert_eq!(noted.class, NoteClass::Approximate);
    assert!(noted.message.contains("empty-dependencies, an error"), "{noted:#?}");

    for package_json in [
        r#"{ "name": "demo", "private": true, "dependencies": { "x": "1.0.0" } }"#,
        r#"{ "name": "demo", "private": true, "devDependencies": {} }"#,
        r#"{ "name": "demo", "private": true, "dependencies": {}, "sherif": { "ignoreRule": ["empty-dependencies"] } }"#,
    ] {
        let migration = migrate(&[sherif_input(package_json)], &options(Profile::Exact)).unwrap();
        assert!(
            !migration.notes.iter().any(|note| note.path == "dependencies"),
            "{package_json}: {:#?}",
            migration.notes
        );
    }
}

#[test]
fn the_sherif_group_follows_the_groups_of_an_existing_file() {
    let migration = migrate(
        &[sherif_input(&package(&[("lint", "sherif")]))],
        &options(Profile::Exact),
    )
    .unwrap();
    let existing =
        "[[version-groups]]\nlabel = \"mine\"\ndependencies = [\"react\"]\npolicy = \"pinned\"\npin = \"18.3.1\"\n";
    let merged: toml::Table = migration.merge_into(existing).unwrap().toml.parse().unwrap();
    let labels: Vec<&str> = merged["version-groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| group["label"].as_str().unwrap())
        .collect();
    assert_eq!(labels, ["mine", "not compared by sherif"]);
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

    let star = syncpack(
        r#"{ "versionGroups": [{ "dependencies": ["**"], "packages": ["**"], "policy": "catalog" }] }"#,
        Profile::Full,
    );
    assert_eq!(
        table(&star)["catalog"]["mode"].as_str(),
        Some("enforce"),
        "{}",
        star.toml
    );

    let global = syncpack(r#"{ "versionGroups": [{ "policy": "catalog" }] }"#, Profile::Full);
    let parsed = table(&global);
    assert_eq!(parsed["catalog"]["mode"].as_str(), Some("enforce"));
    assert_eq!(parsed["catalog"]["min-packages"].as_integer(), Some(1));
    assert_eq!(global.config.catalog.min_packages, 1);
    assert_eq!(class(&global, "versionGroups[0].policy"), NoteClass::Exact);
}

#[test]
fn groups_are_cut_to_the_dependency_types_the_lint_script_scopes_to() {
    let mut input = ToolInput::new(Tool::Syncpack);
    input.config = Some(text(
        ".syncpackrc.json",
        r#"{
          "versionGroups": [
            { "dependencies": ["fsevents"], "dependencyTypes": ["peer"], "isBanned": true },
            { "dependencies": ["react"], "dependencyTypes": ["dev", "peer"], "pinVersion": "18.3.1" },
            { "dependencies": ["lodash"], "pinVersion": "4.17.21" }
          ],
          "semverGroups": [{ "dependencyTypes": ["!dev"], "range": "~" }]
        }"#,
    ));
    input.package_json = Some(text(
        "package.json",
        &package(&[("lint", "syncpack lint --dependency-types prod,dev")]),
    ));
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    let parsed = table(&migration);
    let groups = parsed["version-groups"].as_array().unwrap();
    assert_eq!(groups.len(), 2, "{}", migration.toml);
    assert_eq!(strings(&groups[0]["dependencies"]), ["react"]);
    assert_eq!(strings(&groups[0]["dependency-types"]), ["dev"]);
    assert!(groups[1].get("dependency-types").is_none(), "{}", migration.toml);
    assert_eq!(
        strings(&parsed["semver-groups"][0]["dependency-types"]),
        ["prod"],
        "{}",
        migration.toml
    );
    assert!(
        migration
            .notes
            .iter()
            .any(|note| note.path == "versionGroups[0]" && note.message.contains("--dependency-types prod,dev")),
        "{:#?}",
        migration.notes
    );

    let unscoped = syncpack(
        r#"{ "versionGroups": [{ "dependencies": ["fsevents"], "dependencyTypes": ["peer"], "isBanned": true }] }"#,
        Profile::Full,
    );
    assert_eq!(
        strings(&table(&unscoped)["version-groups"][0]["dependency-types"]),
        ["peer"]
    );
}

#[test]
fn a_named_policy_wins_over_snap_to_and_prefer_version_under_same_minor_is_noted() {
    let migration = syncpack(
        r#"{ "versionGroups": [
          { "dependencies": ["react"], "policy": "sameRange", "snapTo": ["app"] },
          { "dependencies": ["typescript"], "policy": "sameMinor", "preferVersion": "lowestSemver" },
          { "dependencies": ["lodash"], "snapTo": ["app"] }
        ] }"#,
        Profile::Full,
    );
    let parsed = table(&migration);
    let groups = parsed["version-groups"].as_array().unwrap();
    assert_eq!(groups[0]["policy"].as_str(), Some("same-range"), "{}", migration.toml);
    assert!(groups[0].get("snap-to").is_none(), "{}", migration.toml);
    assert_eq!(groups[1]["policy"].as_str(), Some("same-minor"));
    assert!(groups[1].get("prefer").is_none());
    assert_eq!(
        class(&migration, "versionGroups[1].preferVersion"),
        NoteClass::Approximate
    );
    assert_eq!(groups[2]["policy"].as_str(), Some("snapped-to"));
}

#[test]
fn a_group_that_only_covers_a_type_skif_cannot_read_is_not_written() {
    let config = r#"{
      "customTypes": { "odd": { "path": "x", "strategy": "version", "source": ["other.json"] } },
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
fn a_group_on_a_type_syncpack_does_not_know_is_noted_as_invalid_for_syncpack() {
    let migration = syncpack(
        r#"{ "versionGroups": [{ "dependencyTypes": ["optionalDependencies"], "isBanned": true }] }"#,
        Profile::Full,
    );
    assert!(table(&migration).get("version-groups").is_none(), "{}", migration.toml);
    let noted = note(&migration, "versionGroups[0].dependencyTypes");
    assert_eq!(noted.class, NoteClass::Human);
    assert!(
        noted
            .message
            .contains("optionalDependencies is neither a built-in nor a custom dependency type"),
        "{noted:#?}"
    );
    assert!(!noted.message.contains("skif does not read"), "{noted:#?}");
}

#[test]
fn a_group_outside_the_lint_scope_is_not_also_noted_as_written() {
    let mut input = ToolInput::new(Tool::Syncpack);
    input.config = Some(text(
        ".syncpackrc.json",
        r#"{ "versionGroups": [{ "label": "peers loose", "dependencyTypes": ["peer"], "dependencies": ["react"], "isIgnored": true }] }"#,
    ));
    input.package_json = Some(text(
        "package.json",
        &package(&[("l", "syncpack lint --dependency-types prod,dev")]),
    ));
    let migration = migrate(&[input], &options(Profile::Exact)).unwrap();
    assert!(migration.config.version_groups.is_empty(), "{}", migration.toml);
    let messages: Vec<&str> = migration
        .notes
        .iter()
        .filter(|note| note.path == "versionGroups[0]")
        .map(|note| note.message.as_str())
        .collect();
    assert_eq!(messages.len(), 1, "{messages:#?}");
    assert!(messages[0].contains("it was not written"), "{messages:#?}");
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
fn syncpack_runs_that_exclude_different_types_are_united_by_what_they_select() {
    let mut input = ToolInput::new(Tool::Syncpack);
    input.package_json = Some(text(
        "package.json",
        &package(&[
            ("a", "syncpack lint --dependency-types !dev,!peer"),
            ("b", "syncpack lint --dependency-types !peer,!local"),
        ]),
    ));
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    assert_eq!(
        strings(&table(&migration)["policy"]["dependency-types"]),
        ["!peer"],
        "{}",
        migration.toml
    );
    assert_eq!(
        class(&migration, "scripts.a, scripts.b: --dependency-types"),
        NoteClass::Approximate
    );
}

#[test]
fn syncpack_runs_whose_union_no_list_says_keep_the_default_scope_and_the_groups() {
    let mut input = ToolInput::new(Tool::Syncpack);
    input.config = Some(text(
        ".syncpackrc.json",
        r#"{ "semverGroups": [{ "dependencyTypes": ["!local"], "range": "~" }] }"#,
    ));
    input.package_json = Some(text(
        "package.json",
        &package(&[
            ("a", "syncpack lint --dependency-types !dev"),
            ("b", "syncpack lint --dependency-types dev"),
        ]),
    ));
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
        ],
        "{}",
        migration.toml
    );
    assert_eq!(
        strings(&parsed["semver-groups"][0]["dependency-types"]),
        ["!local"],
        "{}",
        migration.toml
    );
    assert_eq!(
        class(&migration, "scripts.a, scripts.b: --dependency-types"),
        NoteClass::Human
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
fn knip_catalog_type_turns_on_the_unused_catalog_entries_report() {
    let migration = knip(None, &[("k", "knip")]);
    let parsed = table(&migration);
    assert_eq!(parsed["rules"]["unused-catalog-entries"].as_str(), Some("error"));
    assert_eq!(parsed["catalog"]["report-unused-entries"].as_bool(), Some(true));
    assert!(migration.config.catalog.report_unused_entries);
    assert_eq!(class(&migration, "catalog"), NoteClass::Exact);

    for (config, scripts) in [
        (Some(r#"{ "exclude": ["catalog"] }"#), [("k", "knip")]),
        (Some(r#"{ "rules": { "catalog": "off" } }"#), [("k", "knip")]),
        (None, [("k", "knip --production")]),
        (None, [("k", "knip --include files")]),
    ] {
        let migration = knip(config, &scripts);
        assert!(
            table(&migration).get("catalog").is_none(),
            "{config:?} {scripts:?}: {}",
            migration.toml
        );
    }
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
fn knip_tsconfig_flags_are_written_to_js() {
    let migration = knip(None, &[("k", "knip --tsConfig tsconfig.app.json --use-tsconfig-files")]);
    let parsed = table(&migration);
    assert_eq!(parsed["js"]["tsconfig"].as_str(), Some("tsconfig.app.json"));
    assert_eq!(parsed["js"]["use-tsconfig-files"].as_bool(), Some(true));
    assert_eq!(class(&migration, "scripts.k: --tsConfig"), NoteClass::Exact);
    assert_eq!(class(&migration, "scripts.k: --use-tsconfig-files"), NoteClass::Exact);
    let short = knip(None, &[("k", "knip -t config/tsconfig.json")]);
    assert_eq!(table(&short)["js"]["tsconfig"].as_str(), Some("config/tsconfig.json"));
}

#[test]
fn knip_scripts_with_different_tsconfigs_are_noted() {
    let migration = knip(
        None,
        &[
            ("a", "knip --tsConfig tsconfig.a.json"),
            ("b", "knip -t tsconfig.b.json"),
        ],
    );
    assert_eq!(table(&migration)["js"]["tsconfig"].as_str(), Some("tsconfig.b.json"));
    assert_eq!(
        class(&migration, "scripts.a, scripts.b: --tsConfig"),
        NoteClass::Approximate
    );
    let same = knip(
        None,
        &[
            ("a", "knip --tsConfig tsconfig.a.json"),
            ("b", "knip -t tsconfig.a.json"),
        ],
    );
    assert!(
        !same
            .notes
            .iter()
            .any(|note| note.path == "scripts.a, scripts.b: --tsConfig"),
        "{:#?}",
        same.notes
    );
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
fn a_script_that_runs_the_knip_script_is_not_a_second_run() {
    let migration = knip(
        None,
        &[("knip", "knip --production"), ("ci", "pnpm run knip && pnpm test")],
    );
    assert_eq!(
        table(&migration)["js"]["production"].as_bool(),
        Some(true),
        "{}",
        migration.toml
    );
    assert!(
        !migration.notes.iter().any(|note| note.path.contains("scripts.ci")),
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
    let strict = options(Profile::Exact).with_strict_hints(true);
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
    for id in [
        "catalog-not-used",
        "catalog-missing-entry",
        "unresolved-catalog-references",
    ] {
        assert!(off.iter().any(|known| known == id), "{id} is off: {off:?}");
    }
    for id in [
        "catalog-mismatch",
        "invalid-local-version",
        "version-mismatch",
        "policy-conflict",
    ] {
        assert!(!off.iter().any(|known| known == id), "{id} stays on: {off:?}");
    }
}

#[test]
fn syncpack_catalog_policy_keeps_the_catalog_rules() {
    let migration = syncpack(r#"{ "versionGroups": [{ "policy": "catalog" }] }"#, Profile::Exact);
    let off = off_rules(&migration);
    for id in [
        "catalog-not-used",
        "catalog-missing-entry",
        "unresolved-catalog-references",
    ] {
        assert!(!off.iter().any(|known| known == id), "{id} stays on: {off:?}");
        assert_eq!(
            table(&migration)["rules"][id].as_str(),
            Some("error"),
            "{id} errors as in syncpack"
        );
    }
    assert_eq!(table(&migration)["catalog"]["mode"].as_str(), Some("enforce"));
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
    for id in [
        "unmatched-workspace-patterns",
        "missing-package-manifest",
        "unsynced-dependency-family",
    ] {
        assert!(!sherif.iter().any(|known| known == id), "sherif runs {id}");
    }
    let syncpack = table_of(Tool::Syncpack);
    for id in [
        "unmatched-workspace-patterns",
        "missing-package-manifest",
        "unsynced-dependency-family",
    ] {
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
            "{existing}\n# Added by `skif migrate --merge` from sherif (profile: full).\n\n[lint]\nfail-on-warnings = true\n\n[policy]\ndependency-types = [\"prod\", \"dev\"]\n# skif migrate: sources = [\"sherif\"]\n"
        )
    );
}

#[test]
fn a_custom_type_read_from_package_json_is_written() {
    let migration = syncpack(
        r#"{ "customTypes": {
              "engines": { "path": "engines", "strategy": "versionsByName", "source": "PackageJson" },
              "pnpmish": { "path": "x", "strategy": "versionsByName", "source": "PnpmWorkspace" },
              "odd": { "path": "y", "strategy": "versionsByName", "source": "Disk" }
            } }"#,
        Profile::Exact,
    );
    assert_eq!(class(&migration, "customTypes.engines"), NoteClass::Exact);
    let written: Vec<(&str, &str)> = migration
        .config
        .custom_types
        .iter()
        .map(|custom| (custom.name.as_str(), custom.path.as_str()))
        .collect();
    assert_eq!(written, [("engines", "engines")]);
    assert_eq!(class(&migration, "customTypes.pnpmish"), NoteClass::Unsupported);
    assert!(
        note(&migration, "customTypes.pnpmish")
            .message
            .contains("pnpm-workspace.yaml")
    );
    assert_eq!(class(&migration, "customTypes.odd"), NoteClass::Human);
    assert!(
        note(&migration, "customTypes.odd")
            .message
            .contains("`Disk` is not a syncpack source")
    );
}

#[test]
fn syncpack_format_options_are_written_to_the_format_table() {
    let mut input = ToolInput::new(Tool::Syncpack);
    input.config = Some(Input::new(
        ".syncpackrc.json",
        r#"{
  "sortFirst": ["name", "version"],
  "sortAz": ["keywords"],
  "sortExports": ["types", "default"],
  "sortPackages": false,
  "formatBugs": true,
  "formatRepository": true,
  "indent": "    "
}"#,
    ));
    let migration = migrate(&[input], &options(Profile::Exact)).unwrap();
    let parsed = table(&migration);
    let format = parsed["format"].as_table().unwrap();
    assert_eq!(format["sort-first"].as_array().unwrap().len(), 2);
    assert_eq!(format["sort-az"].as_array().unwrap()[0].as_str(), Some("keywords"));
    assert_eq!(format["sort-exports"].as_array().unwrap().len(), 2);
    assert_eq!(format["sort-packages"].as_bool(), Some(false));
    assert_eq!(format["format-bugs"].as_bool(), Some(true));
    assert_eq!(format["format-repository"].as_bool(), Some(true));
    assert_eq!(format["indent"].as_str(), Some("    "));
    assert!(
        parsed["rules"].get("package-json-format").is_none(),
        "`syncpack lint` never checks the layout: {}",
        migration.toml
    );
    assert!(has_note(
        &migration,
        NoteClass::Approximate,
        "no script runs `syncpack format`"
    ));
    assert_eq!(class(&migration, "sortFirst"), NoteClass::Exact);
    assert_eq!(class(&migration, "sortAz"), NoteClass::Approximate);
    assert_eq!(class(&migration, "indent"), NoteClass::Approximate);
    assert_eq!(migration.config.format.indent.as_deref(), Some("    "));
}

#[test]
fn a_syncpack_format_script_turns_the_format_rule_on() {
    let mut input = ToolInput::new(Tool::Syncpack);
    input.package_json = Some(Input::new(
        "package.json",
        r#"{"name":"root","private":true,"scripts":{"format:check":"syncpack format --check"}}"#,
    ));
    let migration = migrate(&[input], &options(Profile::Exact)).unwrap();
    let parsed = table(&migration);
    assert_eq!(parsed["rules"]["package-json-format"].as_str(), Some("error"));
    assert!(parsed.get("format").is_none(), "syncpack's defaults are skif's");
    assert_eq!(
        class(&migration, "scripts.format:check: syncpack format"),
        NoteClass::Exact
    );

    let mut plain = ToolInput::new(Tool::Syncpack);
    plain.package_json = Some(Input::new(
        "package.json",
        r#"{"name":"root","private":true,"scripts":{"lint":"syncpack lint"}}"#,
    ));
    let migration = migrate(&[plain], &options(Profile::Exact)).unwrap();
    assert!(
        table(&migration)
            .get("rules")
            .and_then(|rules| rules.get("package-json-format"))
            .is_none()
    );
}

#[test]
fn syncpack_format_keys_turn_the_layout_rules_on_only_with_a_format_script() {
    let run = |script: &str| {
        let mut input = ToolInput::new(Tool::Syncpack);
        input.config = Some(text(
            ".syncpackrc.json",
            r#"{ "sortAz": ["dependencies"], "indent": "  " }"#,
        ));
        input.package_json = Some(text("package.json", &package(&[("deps", script)])));
        migrate(&[input], &options(Profile::Exact)).unwrap()
    };
    let lint = run("syncpack lint");
    let parsed = table(&lint);
    assert_eq!(strings(&parsed["format"]["sort-az"]), ["dependencies"]);
    assert!(parsed["rules"].get("package-json-format").is_none(), "{}", lint.toml);
    assert_eq!(parsed["rules"]["unsorted-dependencies"].as_str(), Some("off"));
    assert_eq!(class(&lint, "sortAz, indent"), NoteClass::Approximate);

    let format = run("syncpack format");
    let parsed = table(&format);
    assert_eq!(parsed["rules"]["package-json-format"].as_str(), Some("error"));
    assert!(
        parsed["rules"].get("unsorted-dependencies").is_none(),
        "`syncpack format` sorts the dependencies: {}",
        format.toml
    );
    assert!(!format.notes.iter().any(|note| note.path == "sortAz, indent"));
}

#[test]
fn knip_built_in_compilers_map_to_skif_extractors() {
    let migration = knip(
        Some(r#"{ "compilers": { "mdx": true, ".styl": true, "vue": false, "hbs": true } }"#),
        &[],
    );
    assert_eq!(class(&migration, "compilers.mdx"), NoteClass::Default);
    assert!(
        note(&migration, "compilers.mdx")
            .suggestion
            .as_deref()
            .is_some_and(|hint| hint.contains("**/*.mdx"))
    );
    assert_eq!(class(&migration, "compilers[\".styl\"]"), NoteClass::Default);
    assert_eq!(class(&migration, "compilers.vue"), NoteClass::Default);
    assert_eq!(class(&migration, "compilers.hbs"), NoteClass::Unsupported);
    let notes: Vec<String> = migration
        .notes
        .iter()
        .filter(|note| note.path.starts_with("compilers"))
        .map(|note| {
            format!(
                "{} {}: {} ({})",
                note.class.letter(),
                note.path,
                note.message,
                note.suggestion.as_deref().unwrap_or("-")
            )
        })
        .collect();
    insta::assert_snapshot!("knip-compilers", notes.join("\n"));

    let all = knip(Some(r#"{ "compilers": true }"#), &[]);
    assert_eq!(class(&all, "compilers"), NoteClass::Default);
    assert!(!all.needs_human());
}

fn knip_file(name: &str, config: &str) -> Migration {
    let mut input = ToolInput::new(Tool::Knip);
    input.config = Some(text(name, config));
    input.package_json = Some(text("package.json", &package(&[("knip", "knip")])));
    migrate(&[input], &options(Profile::Exact)).unwrap()
}

#[test]
fn a_knip_comment_above_an_ignore_entry_becomes_its_reason() {
    let migration = knip_file(
        "knip.ts",
        "export default {\n    workspaces: {\n        'packages/app': {\n            ignoreDependencies: [\n                // false positive: loaded by the bundler from a config string\n                'some-loader',\n            ],\n        },\n    },\n};\n",
    );
    assert!(
        migration.toml.contains(
            "{ name = \"some-loader\", reason = \"false positive: loaded by the bundler from a config string\" }"
        ),
        "{}",
        migration.toml
    );
    assert_eq!(
        migration.config.js.workspaces["packages/app"].ignore_dependencies,
        vec![
            skif_config::IgnoreItem::new("some-loader")
                .with_reason("false positive: loaded by the bundler from a config string")
        ]
    );
    assert!(migration.config.missing_ignore_reason().is_none(), "{}", migration.toml);
}

#[test]
fn a_knip_comment_on_a_list_behind_a_constant_becomes_its_reason() {
    let migration = knip_file(
        "knip.ts",
        "const deps = [\n  // false positive: loaded by the bundler\n  'some-loader',\n];\nexport default { ignoreDependencies: deps };\n",
    );
    assert_eq!(
        migration.config.js.ignore_dependencies,
        vec![skif_config::IgnoreItem::new("some-loader").with_reason("false positive: loaded by the bundler")],
        "{}",
        migration.toml
    );
}

#[test]
fn a_trailing_knip_comment_is_kept_and_entries_without_one_stay_strings() {
    let migration = knip_file(
        "knip.ts",
        "export default {\n  ignoreBinaries: [\n    'docker', // installed on the CI image\n    'make',\n  ],\n  ignoreUnresolved: [/^virtual:/ /* generated by the bundler */],\n  ignoreDependencies: ['left-pad'],\n};\n",
    );
    let parsed = table(&migration);
    let binaries = parsed["js"]["ignore-binaries"].as_array().unwrap();
    assert_eq!(binaries[0]["name"].as_str(), Some("docker"), "{}", migration.toml);
    assert_eq!(binaries[0]["reason"].as_str(), Some("installed on the CI image"));
    assert_eq!(binaries[1].as_str(), Some("make"));
    let unresolved = parsed["js"]["ignore-unresolved"].as_array().unwrap();
    assert_eq!(
        unresolved[0]["name"].as_str(),
        Some("/^virtual:/"),
        "{}",
        migration.toml
    );
    assert_eq!(unresolved[0]["reason"].as_str(), Some("generated by the bundler"));
    assert_eq!(strings(&parsed["js"]["ignore-dependencies"]), ["left-pad"]);
    assert_eq!(
        migration.config.missing_ignore_reason().as_deref(),
        Some("js.ignore-dependencies[0] `left-pad`")
    );
}

#[test]
fn knip_jsonc_comments_become_reasons_of_the_list_they_are_in() {
    let migration = knip_file(
        "knip.jsonc",
        "{\n  // the whole file\n  \"ignoreDependencies\": [\"left-pad\"],\n  \"workspaces\": {\n    \"apps/web\": {\n      \"ignoreDependencies\": [\n        \"left-pad\" // kept for the legacy build\n      ]\n    }\n  }\n}\n",
    );
    let parsed = table(&migration);
    assert_eq!(
        strings(&parsed["js"]["ignore-dependencies"]),
        ["left-pad"],
        "{}",
        migration.toml
    );
    let web = parsed["js"]["workspaces"]["apps/web"]["ignore-dependencies"]
        .as_array()
        .unwrap();
    assert_eq!(
        web[0]["reason"].as_str(),
        Some("kept for the legacy build"),
        "{}",
        migration.toml
    );
}

#[test]
fn a_syncpack_comment_after_a_spread_is_not_written_above_the_wrong_group() {
    let mut input = ToolInput::new(Tool::Syncpack);
    input.config = Some(text(
        "syncpack.config.js",
        "const shared = [\n  { dependencies: ['lodash'], isIgnored: true },\n  { dependencies: ['chalk'], isIgnored: true },\n];\nmodule.exports = {\n  versionGroups: [\n    // first of all\n    { dependencies: ['react'], isIgnored: true },\n    ...shared,\n    // pinned\n    { dependencies: ['vue'], isIgnored: true },\n  ],\n};\n",
    ));
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    assert!(
        migration
            .toml
            .contains("# syncpack versionGroups[0]\n# first of all\n[[version-groups]]\n"),
        "{}",
        migration.toml
    );
    for index in 1..=3 {
        assert!(
            migration
                .toml
                .contains(&format!("# syncpack versionGroups[{index}]\n[[version-groups]]\n")),
            "{}",
            migration.toml
        );
    }
    assert!(!migration.toml.contains("pinned"), "{}", migration.toml);
}

#[test]
fn a_syncpack_yaml_comment_above_a_group_is_written_above_it() {
    let mut input = ToolInput::new(Tool::Syncpack);
    input.config = Some(text(
        ".syncpackrc.yaml",
        "versionGroups:\n  # react stays on 17 until the design system moves\n  - label: react\n    dependencies:\n      - react\n    isIgnored: true\n  - dependencies:\n      - lodash\n    isIgnored: true\n",
    ));
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    assert!(
        migration.toml.contains(
            "# syncpack versionGroups[0]: react\n# react stays on 17 until the design system moves\n[[version-groups]]\n"
        ),
        "{}",
        migration.toml
    );
    assert!(
        migration
            .toml
            .contains("# syncpack versionGroups[1]\n[[version-groups]]\n"),
        "{}",
        migration.toml
    );
}

const MERGED_PATHS: &str = "tsconfig paths";

#[test]
fn knip_in_a_workspace_notes_that_skif_does_not_merge_tsconfig_paths() {
    let mut input = ToolInput::new(Tool::Knip);
    input.config = Some(text("knip.json", r#"{ "entry": ["src/index.ts"] }"#));
    input.workspace_patterns = vec!["packages/*".to_owned()];
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    let merged = note(&migration, MERGED_PATHS);
    assert_eq!(merged.class, NoteClass::Approximate);
    assert_eq!(merged.tool, Tool::Knip);
    assert_eq!(merged.file, "knip.json");
    assert_eq!(
        merged.message,
        "knip merges the tsconfig paths of all workspaces; skif does not, so an import that only another package's \
         paths map is reported as unlisted-dependencies with detail ForeignTsconfigPaths"
    );
    assert!(
        merged
            .suggestion
            .as_deref()
            .is_some_and(|suggestion| suggestion.contains("tsconfig of the importing package")),
        "{merged:#?}"
    );
    assert!(!migration.needs_human(), "{:#?}", migration.notes);
}

#[test]
fn knip_workspaces_in_the_config_alone_note_the_unmerged_tsconfig_paths() {
    let mut input = ToolInput::new(Tool::Knip);
    input.config = Some(read("knip", "knip.ts"));
    let migration = migrate(&[input], &options(Profile::Full)).unwrap();
    assert_eq!(class(&migration, MERGED_PATHS), NoteClass::Approximate);
}

#[test]
fn knip_root_workspace_table_replaces_the_top_level_entry_project_and_paths() {
    let migration = knip(
        Some(
            r#"{ "entry": ["a.ts"], "project": ["**/*.ts"], "paths": { "@/*": ["./src/*"] }, "workspaces": { ".": { "entry": ["src/index.ts"] } } }"#,
        ),
        &[("knip", "knip")],
    );
    let doc = table(&migration);
    let js = doc["js"].as_table().unwrap();
    assert_eq!(strings(&js["entry"]), ["src/index.ts"]);
    assert!(!js.contains_key("project") && !js.contains_key("workspaces"), "{js:#?}");
    for key in ["entry", "project", "paths"] {
        let ignored = note(&migration, key);
        assert_eq!(ignored.class, NoteClass::Exact);
        assert_eq!(
            ignored.message,
            format!(
                "knip configures the root workspace with `workspaces[\".\"]` alone when it is set, so the top-level \
                 `{key}` has no effect; not written"
            )
        );
    }
}

#[test]
fn knip_top_level_ignore_and_ignore_files_apply_with_a_root_workspace_table() {
    let migration = knip(
        Some(
            r#"{ "ignore": ["gen/**"], "ignoreFiles": ["legacy/**"], "workspaces": { ".": { "entry": ["src/index.ts"] } } }"#,
        ),
        &[("knip", "knip")],
    );
    let doc = table(&migration);
    let js = doc["js"].as_table().unwrap();
    assert_eq!(strings(&js["ignore"]), ["gen/**"]);
    assert_eq!(strings(&js["ignore-files"]), ["legacy/**"]);
}

#[test]
fn knip_top_level_entry_applies_without_a_root_workspace_table() {
    let migration = knip(
        Some(r#"{ "entry": ["a.ts"], "workspaces": { "packages/*": { "entry": ["src/index.ts"] } } }"#),
        &[("knip", "knip")],
    );
    assert_eq!(strings(&table(&migration)["js"]["entry"]), ["a.ts"]);
}

#[test]
fn knip_in_a_single_package_project_says_nothing_about_tsconfig_paths() {
    for config in [
        r#"{ "entry": ["src/index.ts"] }"#,
        r#"{ "workspaces": { ".": { "entry": ["src/index.ts"] } } }"#,
        r#"{ "workspaces": { "./": { "entry": ["src/index.ts"] } } }"#,
    ] {
        let migration = knip(Some(config), &[("knip", "knip")]);
        assert!(
            !migration.notes.iter().any(|note| note.path == MERGED_PATHS),
            "{config}: {:#?}",
            migration.notes
        );
    }
    let member = knip(
        Some(r#"{ "workspaces": { ".": {}, "packages/*": { "entry": ["src/index.ts"] } } }"#),
        &[("knip", "knip")],
    );
    assert_eq!(class(&member, MERGED_PATHS), NoteClass::Approximate);
}
