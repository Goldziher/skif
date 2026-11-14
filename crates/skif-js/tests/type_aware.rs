//! Type-aware member rules (ADR 0044, ADR 0046): where the checker covers a declaring file, member uses come from it;
//! elsewhere, and for every read it cannot settle, they are credited by name. Each workspace has a real
//! `tsconfig.json`.

#![cfg(feature = "type-aware")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs, Report, TypeAwarePackage, TypeAwareState, TypeAwareSummary};
use skif_js::{InputSource, JsAnalysis, JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

const TYPES_OFF: &str = "[js]\ntype-aware = false\n";

const PACKAGE: (&str, &str) = ("package.json", r#"{"name":"app","private":true,"main":"src/index.ts"}"#);

const TSCONFIG: (&str, &str) = (
    "tsconfig.json",
    r#"{
  "compilerOptions": {
    "target": "es2022",
    "module": "esnext",
    "moduleResolution": "bundler",
    "strict": true,
    "noEmit": true
  },
  "include": ["src"]
}
"#,
);

const WIDGETS: (&str, &str) = (
    "src/widgets.ts",
    "export class Widget {\n  render(): void {}\n  paint(): void {}\n}\n\n\
     export class Gadget {\n  render(): void {}\n}\n",
);

const DRAW: (&str, &str) = (
    "src/index.ts",
    "import { Gadget, Widget } from './widgets';\n\n\
     export function draw(widget: Widget): void {\n  widget.render();\n}\n\n\
     export const gadget = new Gadget();\n",
);

fn memory(files: &[(&str, &str)]) -> MemoryFs {
    files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    })
}

fn lint(files: &[(&str, &str)], toml: &str) -> Report {
    let memory = memory(files);
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    run(&registry, &workspace, &config, &RunOptions::default()).unwrap()
}

/// The findings of `rules`, as `file: subject` with the subject taken from between the first backticks, sorted.
fn found_by(diagnostics: &[Diagnostic], rules: &[&str]) -> Vec<String> {
    let mut found: Vec<String> = diagnostics
        .iter()
        .filter(|d| rules.contains(&d.rule.as_str()))
        .map(|d| {
            let start = d.message.find('`').map_or(0, |i| i + 1);
            let end = d.message[start..].find('`').map_or(d.message.len(), |i| start + i);
            format!("{}: {}", d.location.path.display(), &d.message[start..end])
        })
        .collect();
    found.sort();
    found
}

fn class_findings(files: &[(&str, &str)], toml: &str) -> Vec<String> {
    found_by(&lint(files, toml).diagnostics, &["unused-class-members"])
}

fn explain(files: &[(&str, &str)], file: &str, export: &str) -> skif_js::UsageReport {
    let memory = memory(files);
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let sources: Vec<Arc<dyn InputSource>> = Vec::new();
    JsAnalysis::build(&workspace, &config, &fs, &sources, true).explain_usage(Path::new(file), Some(export))
}

#[test]
fn typed_reads_leave_a_same_named_member_of_another_class_unused() {
    let files = [PACKAGE, TSCONFIG, WIDGETS, DRAW];
    assert_eq!(
        class_findings(&files, ""),
        ["src/widgets.ts: Gadget.render", "src/widgets.ts: Widget.paint"],
        "the typed read `widget.render()` reaches `Widget.render` only"
    );
    assert_eq!(
        class_findings(&files, TYPES_OFF),
        ["src/widgets.ts: Widget.paint"],
        "by name, `render` is read on every class"
    );
}

#[test]
fn typed_mode_only_removes_enum_and_namespace_findings() {
    let files = [
        PACKAGE,
        TSCONFIG,
        (
            "src/colors.ts",
            "export enum Color {\n  Red,\n  Green,\n  Blue,\n}\n\n\
             export namespace Shapes {\n  export const circle = 1;\n  export const square = 2;\n}\n",
        ),
        (
            "src/index.ts",
            "import { Color, Shapes } from './colors';\n\n\
             const Palette = Color;\nexport const red = Palette.Red;\n\
             export const green = Color.Green;\nexport const circle = Shapes.circle;\n",
        ),
    ];
    let rules = ["unused-enum-members", "unused-namespace-members"];
    let typed = found_by(&lint(&files, "").diagnostics, &rules);
    let untyped = found_by(&lint(&files, TYPES_OFF).diagnostics, &rules);
    assert!(
        typed.iter().all(|finding| untyped.contains(finding)),
        "typed {typed:?} must be a subset of untyped {untyped:?}"
    );
    assert!(
        typed.contains(&"src/colors.ts: Shapes.square".to_owned()),
        "a member nothing reads stays reported with types: {typed:?}"
    );
}

#[test]
fn a_package_without_tsconfig_keeps_name_based_class_member_findings() {
    let files = [PACKAGE, WIDGETS, DRAW];
    let report = lint(&files, "");
    assert_eq!(
        found_by(&report.diagnostics, &["unused-class-members"]),
        class_findings(&files, TYPES_OFF)
    );
    assert_eq!(
        found_by(&report.diagnostics, &["unused-class-members"]),
        ["src/widgets.ts: Widget.paint"]
    );
    let summary = report.type_aware.expect("the JS analysis ran");
    assert_eq!(
        summary.packages,
        [TypeAwarePackage::new("", "app", TypeAwareState::Untyped)
            .with_reason("no-tsconfig")
            .with_untyped_files(2)]
    );
}

