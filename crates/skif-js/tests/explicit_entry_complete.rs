//! A plugin config whose entries are computed (an rspack config exporting a function) leaves a package incomplete,
//! unless the package names its entries itself: an explicit `entry` that applies in the run mode completes it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::{Ceiling, Config, ConfigCache, ConfigLoader};
use skif_core::{
    Diagnostic, FileSystem, IncompleteCause, IncompleteEntries, IncompletePackage, IncompleteScope, MemoryFs,
};
use skif_js::{InputRequest, InputSource, JsAnalysis, JsProvider, PluginSource, WorkspaceInputs};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

const FILES: [(&str, &str); 5] = [
    (
        "package.json",
        r#"{"name":"web","version":"1.0.0","private":true,"scripts":{"build":"rspack build"},"dependencies":{"left-pad":"1.0.0"},"devDependencies":{"@rspack/cli":"1.0.0"}}"#,
    ),
    (
        "rspack.config.js",
        "const fs = require('fs');\n\
         module.exports = (env) => ({\n\
           entry: Object.fromEntries(fs.readdirSync('src').map((f) => [f, `./src/${f}`])),\n\
         });\n",
    ),
    ("src/main.js", "import { used } from './lib.js';\nconsole.log(used);\n"),
    ("src/lib.js", "export const used = 1;\nexport const unusedExport = 2;\n"),
    ("src/orphan.js", "export const orphan = 3;\n"),
];

fn run(toml: &str) -> (Vec<IncompletePackage>, Vec<Diagnostic>) {
    run_files(&FILES, toml)
}

fn run_files(files: &[(&str, &str)], toml: &str) -> (Vec<IncompletePackage>, Vec<Diagnostic>) {
    let memory = files.iter().fold(
        MemoryFs::new().with_file(Path::new(ROOT).join("skif.toml"), toml),
        |memory, (path, text)| memory.with_file(Path::new(ROOT).join(path), text),
    );
    let config = ConfigLoader::new(&memory, &ConfigCache::new())
        .resolve(Path::new(ROOT), &Ceiling::RootOnly)
        .unwrap()
        .config;
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let source = || {
        PluginSource::new(config.js.plugins.clone())
            .with_workspaces(&config.js.workspaces)
            .with_overrides(&config.overrides)
    };
    let sources: Vec<Arc<dyn InputSource>> = vec![Arc::new(source())];
    let incomplete = JsAnalysis::build(&workspace, &config, &fs, &sources, true).incomplete_packages();
    let provider = JsProvider::new(fs).with_input_source(Arc::new(source()));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    let diagnostics = run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap();
    (incomplete, diagnostics)
}

fn found(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    let mut found: Vec<String> = diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| d.location.path.display().to_string())
        .collect();
    found.sort();
    found
}

#[test]
fn an_explicit_production_entry_completes_a_package_with_computed_entries() {
    let (incomplete, diagnostics) = run("[js]\nproduction = true\nentry = [\"src/main.js!\"]\n");
    assert_eq!(incomplete, Vec::new());
    assert_eq!(found(&diagnostics, "unused-files"), ["src/orphan.js"]);
    assert_eq!(found(&diagnostics, "unused-exports"), ["src/lib.js"]);
}

#[test]
fn a_plain_entry_does_not_complete_a_production_run() {
    let (incomplete, _) = run("[js]\nproduction = true\nentry = [\"src/main.js\"]\n");
    assert_eq!(incomplete.len(), 1);
}

#[test]
fn without_an_entry_the_package_stays_incomplete() {
    let (incomplete, diagnostics) = run("[js]\nproduction = true\n");
    assert_eq!(incomplete.len(), 1);
    assert_eq!(incomplete[0].reasons[0].cause, IncompleteCause::ComputedEntries);
    assert_eq!(found(&diagnostics, "unused-files"), Vec::<String>::new());
}

#[test]
fn a_plain_entry_completes_a_normal_run() {
    let (incomplete, _) = run("[js]\nentry = [\"src/main.js\"]\n");
    assert_eq!(incomplete, Vec::new());
}

#[test]
fn an_unused_dependency_is_reported_once_the_package_is_complete() {
    let (_, diagnostics) = run("[js]\nproduction = true\nentry = [\"src/main.js!\"]\n");
    assert_eq!(found(&diagnostics, "unused-dependencies"), ["package.json"]);
    let (_, diagnostics) = run("[js]\nproduction = true\n");
    assert_eq!(found(&diagnostics, "unused-dependencies"), Vec::<String>::new());
}

