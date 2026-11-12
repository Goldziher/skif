//! `JsAnalysis::explain_dependency`: the verdict on a dependency in each package agrees with `unused-dependencies`,
//! and the evidence is what credited it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{FileSystem, MemoryFs};
use skif_js::{
    DependencyCredit, DependencyExplanation, DependencyUse, DependencyVerdict, InputSource, JsAnalysis, JsProvider,
    PackageDependencyUsage, PluginSource,
};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const WORKSPACE: &[(&str, &str)] = &[
    (
        "package.json",
        r#"{"name":"mono","private":true,"workspaces":["packages/*"],"scripts":{"test":"vitest run"},"devDependencies":{"vitest":"3.0.0"}}"#,
    ),
    (
        "packages/app/package.json",
        r#"{"name":"app","version":"1.0.0","main":"src/index.ts","dependencies":{"zod":"3.0.0","left-pad":"1.0.0"}}"#,
    ),
    (
        "packages/app/src/index.ts",
        "import { z } from 'zod';\nimport type { Infer } from 'zod';\nexport const schema: Infer = z;\n",
    ),
    (
        "packages/web/package.json",
        r#"{"name":"web","version":"1.0.0","main":"src/main.ts","dependencies":{"zod":"3.0.0"}}"#,
    ),
    (
        "packages/web/src/main.ts",
        "import type { ZodType } from 'zod';\nexport type Schema = ZodType;\n",
    ),
];

fn memory(extra: &[(&str, &str)]) -> MemoryFs {
    WORKSPACE
        .iter()
        .chain(extra)
        .fold(MemoryFs::new(), |memory, (path, text)| {
            memory.with_file(format!("/ws/{path}"), text)
        })
}

fn explain_with(config: &Config, extra: &[(&str, &str)], name: &str, package: Option<&str>) -> DependencyExplanation {
    let memory = memory(extra);
    let workspace = Workspace::discover_with_fs(Path::new("/ws"), config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let sources: Vec<Arc<dyn InputSource>> = vec![Arc::new(PluginSource::new(config.js.plugins.clone()))];
    let id = package.map(|wanted| {
        workspace
            .iter()
            .find(|(_, candidate)| candidate.name.as_deref() == Some(wanted))
            .map(|(id, _)| id)
            .unwrap()
    });
    JsAnalysis::build(&workspace, config, &fs, &sources, true).explain_dependency(name, id)
}

fn explain(name: &str) -> DependencyExplanation {
    explain_with(&Config::from_toml_str("").unwrap(), &[], name, None)
}

/// The names `unused-dependencies` reports, as `package dependency`, sorted.
fn reported_unused(config: &Config, extra: &[(&str, &str)]) -> Vec<String> {
    let memory = memory(extra);
    let workspace = Workspace::discover_with_fs(Path::new("/ws"), config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    let mut found: Vec<String> = run_unsorted(&registry, &workspace, config, &RunOptions::default())
        .unwrap()
        .into_iter()
        .filter(|diagnostic| diagnostic.rule.as_str() == "unused-dependencies")
        .map(|diagnostic| {
            format!(
                "{} {}",
                diagnostic.package.clone().unwrap_or_default(),
                diagnostic.dependency.clone().unwrap_or_default()
            )
        })
        .collect();
    found.sort();
    found
}

fn package<'a>(explanation: &'a DependencyExplanation, name: &str) -> &'a PackageDependencyUsage {
    explanation
        .packages
        .iter()
        .find(|package| package.package == name)
        .unwrap_or_else(|| panic!("no entry for {name}: {explanation:#?}"))
}

fn import(file: &str) -> serde_json::Value {
    serde_json::json!({ "kind": "import", "file": file })
}

#[test]
fn an_imported_dependency_is_used_with_the_importing_files_as_evidence() {
    let explanation = explain("zod");
    let names: Vec<&str> = explanation
        .packages
        .iter()
        .map(|package| package.package.as_str())
        .collect();
    assert_eq!(names, ["app", "web"], "{explanation:#?}");
    let app = package(&explanation, "app");
    assert_eq!(app.verdict, DependencyVerdict::Used);
    assert_eq!(app.credit, Some(DependencyCredit::Uses));
    assert_eq!(app.dir, "packages/app");
    assert_eq!(app.listed.as_ref().unwrap().section, "dependencies");
    assert_eq!(
        serde_json::to_value(&app.uses).unwrap(),
        serde_json::json!([import("packages/app/src/index.ts")])
    );
    assert!(app.not_counted.is_empty(), "{explanation:#?}");
    // A type-only import uses the package outside strict mode.
    assert_eq!(package(&explanation, "web").verdict, DependencyVerdict::Used);
}

