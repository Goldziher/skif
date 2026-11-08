//! `[js]` configuration parity with knip: regular expression entries, `ignore-unresolved`, `ignore-workspaces`,
//! per-workspace keys, the production marker, `ignore-members`, `ignore-exports-used-in-file` and `stale-js-config`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs, Severity};
use skif_js::{JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

const MANIFEST: (&str, &str) = (
    "package.json",
    r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
);

fn lint(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let source = PluginSource::new(config.js.plugins.clone())
        .with_workspaces(&config.js.workspaces)
        .with_overrides(&config.overrides);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(source));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap()
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
