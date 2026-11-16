//! Type-aware member rules (ADR 0010): where the checker covers a declaring file, member uses come from it;
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

const TYPES_ON: &str = "[js]\ntype-aware = true\n";

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
    let config = Config::from_toml_str(TYPES_ON).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let sources: Vec<Arc<dyn InputSource>> = Vec::new();
    JsAnalysis::build(&workspace, &config, &fs, &sources, true).explain_usage(Path::new(file), Some(export))
}

#[test]
fn typed_reads_leave_a_same_named_member_of_another_class_unused() {
    let files = [PACKAGE, TSCONFIG, WIDGETS, DRAW];
    assert_eq!(
        class_findings(&files, TYPES_ON),
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
fn a_read_through_an_interface_keeps_the_member_an_implementing_class_inherits_used() {
    let files = [
        PACKAGE,
        TSCONFIG,
        (
            "src/cell.ts",
            "export interface ICell {\n  getRange(): number;\n}\n\n\
             export abstract class BaseCell {\n  getRange(): number {\n    return 1;\n  }\n\n  \
             getSize(): number {\n    return 2;\n  }\n}\n\n\
             export class CodeCell extends BaseCell implements ICell {}\n",
        ),
        (
            "src/index.ts",
            "import type { ICell } from './cell';\nimport { BaseCell, CodeCell } from './cell';\n\n\
             export function f(c: ICell): number {\n  return c.getRange();\n}\n\n\
             export const make = (): BaseCell => new CodeCell();\n",
        ),
    ];
    assert_eq!(
        class_findings(&files, TYPES_ON),
        ["src/cell.ts: BaseCell.getSize"],
        "`c.getRange()` reaches `BaseCell.getRange` through `CodeCell`, and nothing reads `getSize`"
    );
    assert_eq!(
        class_findings(&files, TYPES_OFF),
        ["src/cell.ts: BaseCell.getSize"],
        "by name, `getRange` is read and `getSize` is not"
    );
}

/// The forms of a default-exported class in `src/lib.ts` with members `render` and `paint`, by the name a finding
/// gives the class.
const DEFAULT_CLASSES: [(&str, &str); 6] = [
    (
        "Lib",
        "export default class Lib {\n  render(): void {}\n  paint(): void {}\n}\n",
    ),
    (
        "default",
        "export default class {\n  render(): void {}\n  paint(): void {}\n}\n",
    ),
    (
        "Lib",
        "class Lib {\n  render(): void {}\n  paint(): void {}\n}\nexport default Lib;\n",
    ),
    (
        "Lib",
        "class Lib {\n  render(): void {}\n  paint(): void {}\n}\nexport { Lib as default };\n",
    ),
    (
        "Lib",
        "export class Lib {\n  render(): void {}\n  paint(): void {}\n}\nexport default Lib;\n",
    ),
    (
        "Lib",
        "class Lib {\n  render(): void {}\n  paint(): void {}\n}\nexport { Lib, Lib as default };\n",
    ),
];

/// `paint` of another class, which `src/index.ts` reads, so by name `paint` of every class counts as read.
const OTHER: (&str, &str) = ("src/other.ts", "export class Other {\n  paint(): void {}\n}\n");

/// A file of a fixture: its path and its content.
type File = (&'static str, &'static str);

/// The ways `src/index.ts` reaches the default export of `src/lib.ts`: the import, the expression it constructs, and
/// the re-exporting module it needs.
const DEFAULT_READERS: [(&str, &str, &str, Option<File>); 4] = [
    ("a default import", "import Lib from './lib';", "Lib", None),
    (
        "a namespace import",
        "import * as lib from './lib';",
        "lib.default",
        None,
    ),
    (
        "a default re-export",
        "import Lib from './barrel';",
        "Lib",
        Some(("src/barrel.ts", "export { default } from './lib';\n")),
    ),
    (
        "a re-export as a name",
        "import { Lib } from './barrel';",
        "Lib",
        Some(("src/barrel.ts", "export { default as Lib } from './lib';\n")),
    ),
];

#[test]
fn a_default_exported_class_is_judged_with_types_like_a_named_one() {
    for (label, class) in DEFAULT_CLASSES {
        for (reader, import, constructed, barrel) in DEFAULT_READERS {
            let index = format!(
                "{import}\nimport {{ Other }} from './other';\n\nnew {constructed}().render();\nnew Other().paint();\n"
            );
            let mut files = vec![
                PACKAGE,
                TSCONFIG,
                OTHER,
                ("src/index.ts", index.as_str()),
                ("src/lib.ts", class),
            ];
            files.extend(barrel);
            assert_eq!(
                class_findings(&files, TYPES_ON),
                [format!("src/lib.ts: {label}.paint")],
                "with types, `paint` is read on `Other` only: {reader} of {class:?}"
            );
            assert_eq!(
                class_findings(&files, TYPES_OFF),
                Vec::<String>::new(),
                "by name, `paint` is read on every class: {reader} of {class:?}"
            );
        }
    }
}

#[test]
fn a_class_exported_under_two_names_is_reported_once_with_types() {
    let index =
        "import Lib, { Lib as Named } from './lib';\n\nnew Lib().render();\nexport const named = new Named();\n";
    for &(_, class) in &DEFAULT_CLASSES[4..] {
        let files = [PACKAGE, TSCONFIG, ("src/index.ts", index), ("src/lib.ts", class)];
        assert_eq!(class_findings(&files, TYPES_ON), ["src/lib.ts: Lib.paint"], "{class:?}");
    }
}

#[test]
fn explain_usage_names_the_typed_evidence_of_an_anonymous_default_class() {
    let files = [
        PACKAGE,
        TSCONFIG,
        OTHER,
        (
            "src/index.ts",
            "import Lib from './lib';\nimport { Other } from './other';\n\nnew Lib().render();\nnew Other().paint();\n",
        ),
        ("src/lib.ts", DEFAULT_CLASSES[1].1),
    ];
    let read = serde_json::to_value(explain(&files, "src/lib.ts", "default.render")).unwrap();
    assert_eq!(read["class-member"]["typed"]["read"], true);
    assert_eq!(read["class-member"]["typed"]["declaration"], "src/lib.ts#default");
    let unread = serde_json::to_value(explain(&files, "src/lib.ts", "default.paint")).unwrap();
    assert_eq!(unread["class-member"]["typed"]["read"], false);
    assert_eq!(unread["class-member"]["verdict"], "unused");
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
    let typed = found_by(&lint(&files, TYPES_ON).diagnostics, &rules);
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
    let report = lint(&files, TYPES_ON);
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
    let report = lint(&files, TYPES_ON);
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
    assert_eq!(class_findings(&files, TYPES_ON), ["src/service.ts: Service.unread"]);
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
    let summary = lint(&files, TYPES_ON).type_aware.expect("the JS analysis ran");
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
    let summary = lint(
        &files,
        "[js]\ntype-aware = true\n[js.workspaces.\".\"]\ntype-aware = false\n",
    )
    .type_aware
    .expect("the JS analysis ran");
    assert!(!summary.enabled);
    assert_eq!(summary.packages[0].reason.as_deref(), Some("disabled"));
}

#[test]
fn types_are_off_by_default() {
    let files = [PACKAGE, TSCONFIG, WIDGETS, DRAW];
    let report = lint(&files, "");
    assert_eq!(
        found_by(&report.diagnostics, &["unused-class-members"]),
        ["src/widgets.ts: Widget.paint"],
        "by name, `render` is read on every class"
    );
    let summary = report.type_aware.expect("the JS analysis ran");
    assert!(!summary.enabled);
    assert_eq!(summary.packages[0].reason.as_deref(), Some("disabled"));
}

#[test]
fn a_workspace_that_turns_types_on_is_typed_under_the_off_default() {
    let files = [PACKAGE, TSCONFIG, WIDGETS, DRAW];
    let report = lint(&files, "[js.workspaces.\".\"]\ntype-aware = true\n");
    assert_eq!(
        found_by(&report.diagnostics, &["unused-class-members"]),
        ["src/widgets.ts: Gadget.render", "src/widgets.ts: Widget.paint"],
        "the typed read `widget.render()` reaches `Widget.render` only"
    );
    let summary = report.type_aware.expect("the JS analysis ran");
    assert!(summary.enabled);
    assert_eq!(
        summary.packages,
        [TypeAwarePackage::new("", "app", TypeAwareState::Typed)]
    );
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

#[test]
fn an_enum_member_used_without_types_is_not_asked_of_the_checker() {
    let files = [
        PACKAGE,
        TSCONFIG,
        ("src/colors.ts", "export enum Color {\n  Red,\n  Green,\n}\n"),
        (
            "src/index.ts",
            "import { Color } from './colors';\n\nexport const red = Color.Red;\n",
        ),
    ];
    let red = serde_json::to_value(explain(&files, "src/colors.ts", "Color.Red")).unwrap();
    assert_eq!(red["class-member"]["used-without-types"], true);
    assert_eq!(red["class-member"]["typed"]["declaration"], "src/colors.ts#Color");
    assert_eq!(
        red["class-member"]["typed"]["read"], false,
        "the read of `Red`, used without types, is not asked of the checker"
    );
    assert_eq!(red["class-member"]["verdict"], "used");
    let green = serde_json::to_value(explain(&files, "src/colors.ts", "Color.Green")).unwrap();
    assert_eq!(
        green["class-member"].get("used-without-types"),
        None,
        "false is left out"
    );
    assert_eq!(green["class-member"]["verdict"], "unused");
    let rules = ["unused-enum-members"];
    assert_eq!(
        found_by(&lint(&files, TYPES_ON).diagnostics, &rules),
        ["src/colors.ts: Color.Green"]
    );
    assert_eq!(
        found_by(&lint(&files, TYPES_OFF).diagnostics, &rules),
        ["src/colors.ts: Color.Green"]
    );
}