#[test]
fn a_dependency_nothing_uses_is_unused_as_the_rule_reports_it() {
    let explanation = explain("left-pad");
    let app = package(&explanation, "app");
    assert_eq!(app.verdict, DependencyVerdict::Unused);
    assert_eq!(app.credit, None);
    assert_eq!(app.uses, Vec::<DependencyUse>::new());
    assert_eq!(
        reported_unused(&Config::from_toml_str("").unwrap(), &[]),
        ["app left-pad"]
    );
}

#[test]
fn a_binary_a_script_runs_credits_the_dependency_that_provides_it() {
    let explanation = explain("vitest");
    let root = package(&explanation, "mono");
    assert_eq!(root.verdict, DependencyVerdict::Used, "{explanation:#?}");
    assert_eq!(root.dir, ".");
    let binary = serde_json::to_value(&root.uses).unwrap();
    assert!(
        binary
            .as_array()
            .unwrap()
            .contains(&serde_json::json!({ "kind": "binary", "binary": "vitest" })),
        "{binary}"
    );
}

#[test]
fn an_ignored_dependency_is_unused_but_not_reported() {
    let config = Config::from_toml_str("[js]\nignore-dependencies = [\"left-pad\"]\n").unwrap();
    let explanation = explain_with(&config, &[], "left-pad", None);
    assert_eq!(package(&explanation, "app").verdict, DependencyVerdict::Ignored);
    assert_eq!(reported_unused(&config, &[]), Vec::<String>::new());
}

/// In strict mode a type-only import uses nothing: `web`'s only import of `zod` is listed as counting for no
/// manifest, and the verdict is the `unused-dependencies` finding.
#[test]
fn strict_mode_lists_the_type_only_imports_that_count_for_nothing() {
    let config = Config::from_toml_str("[js]\nstrict = true\n").unwrap();
    let explanation = explain_with(&config, &[], "zod", None);
    assert!(explanation.strict && explanation.production);
    let web = package(&explanation, "web");
    assert_eq!(web.verdict, DependencyVerdict::Unused, "{explanation:#?}");
    let not_counted: Vec<(&str, u32, bool)> = web
        .not_counted
        .iter()
        .map(|import| (import.file.as_str(), import.line, import.type_only))
        .collect();
    assert_eq!(not_counted, [("packages/web/src/main.ts", 1, true)]);
    // `app`'s file also imports it at runtime, which credits the file.
    let app = package(&explanation, "app");
    assert_eq!(app.verdict, DependencyVerdict::Used);
    assert!(app.not_counted.is_empty(), "{explanation:#?}");
    assert!(reported_unused(&config, &[]).contains(&"web zod".to_owned()));
}

#[test]
fn a_package_asked_for_is_explained_even_when_it_does_not_list_the_dependency() {
    let explanation = explain_with(&Config::from_toml_str("").unwrap(), &[], "zod", Some("mono"));
    assert_eq!(explanation.packages.len(), 1, "{explanation:#?}");
    let root = &explanation.packages[0];
    assert_eq!(root.package, "mono");
    assert_eq!(root.verdict, DependencyVerdict::NotListed);
    assert_eq!(root.listed, None);
}

#[test]
fn the_explanation_renders_each_package_with_its_evidence() {
    let text = explain("zod").render();
    assert!(
        text.starts_with("zod\n  packages/app (app): used\n    listed in packages/app/package.json dependencies \"3.0.0\"\n    used by:\n      import in packages/app/src/index.ts\n"),
        "{text}"
    );
    assert_eq!(
        serde_json::to_value(explain("left-pad")).unwrap()["packages"][0]["verdict"],
        "unused"
    );
}

/// A package that imports an optional peer of a dependency it uses without listing the peer gets no use of it: the
/// verdict is the unlisted import's, not a use through the host that names the peer.
#[test]
fn an_unlisted_peer_of_a_used_dependency_is_not_listed_rather_than_used() {
    let extra = [
        (
            "packages/tool/package.json",
            r#"{"name":"tool","version":"1.0.0","main":"src/index.ts","dependencies":{"host-lib":"1.0.0"}}"#,
        ),
        (
            "packages/tool/node_modules/host-lib/package.json",
            r#"{"name":"host-lib","version":"1.0.0","peerDependencies":{"peer-lib":"*"},"peerDependenciesMeta":{"peer-lib":{"optional":true}}}"#,
        ),
        (
            "packages/tool/src/index.ts",
            "import 'host-lib';\nimport { t } from 'peer-lib';\nexport const x = t;\n",
        ),
    ];
    let explanation = explain_with(&Config::from_toml_str("").unwrap(), &extra, "peer-lib", Some("tool"));
    let tool = package(&explanation, "tool");
    assert_eq!(tool.verdict, DependencyVerdict::NotListed, "{explanation:#?}");
    assert_eq!(tool.credit, None, "{explanation:#?}");
    assert!(
        explanation.render().contains("packages/tool (tool): not listed\n"),
        "{}",
        explanation.render()
    );
}
