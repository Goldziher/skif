//! `[js]` configuration parity with knip: regular expression entries, `ignore-unresolved`, `ignore-workspaces`,
//! per-workspace keys, the production marker, `ignore-members`, `ignore-exports-used-in-file`, `stale-js-config` and
//! `ignore-global-binaries`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::{Config, OverrideScope};
use skif_core::{Diagnostic, FileSystem, MemoryFs, Report, Severity};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

const MANIFEST: (&str, &str) = (
    "package.json",
    r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
);

fn lint(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    lint_anchored(files, toml, None)
}

/// [`lint`] with the `[js.workspaces]` keys matched from `anchor`, as for a root below the start of a run.
fn lint_anchored(files: &[(&str, &str)], toml: &str, anchor: Option<&str>) -> Vec<Diagnostic> {
    let (registry, workspace, config) = prepared(files, toml, anchor);
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

/// The whole report of linting `files` with `toml`.
fn report(files: &[(&str, &str)], toml: &str) -> Report {
    let (registry, workspace, config) = prepared(files, toml, None);
    run(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

/// The JS provider, the workspace of `files` with its keys anchored at `anchor`, and the configuration `toml`.
fn prepared(files: &[(&str, &str)], toml: &str, anchor: Option<&str>) -> (Registry, Workspace, Config) {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let mut config = Config::from_toml_str(toml).unwrap();
    // A resolved configuration scopes its overrides; this one is declared at the root, as `/ws/skif.toml` would be.
    for scoped in &mut config.overrides {
        let scope = OverrideScope::new(scoped, Path::new(ROOT), 0, Path::new(ROOT)).unwrap();
        scoped.scope = Some(scope);
    }
    let mut workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    workspace.set_workspace_keys_anchor(anchor.map(str::to_owned));
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let source = PluginSource::new(config.js.plugins.clone())
        .with_workspaces(&config.js.workspaces)
        .with_overrides(&config.overrides);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(source));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    (registry, workspace, config)
}

/// `path: message` of the diagnostics of `rule`, sorted.
fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    let mut found: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            format!(
                "{}: {}",
                d.location.path.display().to_string().replace('\\', "/"),
                d.message
            )
        })
        .collect();
    found.sort();
    found
}

fn paths(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    found(diagnostics, rule)
        .into_iter()
        .map(|line| line.split(": ").next().unwrap().to_owned())
        .collect()
}

fn none() -> Vec<String> {
    Vec::new()
}

fn two_packages(a_index: &'static str) -> Vec<(&'static str, &'static str)> {
    vec![
        MANIFEST,
        (
            "packages/a/package.json",
            r#"{"name":"@x/a","dependencies":{"left":"1"}}"#,
        ),
        (
            "packages/b/package.json",
            r#"{"name":"@x/b","dependencies":{"left":"1"}}"#,
        ),
        ("packages/a/src/index.ts", a_index),
        ("packages/b/src/index.ts", "export {};\n"),
    ]
}

#[test]
fn ignore_unresolved_takes_globs_regular_expressions_and_workspace_tables() {
    let files = two_packages("import \"./missing-one\";\nimport \"./missing-two\";\nimport \"./missing-three\";\n");
    assert_eq!(found(&lint(&files, ""), "unresolved-imports").len(), 3);

    let found_after = lint(
        &files,
        "[js]\nignore-unresolved = [\"/missing-one$/\"]\n[js.workspaces.\"packages/a\"]\nignore-unresolved = [\"./missing-two\"]\n",
    );
    assert_eq!(
        found(&found_after, "unresolved-imports"),
        ["packages/a/src/index.ts: cannot resolve `./missing-three`"]
    );
}