#[test]
fn uninstalled_dependencies_credit_reads_by_name() {
    let files = [
        (
            "package.json",
            r#"{"name":"app","private":true,"main":"src/index.ts","dependencies":{"ui-lib":"1"}}"#,
        ),
        TSCONFIG,
        WIDGETS,
        (
            "src/index.ts",
            "import { mount } from 'ui-lib';\nimport { Gadget, Widget } from './widgets';\n\n\
             export const widget = new Widget();\nexport const gadget = new Gadget();\nmount().render();\n",
        ),
    ];
    let report = lint(&files, "");
    assert_eq!(
        found_by(&report.diagnostics, &["unused-class-members"]),
        ["src/widgets.ts: Widget.paint"],
        "`render` read through the unresolved `ui-lib` credits every `render`; `paint` is read nowhere"
    );
    let summary = report.type_aware.expect("the JS analysis ran");
    assert_eq!(summary.packages.len(), 1);
    assert_eq!(summary.packages[0].state, TypeAwareState::Partial);
    assert_eq!(summary.packages[0].reason.as_deref(), Some("unresolved-modules: 1"));
}

#[test]
fn a_vue_template_reading_a_class_member_by_name_keeps_it_used() {
    let files = [
        (
            "package.json",
            r#"{"name":"app","private":true,"main":"src/index.ts","dependencies":{"vue":"3"}}"#,
        ),
        TSCONFIG,
        (
            "src/index.ts",
            "import { Service } from './service';\nimport App from './App.vue';\nconsole.log(new Service(), App);\n",
        ),
        (
            "src/service.ts",
            "export class Service {\n  shown = '';\n  unread() {}\n}\nexport const service = new Service();\n",
        ),
        (
            "src/App.vue",
            "<script setup lang=\"ts\">\nimport { service } from './service';\n</script>\n\
             <template><p>{{ service.shown }}</p></template>\n",
        ),
    ];
    assert_eq!(class_findings(&files, ""), ["src/service.ts: Service.unread"]);
}

#[test]
fn the_report_summarizes_a_mixed_workspace() {
    let files = [
        (
            "package.json",
            r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
        ),
        (
            "packages/typed/package.json",
            r#"{"name":"typed","main":"src/index.ts"}"#,
        ),
        ("packages/typed/tsconfig.json", TSCONFIG.1),
        ("packages/typed/src/index.ts", "export const typed = 1;\n"),
        (
            "packages/plain/package.json",
            r#"{"name":"plain","main":"src/index.ts"}"#,
        ),
        ("packages/plain/src/index.ts", "export const plain = 1;\n"),
    ];
    let summary = lint(&files, "").type_aware.expect("the JS analysis ran");
    assert_eq!(
        summary,
        TypeAwareSummary::new(
            true,
            vec![
                TypeAwarePackage::new("packages/plain", "plain", TypeAwareState::Untyped)
                    .with_reason("no-tsconfig")
                    .with_untyped_files(1),
                TypeAwarePackage::new("packages/typed", "typed", TypeAwareState::Typed),
            ],
        )
        .with_engine(skif_js_types::ENGINE)
    );
}

#[test]
fn a_workspace_that_turns_types_off_is_reported_disabled() {
    let files = [PACKAGE, TSCONFIG, WIDGETS, DRAW];
    let summary = lint(&files, "[js.workspaces.\".\"]\ntype-aware = false\n")
        .type_aware
        .expect("the JS analysis ran");
    assert!(!summary.enabled);
    assert_eq!(summary.packages[0].reason.as_deref(), Some("disabled"));
}

#[test]
fn explain_usage_names_the_typed_evidence() {
    let files = [PACKAGE, TSCONFIG, WIDGETS, DRAW];
    let read = explain(&files, "src/widgets.ts", "Widget.render");
    let text = read.render();
    assert!(
        text.contains("read with types: a typed read reaches it on `src/widgets.ts#Widget`"),
        "{text}"
    );
    assert!(text.contains("\n    => used\n"), "{text}");
    let json = serde_json::to_value(&read).unwrap();
    assert_eq!(json["class-member"]["typed"]["read"], true);
    assert_eq!(json["class-member"]["typed"]["declaration"], "src/widgets.ts#Widget");

    let unread = explain(&files, "src/widgets.ts", "Gadget.render").render();
    assert!(
        unread.contains("no typed read reaches it on `src/widgets.ts#Gadget`"),
        "{unread}"
    );
    assert!(
        unread.contains("=> never read: reported by `unused-class-members`"),
        "{unread}"
    );
}

#[test]
fn explain_usage_names_reads_credited_by_name_through_unresolved_receivers() {
    let files = [
        PACKAGE,
        TSCONFIG,
        WIDGETS,
        (
            "src/index.ts",
            "import { Gadget } from './widgets';\n\n\
             export const gadget = new Gadget();\n\
             export function draw(item: any): void {\n  item.render();\n}\n",
        ),
    ];
    let report = explain(&files, "src/widgets.ts", "Gadget.render");
    let text = report.render();
    assert!(
        text.contains("read by name where the checker could not type the receiver, in src/index.ts"),
        "{text}"
    );
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(
        json["class-member"]["typed"]["unresolved-readers"],
        serde_json::json!(["src/index.ts"])
    );
}

#[test]
fn explain_usage_explains_an_enum_member_with_types() {
    let files = [
        PACKAGE,
        TSCONFIG,
        ("src/colors.ts", "export enum Color {\n  Red,\n  Green,\n}\n"),
        (
            "src/index.ts",
            "import { Color } from './colors';\n\nconst Palette = Color;\nexport const red = Palette.Red;\n",
        ),
    ];
    let report = explain(&files, "src/colors.ts", "Color.Red");
    let text = report.render();
    assert!(text.contains("enum member `Color.Red`:"), "{text}");
    assert!(text.contains("\n    => used\n"), "{text}");
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["class-member"]["of"], "enum");
    assert_eq!(json["class-member"]["typed"]["declaration"], "src/colors.ts#Color");
}