#[test]
fn a_stale_entry_naming_no_file_of_the_package_leaves_it_as_incomplete_as_no_entry() {
    let (without, _) = run("[js]\nproduction = true\n");
    let (stale, diagnostics) = run("[js]\nproduction = true\nentry = [\"src/old.js!\"]\n");
    assert_eq!(stale, without);
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].scope, IncompleteScope::Package);
    let reasons: Vec<_> = stale[0]
        .reasons
        .iter()
        .map(|r| (r.cause, r.plugin.as_deref(), r.file.as_deref()))
        .collect();
    assert_eq!(
        reasons,
        [(
            IncompleteCause::ComputedEntries,
            Some("rspack"),
            Some(Path::new("rspack.config.js"))
        )]
    );
    assert_eq!(found(&diagnostics, "unused-files"), Vec::<String>::new());
}

/// A source whose only input is a computed-entries reason that can name files below `src` only, as a tool that looks
/// for entries by convention would report.
struct NarrowedSource;

impl InputSource for NarrowedSource {
    fn inputs(&self, _request: &InputRequest<'_>) -> WorkspaceInputs {
        let mut inputs = WorkspaceInputs::default();
        inputs.entries_incomplete = true;
        inputs.incomplete_scope = IncompleteScope::Within(vec!["src/**".to_owned()]);
        inputs.incomplete_reasons = vec![IncompleteEntries::new(IncompleteCause::ComputedEntries)];
        inputs
    }
}

fn narrowed(toml: &str) -> Vec<IncompletePackage> {
    let memory = FILES.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let sources: Vec<Arc<dyn InputSource>> = vec![Arc::new(NarrowedSource)];
    JsAnalysis::build(&workspace, &config, &fs, &sources, true).incomplete_packages()
}

#[test]
fn a_stale_entry_restores_the_narrowed_scope_of_the_config() {
    let without = narrowed("[js]\nproduction = true\n");
    assert_eq!(without.len(), 1);
    assert_eq!(without[0].scope, IncompleteScope::Within(vec!["src/**".to_owned()]));
    let stale = narrowed("[js]\nproduction = true\nentry = [\"src/old.js!\"]\n");
    assert_eq!(stale, without);
    let complete = narrowed("[js]\nproduction = true\nentry = [\"src/main.js!\"]\n");
    assert_eq!(complete, Vec::new());
}

#[test]
fn empty_and_negated_entries_do_not_complete_the_package() {
    for toml in [
        "[js]\nentry = [\"\", \"  \"]\n",
        "[js]\nentry = [\"!src/main.js\"]\n",
        "[js]\nproduction = true\nentry = [\"!\", \" !\"]\n",
    ] {
        let (incomplete, _) = run(toml);
        assert_eq!(incomplete.len(), 1, "{toml}");
    }
}

#[test]
fn another_cause_keeps_the_package_incomplete() {
    let mut files = FILES.to_vec();
    files.push((
        "vitest.config.ts",
        "import base from '@acme/preset';\nexport default { ...base, test: {} };\n",
    ));
    files[0].1 =
        r#"{"name":"web","version":"1.0.0","private":true,"devDependencies":{"@rspack/cli":"1.0.0","vitest":"3.0.0"}}"#;
    let (incomplete, _) = run_files(&files, "[js]\nproduction = true\nentry = [\"src/main.js!\"]\n");
    assert_eq!(incomplete.len(), 1);
    let causes: Vec<IncompleteCause> = incomplete[0].reasons.iter().map(|reason| reason.cause).collect();
    assert!(causes.contains(&IncompleteCause::ComputedEntries), "{causes:?}");
    assert!(causes.len() > 1, "{causes:?}");
}

const MEMBERS: [(&str, &str); 6] = [
    (
        "package.json",
        r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
    ),
    (
        "packages/web/package.json",
        r#"{"name":"web","version":"1.0.0","private":true,"devDependencies":{"@rspack/cli":"1.0.0"}}"#,
    ),
    (
        "packages/web/rspack.config.js",
        "module.exports = (env) => ({ entry: Object.fromEntries(Object.keys(env).map((k) => [k, k])) });\n",
    ),
    ("packages/web/src/main.js", "export const main = 1;\n"),
    ("packages/web/src/orphan.js", "export const orphan = 3;\n"),
    ("src/index.js", "console.log(1);\n"),
];

#[test]
fn an_entry_of_the_workspace_table_completes_its_package_but_a_top_level_one_does_not() {
    let (incomplete, _) = run_files(
        &MEMBERS,
        "[js]\nproduction = true\n\n[js.workspaces.\"packages/web\"]\nentry = [\"src/main.js!\"]\n",
    );
    assert_eq!(incomplete, Vec::new());
    let (incomplete, _) = run_files(&MEMBERS, "[js]\nproduction = true\nentry = [\"src/main.js!\"]\n");
    assert_eq!(incomplete.len(), 1);
    assert_eq!(incomplete[0].name, "web");
}