#[test]
fn ignore_dependencies_take_regular_expressions() {
    let files = vec![
        MANIFEST,
        (
            "packages/a/package.json",
            r#"{"name":"@x/a","dependencies":{"@org/one":"1","@org/two":"1","other":"1"}}"#,
        ),
        ("packages/a/src/index.ts", "export {};\n"),
    ];
    assert_eq!(found(&lint(&files, ""), "unused-dependencies").len(), 3);
    for pattern in ["/^@org\\\\//", "^@org/", "@org/.*"] {
        let diagnostics = lint(&files, &format!("[js]\nignore-dependencies = [\"{pattern}\"]\n"));
        let unused = found(&diagnostics, "unused-dependencies");
        assert_eq!(unused.len(), 1, "{pattern}: {unused:?}");
        assert!(unused[0].contains("other"), "{pattern}: {unused:?}");
    }
}

#[test]
fn the_production_marker_limits_an_ignore_entry_to_production() {
    let files = vec![
        MANIFEST,
        (
            "packages/a/package.json",
            r#"{"name":"@x/a","dependencies":{"left":"1"}}"#,
        ),
        ("packages/a/src/index.ts", "export {};\n"),
    ];
    let normal = lint(&files, "[js]\nignore-dependencies = [\"left!\"]\n");
    assert_eq!(found(&normal, "unused-dependencies").len(), 1);
    let production = lint(&files, "[js]\nproduction = true\nignore-dependencies = [\"left!\"]\n");
    assert_eq!(found(&production, "unused-dependencies"), none());
}

#[test]
fn the_production_marker_selects_entries_in_a_production_run() {
    let files = vec![
        MANIFEST,
        ("packages/a/package.json", r#"{"name":"@x/a"}"#),
        ("packages/a/src/index.ts", "export {};\n"),
        ("packages/a/src/shipped.ts", "export {};\n"),
        ("packages/a/src/extra.ts", "export {};\n"),
    ];
    let entry = "entry = [\"src/index.ts\", \"src/shipped.ts!\", \"src/extra.ts\"]\n";
    let normal = lint(&files, &format!("[js.workspaces.\"packages/a\"]\n{entry}"));
    assert_eq!(paths(&normal, "unused-files"), none());
    let production = lint(
        &files,
        &format!("[js]\nproduction = true\n[js.workspaces.\"packages/a\"]\n{entry}"),
    );
    // Entries without the marker run in development only: they are no project files in a production run.
    assert_eq!(paths(&production, "unused-files"), none());
}

#[test]
fn a_production_run_keeps_unmarked_entries_out_of_the_project() {
    let files = vec![
        MANIFEST,
        ("packages/a/package.json", r#"{"name":"@x/a"}"#),
        ("packages/a/src/index.ts", "export {};\n"),
        ("packages/a/scripts/build.ts", "export {};\n"),
        ("packages/a/src/orphan.ts", "export {};\n"),
    ];
    let entry = "entry = [\"src/index.ts!\", \"scripts/*.ts\"]\n";
    let production = lint(
        &files,
        &format!("[js]\nproduction = true\n[js.workspaces.\"packages/a\"]\n{entry}"),
    );
    assert_eq!(paths(&production, "unused-files"), ["packages/a/src/orphan.ts"]);
}

#[test]
fn ignore_workspaces_skips_the_analysis_of_matching_packages() {
    let files = two_packages("export {};\n");
    assert_eq!(found(&lint(&files, ""), "unused-dependencies").len(), 2);
    let ignored = lint(&files, "[js]\nignore-workspaces = [\"packages/b\"]\n");
    assert_eq!(paths(&ignored, "unused-dependencies"), ["packages/a/package.json"]);
}

#[test]
fn a_package_whose_only_entry_is_already_an_entry_of_another_is_judged() {
    // `packages/b` has no file of its own; its entry is the entry of `packages/a`, found first.
    let files = vec![
        MANIFEST,
        ("packages/a/package.json", r#"{"name":"@x/a"}"#),
        ("packages/a/src/index.ts", "export {};\n"),
        (
            "packages/b/package.json",
            r#"{"name":"@x/b","dependencies":{"left":"1"}}"#,
        ),
    ];
    let diagnostics = lint(
        &files,
        "[js.workspaces.\"packages/b\"]\nentry = [\"../a/src/index.ts\"]\n",
    );
    assert_eq!(paths(&diagnostics, "unused-dependencies"), ["packages/b/package.json"]);
}

fn nested_packages() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/**"]}"#,
        ),
        ("packages/a/package.json", r#"{"name":"@x/a"}"#),
        ("packages/a/src/index.ts", "export {};\n"),
        ("packages/a/nested/package.json", r#"{"name":"@x/nested"}"#),
        ("packages/a/nested/src/index.ts", "export {};\n"),
        ("packages/a/nested/src/use.ts", "export {};\n"),
    ]
}

#[test]
fn a_star_in_ignore_workspaces_does_not_cross_a_slash() {
    let files = nested_packages();
    let ignored = lint(&files, "[js]\nignore-workspaces = [\"packages/*\"]\n");
    assert_eq!(paths(&ignored, "unused-files"), ["packages/a/nested/src/use.ts"]);
    let all = lint(&files, "[js]\nignore-workspaces = [\"packages/**\"]\n");
    assert_eq!(paths(&all, "unused-files"), none());
}

#[test]
fn a_star_in_a_workspace_key_does_not_select_nested_packages() {
    let files = nested_packages();
    let single = lint(&files, "[js.workspaces.\"packages/*\"]\nentry = [\"src/*.ts\"]\n");
    assert_eq!(paths(&single, "unused-files"), ["packages/a/nested/src/use.ts"]);
    let deep = lint(&files, "[js.workspaces.\"packages/**\"]\nentry = [\"src/*.ts\"]\n");
    assert_eq!(paths(&deep, "unused-files"), none());
}

#[test]
fn the_root_workspace_key_configures_the_root_package() {
    let files = vec![
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"],"dependencies":{"rootdep":"1"}}"#,
        ),
        ("packages/a/package.json", r#"{"name":"@x/a"}"#),
        ("packages/a/src/index.ts", "export {};\n"),
    ];
    assert_eq!(paths(&lint(&files, ""), "unused-dependencies"), ["package.json"]);
    let configured = lint(&files, "[js.workspaces.\".\"]\nignore-dependencies = [\"rootdep\"]\n");
    assert_eq!(found(&configured, "unused-dependencies"), none());
}

#[test]
fn paths_resolve_aliases_for_one_workspace() {
    let files = two_packages("import { x } from \"@lib/util\";\nexport const y = x;\n");
    let mut files = files;
    files.push(("packages/a/src/lib/util.ts", "export const x = 1;\n"));
    let plain = lint(&files, "");
    assert_eq!(found(&plain, "unlisted-dependencies").len(), 1, "{plain:#?}");

    let aliased = lint(
        &files,
        "[js.workspaces.\"packages/a\".paths]\n\"@lib/*\" = [\"./src/lib/*\"]\n",
    );
    assert_eq!(found(&aliased, "unlisted-dependencies"), none(), "{aliased:#?}");
    assert_eq!(found(&aliased, "unresolved-imports"), none(), "{aliased:#?}");
    assert_eq!(paths(&aliased, "unused-files"), none(), "{aliased:#?}");
}

#[test]
fn include_entry_exports_is_set_per_workspace() {
    let files = {
        let mut files = two_packages("export const api = 1;\n");
        files[4] = ("packages/b/src/index.ts", "export const api = 1;\n");
        files
    };
    assert_eq!(paths(&lint(&files, ""), "unused-exports"), none());
    let scoped = lint(&files, "[js.workspaces.\"packages/a\"]\ninclude-entry-exports = true\n");
    assert_eq!(paths(&scoped, "unused-exports"), ["packages/a/src/index.ts"]);
}

fn used_in_file_files() -> Vec<(&'static str, &'static str)> {
    let mut files = two_packages("import { other } from \"./util\";\nconsole.log(other);\n");
    files.push((
        "packages/a/src/util.ts",
        "export const helper = 1;\nconsole.log(helper);\nexport const other = 2;\n",
    ));
    files
}

#[test]
fn exports_used_in_their_own_file_can_be_ignored() {
    let files = used_in_file_files();
    assert_eq!(paths(&lint(&files, ""), "unused-exports"), ["packages/a/src/util.ts"]);
    let all = lint(&files, "[js]\nignore-exports-used-in-file = true\n");
    assert_eq!(paths(&all, "unused-exports"), none());
    let variables = lint(&files, "[js]\nignore-exports-used-in-file = { variable = true }\n");
    assert_eq!(paths(&variables, "unused-exports"), none());
    let functions = lint(&files, "[js]\nignore-exports-used-in-file = { function = true }\n");
    assert_eq!(paths(&functions, "unused-exports"), ["packages/a/src/util.ts"]);
    let workspace = lint(
        &files,
        "[js.workspaces.\"packages/a\"]\nignore-exports-used-in-file = true\n",
    );
    assert_eq!(paths(&workspace, "unused-exports"), none());
}

#[test]
fn ignore_members_hides_enum_members() {
    let mut files = two_packages("import { E } from \"./util\";\nconsole.log(E.A);\n");
    files.push(("packages/a/src/util.ts", "export enum E {\n  A,\n  B,\n}\n"));
    let plain = lint(&files, "");
    assert_eq!(found(&plain, "unused-enum-members").len(), 1, "{plain:#?}");
    for pattern in ["B", "/^B$/", "[B]"] {
        let ignored = lint(&files, &format!("[js]\nignore-members = [\"{pattern}\"]\n"));
        assert_eq!(
            found(&ignored, "unused-enum-members"),
            none(),
            "{pattern}: {ignored:#?}"
        );
    }
}

#[test]
fn plugin_options_apply_to_one_workspace() {
    let mut files = two_packages("export {};\n");
    files.push(("packages/a/src/extra.ts", "export {};\n"));
    files.push(("packages/b/src/extra.ts", "export {};\n"));
    assert_eq!(
        paths(&lint(&files, ""), "unused-files"),
        ["packages/a/src/extra.ts", "packages/b/src/extra.ts"]
    );
    let scoped = lint(
        &files,
        "[js.workspaces.\"packages/a\".plugins]\nvitest = { enabled = true, entry = [\"src/extra.ts\"] }\n",
    );
    assert_eq!(paths(&scoped, "unused-files"), ["packages/b/src/extra.ts"]);
}

fn messages(diagnostics: &[Diagnostic]) -> Vec<String> {
    let mut messages: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == "stale-js-config")
        .map(|d| d.message.clone())
        .collect();
    messages.sort();
    messages
}

#[test]
fn stale_js_config_reports_entries_that_matched_nothing() {
    let files = two_packages("export {};\n");
    let diagnostics = lint(
        &files,
        "[js]\nignore-dependencies = [\"left\", \"nothing-here\"]\nignore-binaries = [\"ghost\"]\nignore-unresolved = [\"./gone\"]\nignore-workspaces = [\"packages/b\", \"nope/*\"]\n\
         [js.workspaces.\"packages/a\"]\nignore-dependencies = [\"zzz\"]\n\
         [js.workspaces.\"packages/missing\"]\nentry = [\"x.ts\"]\n",
    );
    assert_eq!(
        messages(&diagnostics),
        [
            "`./gone` in `js.ignore-unresolved` matched nothing",
            "`ghost` in `js.ignore-binaries` matched nothing",
            "`nope/*` in `js.ignore-workspaces` matched nothing",
            "`nothing-here` in `js.ignore-dependencies` matched nothing",
            "`packages/missing` in `js.workspaces` matched nothing",
            "`zzz` in `js.workspaces.\"packages/a\".ignore-dependencies` matched nothing",
        ]
    );
    assert!(
        diagnostics
            .iter()
            .filter(|d| d.rule.as_str() == "stale-js-config")
            .all(|d| d.severity == Severity::Warning)
    );
}

#[test]
fn anchored_workspace_keys_select_packages_by_their_directory_below_the_start() {
    let files = two_packages("export {};\n");
    let toml = "[js]\nignore-workspaces = [\"repo/packages/b\"]\n\
                [js.workspaces.\".\"]\nentry = [\"gone.ts\"]\n\
                [js.workspaces.\"repo/packages/a\"]\nignore-dependencies = [\"left\", \"zzz\"]\n";
    let diagnostics = lint_anchored(&files, toml, Some("repo"));
    // `packages/a` takes the `repo/packages/a` table, which ignores `left`, and `packages/b` is skipped.
    assert_eq!(found(&diagnostics, "unused-dependencies"), none());
    // `"."` is the start directory, not this root, so its table applies to nothing here.
    assert_eq!(
        messages(&diagnostics),
        [
            "`.` in `js.workspaces` matched nothing",
            "`zzz` in `js.workspaces.\"repo/packages/a\".ignore-dependencies` matched nothing",
        ]
    );
}

#[test]
fn stale_js_config_records_every_entry_it_judged_under_the_key_it_is_written_at() {
    let files = two_packages("export {};\n");
    let report = report(
        &files,
        "[js]\nignore-dependencies = [\"left\", \"zzz\"]\n[js.workspaces.\"packages/*\"]\nignore-binaries = [\"tsc\"]\n",
    );
    let file = &report.config_judgements[0].file;
    assert!(report.config_judgements.iter().all(|judgement| &judgement.file == file));
    let judged: Vec<(&str, &str, bool, &str)> = report
        .config_judgements
        .iter()
        .map(|judgement| {
            (
                judgement.key.as_str(),
                judgement.entry.as_str(),
                judgement.stale,
                judgement.message.as_str(),
            )
        })
        .collect();
    // `left` silences the unused `left` of both packages; the glob table's entry is judged in each package it
    // selects, under the key as written, and named by the package in the finding.
    assert_eq!(
        judged,
        [
            (
                "js.ignore-dependencies",
                "left",
                false,
                "`left` in `js.ignore-dependencies` matched nothing"
            ),
            (
                "js.ignore-dependencies",
                "zzz",
                true,
                "`zzz` in `js.ignore-dependencies` matched nothing"
            ),
            (
                "js.workspaces",
                "packages/*",
                false,
                "`packages/*` in `js.workspaces` matched nothing"
            ),
            (
                "js.workspaces.\"packages/*\".ignore-binaries",
                "tsc",
                true,
                "`tsc` in `js.workspaces.\"packages/a\".ignore-binaries` matched nothing"
            ),
            (
                "js.workspaces.\"packages/*\".ignore-binaries",
                "tsc",
                true,
                "`tsc` in `js.workspaces.\"packages/b\".ignore-binaries` matched nothing"
            ),
        ]
    );
}

#[test]
fn a_list_stale_js_config_cannot_judge_records_nothing() {
    let files = two_packages("export {};\n");
    let toml = "[js]\nignore-dependencies = [\"zzz\"]\n[rules]\nunused-dependencies = \"off\"\n";
    assert!(report(&files, toml).config_judgements.is_empty());
}

#[test]
fn findings_spell_an_overridden_package_by_its_directory_below_the_start() {
    let files = two_packages("export {};\n");
    let toml = "[[overrides]]\npackages = [\"./packages/a\"]\n[overrides.js]\nentry = [\"gone.ts\"]\n";
    let diagnostics = lint_anchored(&files, toml, Some("repo"));
    // The pattern comes from an override, not a `[js.workspaces]` table, so the key names the package's directory
    // as a key of the file would select it.
    assert_eq!(
        messages(&diagnostics),
        ["`gone.ts` in `js.workspaces.\"repo/packages/a\".entry` matched nothing"]
    );
}

#[test]
fn findings_name_a_glob_table_below_the_start_by_its_key_as_written() {
    let files = two_packages("export {};\n");
    let toml = "[js.workspaces.\"repo/packages/*\"]\nentry = [\"gone.ts\"]\n";
    let diagnostics = lint_anchored(&files, toml, Some("repo"));
    // The pattern matches in neither package the glob selects, so it is reported once, under the key of the file.
    assert_eq!(
        messages(&diagnostics),
        ["`gone.ts` in `js.workspaces.\"repo/packages/*\".entry` matched nothing"]
    );
}

#[test]
fn stale_js_config_is_silent_when_every_entry_matched() {
    let mut files = two_packages("import \"./gone\";\n");
    files.push(("index.ts", "export {};\n"));
    let diagnostics = lint(
        &files,
        "[js]\nignore-dependencies = [\"left\"]\nignore-unresolved = [\"./gone\"]\n[js.workspaces.\".\"]\nentry = [\"index.ts\"]\n",
    );
    assert_eq!(messages(&diagnostics), none());
}

#[test]
fn stale_js_config_judges_ignore_dependencies_over_the_packages_whose_dependencies_were_checked() {
    // `packages/b` has no code, so its dependencies are not judged; an entry matching one of them may be needed.
    let files = vec![
        MANIFEST,
        (
            "packages/a/package.json",
            r#"{"name":"@x/a","dependencies":{"left":"1"}}"#,
        ),
        ("packages/a/src/index.ts", "import \"left\";\n"),
        (
            "packages/b/package.json",
            r#"{"name":"@x/b","dependencies":{"right":"1"}}"#,
        ),
    ];
    let diagnostics = lint(
        &files,
        "[js]\nignore-dependencies = [\"nothing-here\", \"right\"]\n[js.workspaces.\"packages/a\"]\nignore-dependencies = [\"zzz\"]\n",
    );
    assert_eq!(
        messages(&diagnostics),
        [
            "`nothing-here` in `js.ignore-dependencies` matched nothing",
            "`zzz` in `js.workspaces.\"packages/a\".ignore-dependencies` matched nothing",
        ]
    );
}

#[test]
fn ignore_entries_with_a_reason_suppress_and_are_judged_like_strings() {
    let files = vec![
        MANIFEST,
        (
            "packages/a/package.json",
            r#"{"name":"@x/a","dependencies":{"some-loader":"1","left":"1"}}"#,
        ),
        ("packages/a/src/index.ts", "import \"./missing\";\n"),
    ];
    let before = lint(&files, "");
    assert_eq!(found(&before, "unused-dependencies").len(), 2);
    assert_eq!(found(&before, "unresolved-imports").len(), 1);

    let diagnostics = lint(
        &files,
        r#"
[js]
ignore-unresolved = [{ name = "./missing", reason = "generated before the build" }]
ignore-dependencies = [{ name = "nothing-here", reason = "kept for a package that is gone" }]

[js.workspaces."packages/a"]
ignore-dependencies = [
  "left",
  { name = "some-loader", reason = "false positive: loaded by the bundler from a config string" },
]
"#,
    );
    assert_eq!(found(&diagnostics, "unused-dependencies"), none());
    assert_eq!(found(&diagnostics, "unresolved-imports"), none());
    assert_eq!(
        messages(&diagnostics),
        ["`nothing-here` in `js.ignore-dependencies` matched nothing"]
    );
}

#[test]
fn stale_js_config_can_be_raised_to_an_error() {
    let files = two_packages("export {};\n");
    let diagnostics = lint(
        &files,
        "[rules]\nstale-js-config = \"error\"\n[js]\nignore-binaries = [\"ghost\"]\n",
    );
    let stale: Vec<&Diagnostic> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == "stale-js-config")
        .collect();
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].severity, Severity::Error);
}

#[test]
fn stale_js_config_does_not_judge_a_list_whose_rules_are_off() {
    let files = two_packages("export {};\n");
    let diagnostics = lint(
        &files,
        "[rules]\nunused-dev-dependencies = \"off\"\n[js]\nignore-dependencies = [\"nothing-here\"]\n",
    );
    assert_eq!(messages(&diagnostics), none());
}

#[test]
fn stale_js_config_does_not_judge_ignore_binaries_when_unlisted_binaries_is_off() {
    let files = two_packages("export {};\n");
    let toml = "[js]\nignore-binaries = [\"ghost\"]\n[js.workspaces.\"packages/a\"]\nignore-binaries = [\"phantom\"]\n";
    let on = lint(&files, toml);
    assert_eq!(
        messages(&on),
        [
            "`ghost` in `js.ignore-binaries` matched nothing",
            "`phantom` in `js.workspaces.\"packages/a\".ignore-binaries` matched nothing"
        ]
    );
    let off = lint(&files, &format!("[rules]\nunlisted-binaries = \"off\"\n{toml}"));
    assert_eq!(messages(&off), none());
}

fn validate(toml: &str) -> Result<(), skif_config::ConfigError> {
    let registry = Registry::new(vec![Box::new(JsProvider::on_disk())]).unwrap();
    registry.validate_config(&Config::from_toml_str(toml).unwrap())
}

#[test]
fn invalid_js_configuration_is_rejected_when_the_configuration_is_checked() {
    assert!(validate("[js]\nignore-dependencies = [\"/^@org\\\\/.+$/\", \"@types/*\"]\n").is_ok());
    let regex = validate("[js]\nignore-dependencies = [\"/(/\"]\n")
        .unwrap_err()
        .to_string();
    assert!(regex.contains("js.ignore-dependencies[0]"), "{regex}");
    let glob = validate("[js.workspaces.\"a\"]\nentry = [\"src/[\"]\n")
        .unwrap_err()
        .to_string();
    assert!(glob.contains("js.workspaces.\"a\".entry[0]"), "{glob}");
    let cycles = validate("[js.cycles]\nallow = [[\"src/a.ts\"], []]\n")
        .unwrap_err()
        .to_string();
    assert!(cycles.contains("js.cycles.allow[1]"), "{cycles}");
    let cycle_path = validate("[js.cycles]\nallow = [[\"src/a.ts\", \" \"]]\n")
        .unwrap_err()
        .to_string();
    assert!(cycle_path.contains("js.cycles.allow[0]"), "{cycle_path}");
    let plugin = validate("[js.workspaces.\"a\".plugins]\nvitset = true\n")
        .unwrap_err()
        .to_string();
    assert!(
        plugin.contains("unknown plugin `vitset`") && plugin.contains("did you mean `vit"),
        "{plugin}"
    );
}

#[test]
fn stale_js_config_reports_entry_project_and_ignore_patterns_that_match_no_file() {
    let mut files = two_packages("export {};\n");
    files.push(("index.ts", "export {};\n"));
    files.push(("packages/a/src/extra.ts", "export {};\n"));
    // Files the analysis never reads: gitignored output, JSON and HTML. Patterns for them are not empty.
    files.push((".gitignore", "dist\n"));
    files.push(("dist/index.js", "export {};\n"));
    files.push(("index.html", "<script type=\"module\" src=\"./index.ts\"></script>\n"));
    let diagnostics = lint(
        &files,
        "[js]\nentry = [\"index.ts\", \"main.ts\", \"!skip.ts\", \"index.html\"]\nproject = [\"**/*.ts\", \"lib/**\"]\n\
         ignore = [\"packages/a/src/extra.ts\", \"generated/**\", \"dist/**\", \"**/*.json\"]\n\
         ignore-files = [\"dist/**\", \"gone/**\"]\n\
         [js.workspaces.\"packages/*\"]\nentry = [\"src/index.ts\", \"src/cli.ts!\"]\nproject = [\"src/**/*.ts\"]\nignore = [\"src/extra.ts\", \"src/gone/**\"]\n",
    );
    // `dist` is gitignored, so only the walk of the disk finds its file.
    assert!(
        !paths(&diagnostics, "unused-files")
            .iter()
            .any(|path| path.starts_with("dist/"))
    );
    insta::assert_snapshot!(messages(&diagnostics).join("\n"));
}

fn cycle_files() -> Vec<(&'static str, &'static str)> {
    vec![
        ("package.json", r#"{"name":"app","private":true,"main":"src/index.ts"}"#),
        (
            "src/index.ts",
            "import { a } from \"./a\";\nimport { c } from \"./c\";\nconsole.log(a, c);\n",
        ),
        ("src/a.ts", "import { b } from \"./b\";\nexport const a = () => b;\n"),
        ("src/b.ts", "import { a } from \"./a\";\nexport const b = () => a;\n"),
        ("src/c.ts", "export const c = () => import(\"./d\");\n"),
        ("src/d.ts", "import { c } from \"./c\";\nexport const d = c;\n"),
    ]
}

#[test]
fn cycles_allow_and_dynamic_imports() {
    let on = "[rules]\ncircular-imports = \"warn\"\n";
    let cycles = |toml: &str| found(&lint(&cycle_files(), &format!("{on}{toml}")), "circular-imports");
    assert_eq!(cycles(""), ["src/a.ts: 2 modules import each other in a cycle"]);
    assert_eq!(
        cycles("[js.cycles]\ndynamic-imports = true\n"),
        [
            "src/a.ts: 2 modules import each other in a cycle",
            "src/c.ts: 2 modules import each other in a cycle",
        ]
    );
    assert_eq!(
        cycles("[js.cycles]\ndynamic-imports = true\nallow = [[\"./src/b.ts\", \"src/a.ts\"], [\"src/c.ts\"]]\n"),
        ["src/c.ts: 2 modules import each other in a cycle"]
    );
}

#[test]
fn ignore_files_hides_files_from_unused_files_only() {
    let mut files = two_packages("export const unusedExport = 1;\n");
    files.push(("packages/a/src/legacy.ts", "export const old = 1;\n"));
    files.push(("packages/b/src/orphan.ts", "export {};\n"));
    files.push(("packages/b/src/keep.ts", "export {};\n"));
    let before = lint(&files, "");
    assert_eq!(
        paths(&before, "unused-files"),
        [
            "packages/a/src/legacy.ts",
            "packages/b/src/keep.ts",
            "packages/b/src/orphan.ts"
        ]
    );
    let after = lint(
        &files,
        "[js]\nignore-files = [\"packages/a/src/legacy.ts\", \"nowhere/**\"]\n\
         [js.workspaces.\"packages/b\"]\nignore-files = [\"src/keep.ts\", \"src/gone.ts\"]\n",
    );
    assert_eq!(paths(&after, "unused-files"), ["packages/b/src/orphan.ts"]);
    assert_eq!(
        messages(&after),
        [
            "`nowhere/**` in `js.ignore-files` matched nothing",
            "`src/gone.ts` in `js.workspaces.\"packages/b\".ignore-files` matched nothing",
        ]
    );
}

#[test]
fn ignore_global_binaries_false_reports_system_binaries() {
    let files = [
        (
            "package.json",
            r#"{"name":"app","scripts":{"clean":"rm -rf out && cd out && echo done","fetch":"curl -o x y && git pull","x":"npx jq ."},"devDependencies":{"rimraf":"1"}}"#,
        ),
        ("index.ts", "export {};\n"),
    ];
    assert_eq!(found(&lint(&files, ""), "unlisted-binaries"), Vec::<String>::new());
    let reported = found(
        &lint(
            &files,
            "[js]\nignore-global-binaries = false\nignore-binaries = [\"git\"]\n",
        ),
        "unlisted-binaries",
    );
    insta::assert_snapshot!(reported.join("\n"));
}