#[test]
fn a_marked_entry_completes_a_normal_run_and_a_dot_slash_one_a_production_run() {
    let (incomplete, _) = run("[js]\nentry = [\"src/main.js!\"]\n");
    assert_eq!(incomplete, Vec::new());
    let (incomplete, _) = run("[js]\nproduction = true\nentry = [\"./src/main.js!\"]\n");
    assert_eq!(incomplete, Vec::new());
}

#[test]
fn a_marker_followed_by_whitespace_is_not_a_production_entry() {
    let (incomplete, _) = run("[js]\nproduction = true\nentry = [\"src/main.js! \"]\n");
    assert_eq!(incomplete.len(), 1);
}

#[test]
fn an_entry_of_an_override_completes_the_package() {
    let toml = "[js]\nproduction = true\n\n[[overrides]]\npackages = [\"web\"]\n\n[overrides.js]\nentry = [\"src/main.js!\"]\n";
    let (incomplete, _) = run(toml);
    assert_eq!(incomplete, Vec::new());
}

#[test]
fn an_entry_outside_the_package_that_names_a_file_completes_it() {
    let mut files = MEMBERS.to_vec();
    files.push(("shared/app.js", "export const app = 1;\n"));
    let toml = "[js]\nproduction = true\n\n[js.workspaces.\"packages/web\"]\nentry = [\"../../shared/app.js!\"]\n";
    let (incomplete, _) = run_files(&files, toml);
    assert_eq!(incomplete, Vec::new());
}

fn narrowed_in(files: &[(&str, &str)], toml: &str) -> Vec<IncompletePackage> {
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let sources: Vec<Arc<dyn InputSource>> = vec![Arc::new(NarrowedSource)];
    JsAnalysis::build(&workspace, &config, &fs, &sources, true).incomplete_packages()
}

#[test]
fn a_stale_entry_keeps_the_no_entries_reason_of_a_package_without_files() {
    let files = [("package.json", r#"{"name":"web","version":"1.0.0","private":true}"#)];
    let without = narrowed_in(&files, "[js]\nproduction = true\n");
    let causes: Vec<IncompleteCause> = without[0].reasons.iter().map(|reason| reason.cause).collect();
    assert_eq!(causes, [IncompleteCause::ComputedEntries, IncompleteCause::NoEntries]);
    let stale = narrowed_in(&files, "[js]\nproduction = true\nentry = [\"src/old.js!\"]\n");
    assert_eq!(stale, without);
}

#[test]
fn a_dot_slash_outside_pattern_does_not_complete_the_package() {
    let mut files = MEMBERS.to_vec();
    files.push(("shared/app.js", "export const app = 1;\n"));
    let toml = "[js]\nproduction = true\n\n[js.workspaces.\"packages/web\"]\nentry = [\"./../../shared/app.js!\"]\n";
    let (incomplete, _) = run_files(&files, toml);
    assert_eq!(incomplete.len(), 1);
}

#[test]
fn an_outside_entry_needs_to_name_a_file() {
    let mut files = MEMBERS.to_vec();
    files.push(("shared/app.js", "export const app = 1;\n"));
    let without = "[js]\nproduction = true\n";
    assert_eq!(run_files(&files, without).0.len(), 1);
    let stale = "[js]\nproduction = true\n\n[js.workspaces.\"packages/web\"]\nentry = [\"../../shared/gone.js!\"]\n";
    assert_eq!(run_files(&files, stale).0.len(), 1);
}

#[test]
fn a_restored_narrowed_scope_gives_way_to_an_unrelated_cause() {
    let files = [
        (
            "package.json",
            r#"{"name":"web","version":"1.0.0","private":true,"main":"dist/index.js"}"#,
        ),
        ("src/main.js", "export const main = 1;\n"),
    ];
    let memory = files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    });
    let config = Config::from_toml_str("[js]\nproduction = true\nentry = [\"src/old.js!\"]\n").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let sources: Vec<Arc<dyn InputSource>> = vec![Arc::new(NarrowedSource)];
    let found = JsAnalysis::build(&workspace, &config, &fs, &sources, true).incomplete_packages();
    assert_eq!(found.len(), 1);
    let causes: Vec<IncompleteCause> = found[0].reasons.iter().map(|reason| reason.cause).collect();
    assert_eq!(
        causes,
        [IncompleteCause::ComputedEntries, IncompleteCause::UnmappedPublished]
    );
    assert_eq!(found[0].scope, IncompleteScope::Package);
}
