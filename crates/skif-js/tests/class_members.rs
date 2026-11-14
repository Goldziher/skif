//! `unused-class-members` (ADR 0042): which members of used exported classes count as read, which are not judged,
//! and the `explain-usage` evidence for one member.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Arc;

use skif_config::Config;
use skif_core::{Diagnostic, FileSystem, MemoryFs};
use skif_js::{InputSource, JsAnalysis, JsProvider, PluginSource};
use skif_lint::{Registry, RunOptions, run_unsorted};
use skif_workspace::Workspace;

const ROOT: &str = "/ws";

const RULE: &str = "unused-class-members";

const PACKAGE: (&str, &str) = ("package.json", r#"{"name":"app","private":true,"main":"index.ts"}"#);

fn memory(files: &[(&str, &str)]) -> MemoryFs {
    files.iter().fold(MemoryFs::new(), |memory, (path, text)| {
        memory.with_file(Path::new(ROOT).join(path), text)
    })
}

fn lint_with(files: &[(&str, &str)], toml: &str) -> Vec<Diagnostic> {
    let memory = memory(files);
    let config = Config::from_toml_str(toml).unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let provider = JsProvider::new(fs).with_input_source(Arc::new(PluginSource::new(config.js.plugins.clone())));
    let registry = Registry::new(vec![Box::new(provider)]).unwrap();
    let mut diagnostics = run_unsorted(&registry, &workspace, &config, &RunOptions::default()).unwrap();
    diagnostics.sort_by(|a, b| {
        (a.rule.as_str(), &a.location.path, &a.message).cmp(&(b.rule.as_str(), &b.location.path, &b.message))
    });
    diagnostics
}

/// The findings of `rule`, as `file: subject` with the subject taken from between the first backticks.
fn found_by(diagnostics: &[Diagnostic], rule: &str) -> Vec<String> {
    diagnostics
        .iter()
        .filter(|d| d.rule.as_str() == rule)
        .map(|d| {
            let start = d.message.find('`').map_or(0, |i| i + 1);
            let end = d.message[start..].find('`').map_or(d.message.len(), |i| start + i);
            format!("{}: {}", d.location.path.display(), &d.message[start..end])
        })
        .collect()
}

fn found(files: &[(&str, &str)]) -> Vec<String> {
    found_by(&lint_with(files, ""), RULE)
}

fn none() -> Vec<String> {
    Vec::new()
}

fn explain(files: &[(&str, &str)], file: &str, export: &str) -> String {
    let memory = memory(files);
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let sources: Vec<Arc<dyn InputSource>> = Vec::new();
    JsAnalysis::build(&workspace, &config, &fs, &sources, true)
        .explain_usage(Path::new(file), Some(export))
        .render()
}

const STORE: &str = "export class Store {\n\
                     read() { return this.internal(); }\n\
                     optional() {}\n\
                     computed() {}\n\
                     destructured = 1;\n\
                     written = 0;\n\
                     internal() {}\n\
                     unusedMethod() {}\n\
                     unusedField = 1;\n\
                     static staticRead = 1;\n\
                     static staticUnused = 2;\n\
                     get getterUsed() { return 1; }\n\
                     get getterUnused() { return 1; }\n\
                     set setterUsed(value: number) {}\n\
                     set setterUnused(value: number) {}\n\
                     protected unusedProtected() {}\n\
                     private hidden() {}\n\
                     #secretUnused = 1;\n\
                     #secretRead = 2;\n\
                     peek() { return this.#secretRead; }\n\
                     }\n";

const STORE_USER: &str = "import { Store } from './store';\n\
                          const store = new Store();\n\
                          store.read();\n\
                          store?.optional();\n\
                          store['computed']();\n\
                          const { destructured } = store;\n\
                          store.written = 1;\n\
                          store.setterUsed = 2;\n\
                          store.peek();\n\
                          console.log(Store.staticRead, store.getterUsed, destructured);\n";

#[test]
fn members_no_module_reads_are_reported() {
    assert_eq!(
        found(&[PACKAGE, ("index.ts", STORE_USER), ("store.ts", STORE)]),
        [
            "store.ts: Store.#secretUnused",
            "store.ts: Store.getterUnused",
            "store.ts: Store.setterUnused",
            "store.ts: Store.staticUnused",
            "store.ts: Store.unusedField",
            "store.ts: Store.unusedMethod",
            "store.ts: Store.unusedProtected",
            "store.ts: Store.written",
        ],
        "reads through `.`, `?.`, `['x']`, destructuring and `this` count, as does a write for a setter; a field that is \
         only assigned is unused, and `private` members are left to the compiler"
    );
}

#[test]
fn a_class_no_module_uses_is_left_to_unused_exports() {
    let diagnostics = lint_with(
        &[
            PACKAGE,
            ("index.ts", "export {};\n"),
            ("lonely.ts", "export class Lonely { unread() {} }\n"),
        ],
        "",
    );
    assert_eq!(found_by(&diagnostics, RULE), none());
    assert_eq!(found_by(&diagnostics, "unused-files"), ["lonely.ts: lonely.ts"]);
}

#[test]
fn a_decorated_member_or_class_is_not_judged() {
    let files = [
        PACKAGE,
        (
            "index.ts",
            "import { Widget, Decorated, Plain } from './widget';\nconsole.log(new Widget(), new Decorated(), new Plain());\n",
        ),
        (
            "widget.ts",
            "function Input() { return (target: object, key: string) => {}; }\n\
             function Component(options: object) { return (target: object) => target; }\n\
             export class Widget {\n  @Input() label = '';\n  plain = '';\n}\n\
             @Component({})\nexport class Decorated {\n  unread() {}\n}\n\
             export class Plain {\n  unread() {}\n}\n",
        ),
    ];
    assert_eq!(found(&files), ["widget.ts: Plain.unread", "widget.ts: Widget.plain"]);
}

#[test]
fn members_a_known_library_base_declares_are_not_judged() {
    let files = [
        PACKAGE,
        (
            "index.ts",
            "import { View, Other } from './view';\nconsole.log(new View(), new Other());\n",
        ),
        (
            "view.ts",
            "import { Component } from 'react';\nimport * as React from 'react';\n\
             export class View extends Component {\n  render() { return null; }\n  componentDidMount() {}\n  \
             static defaultProps = {};\n  helper() {}\n}\n\
             export class Other extends React.PureComponent {\n  render() { return null; }\n  extra() {}\n}\n",
        ),
    ];
    assert_eq!(found(&files), ["view.ts: Other.extra", "view.ts: View.helper"]);
}

#[test]
fn a_base_class_whose_members_are_unknown_leaves_every_member_alone() {
    let files = [
        PACKAGE,
        (
            "index.ts",
            "import { Emitter, Element, Mixed, Root } from './bases';\n\
             console.log(new Emitter(), Element, Mixed, new Root());\n",
        ),
        (
            "bases.ts",
            "import { EventEmitter } from 'node:events';\nimport { mixin } from 'some-library';\n\
             export class Emitter extends EventEmitter {\n  unread() {}\n}\n\
             export class Element extends HTMLElement {\n  connectedCallback() {}\n}\n\
             export class Mixed extends mixin(Object) {\n  unread() {}\n}\n\
             export class Root {\n  unread() {}\n}\n",
        ),
    ];
    assert_eq!(found(&files), ["bases.ts: Root.unread"]);
}

#[test]
fn implemented_interface_members_are_not_judged() {
    let files = [
        PACKAGE,
        (
            "index.ts",
            "import { Square, Adapter } from './shapes';\nconsole.log(new Square(), new Adapter());\n",
        ),
        ("remote.ts", "export interface Remote { anything(): void }\n"),
        (
            "shapes.ts",
            "import type { Remote } from './remote';\n\
             interface Shape { area(): number }\n\
             export class Square implements Shape {\n  area() { return 1; }\n  extra() {}\n}\n\
             export class Adapter implements Remote {\n  anything() {}\n  static create() {}\n}\n",
        ),
    ];
    assert_eq!(
        found(&files),
        ["shapes.ts: Adapter.create", "shapes.ts: Square.extra"],
        "an imported interface may declare any instance member, but no static one"
    );
}

#[test]
fn tags_and_ignore_members_exempt_a_member() {
    let files = [
        PACKAGE,
        (
            "index.ts",
            "import { Tagged } from './tagged';\nconsole.log(new Tagged());\n",
        ),
        (
            "tagged.ts",
            "export class Tagged {\n  /** @public */\n  api() {}\n  ignoredName() {}\n  unread() {}\n}\n",
        ),
    ];
    assert_eq!(
        found(&files),
        ["tagged.ts: Tagged.ignoredName", "tagged.ts: Tagged.unread"]
    );
    assert_eq!(
        found_by(&lint_with(&files, "[js]\nignore-members = [\"ignoredName\"]\n"), RULE),
        ["tagged.ts: Tagged.unread"]
    );
}

/// knip's `ignore-members` fixture (knip 5.88.1, commit 8c25e4b7ff5c43d348082030600b1ba8677486af, ISC): a string and a
/// regular expression in `ignoreMembers`, a member read through `super`, and one only the subclass declares.
#[test]
fn ignore_members_follow_knips_fixture() {
    let files = [
        PACKAGE,
        ("index.ts", "import { MyClass } from './MyClass';\n\nMyClass.init();\n"),
        (
            "MyClass.ts",
            "class BaseClass {\n  constructor() {}\n  init() {}\n  used() {}\n  unused() {}\n}\n\n\
             export class MyClass extends BaseClass {\n  constructor() {\n    super();\n  }\n  ignored() {}\n  \
             init() {\n    super.init();\n  }\n  implemented() {\n    this.used();\n  }\n  Left() {}\n}\n",
        ),
    ];
    let diagnostics = lint_with(&files, "[js]\nignore-members = [\"ignored\", \"/Left|Right/\"]\n");
    assert_eq!(found_by(&diagnostics, RULE), ["MyClass.ts: MyClass.implemented"]);
}

#[test]
fn entry_exports_are_public_api_unless_include_entry_exports() {
    let files = [
        PACKAGE,
        (
            "index.ts",
            "import { Barreled } from './barrel';\nexport { Reexported } from './reexported';\n\
             export class Api {\n  unread() {}\n}\nconsole.log(new Barreled());\n",
        ),
        ("user.ts", "import { Api } from './index';\nconsole.log(new Api());\n"),
        ("main.ts", "import './user';\n"),
        ("barrel.ts", "export { Barreled } from './barreled';\n"),
        ("barreled.ts", "export class Barreled {\n  unread() {}\n}\n"),
        ("reexported.ts", "export class Reexported {\n  unread() {}\n}\n"),
    ];
    let package = (
        "package.json",
        r#"{"name":"app","private":true,"main":"index.ts","bin":"main.ts"}"#,
    );
    let mut files = files.to_vec();
    files[0] = package;
    assert_eq!(
        found(&files),
        ["barreled.ts: Barreled.unread"],
        "a class re-exported by the entry is public; one re-exported by a barrel is judged"
    );
    assert_eq!(
        found_by(&lint_with(&files, "[js]\ninclude-entry-exports = true\n"), RULE),
        ["barreled.ts: Barreled.unread", "index.ts: Api.unread"]
    );
}

#[test]
fn members_of_a_class_hierarchy_across_modules() {
    let files = [
        PACKAGE,
        (
            "index.ts",
            "import { Impl } from './impl';\nimport { Screen } from './screen';\nnew Impl().run();\nconsole.log(new Screen());\n",
        ),
        (
            "base.ts",
            "export abstract class Base {\n  run() { return this.hook(); }\n  abstract hook(): void;\n  \
             abstract unimplemented(): void;\n}\n",
        ),
        (
            "impl.ts",
            "import { Base } from './base';\nexport class Impl extends Base {\n  hook() {}\n  unimplemented() {}\n  \
             own() {}\n}\n",
        ),
        (
            "app-component.ts",
            "import { Component } from 'react';\nexport class AppComponent extends Component {\n  shared() {}\n}\n",
        ),
        (
            "screen.ts",
            "import { AppComponent } from './app-component';\nexport class Screen extends AppComponent {\n  \
             render() { return this.shared(); }\n  extra() {}\n}\n",
        ),
    ];
    assert_eq!(
        found(&files),
        [
            "base.ts: Base.unimplemented",
            "impl.ts: Impl.own",
            "impl.ts: Impl.unimplemented",
            "screen.ts: Screen.extra",
        ],
        "a base calling `this.hook()` uses every `hook`; React's members reach a subclass through a project base"
    );
}

#[test]
fn a_spied_name_and_a_template_read_count() {
    let files = [
        (
            "package.json",
            r#"{"name":"app","private":true,"main":"index.ts","devDependencies":{"vitest":"1"}}"#,
        ),
        (
            "index.ts",
            "import { Service } from './service';\nimport App from './App.vue';\nconsole.log(new Service(), App);\n",
        ),
        (
            "service.ts",
            "export class Service {\n  spied() {}\n  shown = '';\n  unread() {}\n}\nexport const service = new Service();\n",
        ),
        (
            "service.test.ts",
            "import { vi } from 'vitest';\nimport { service } from './service';\nvi.spyOn(service, 'spied');\n",
        ),
        (
            "App.vue",
            "<script setup lang=\"ts\">\nimport { service } from './service';\n</script>\n\
             <template><p>{{ service.shown }}</p></template>\n",
        ),
    ];
    assert_eq!(found(&files), ["service.ts: Service.unread"]);
}

#[test]
fn explain_usage_gives_the_evidence_for_a_member() {
    let files = [PACKAGE, ("index.ts", STORE_USER), ("store.ts", STORE)];
    let used = explain(&files, "store.ts", "Store.read");
    assert!(used.contains("class member `Store.read`:"), "{used}");
    assert!(used.contains("read by name in index.ts\n"), "{used}");
    assert!(used.contains("\n    => used\n"), "{used}");
    let internal = explain(&files, "store.ts", "Store.internal");
    assert!(internal.contains("read by name in store.ts\n"), "{internal}");
    let unused = explain(&files, "store.ts", "Store.unusedMethod");
    assert!(unused.contains("no module reads a property of this name"), "{unused}");
    assert!(
        unused.contains("=> never read: reported by `unused-class-members`"),
        "{unused}"
    );
    let missing = explain(&files, "store.ts", "Store.nothing");
    assert!(
        missing.contains("`Store.nothing` is not a member of an exported class"),
        "{missing}"
    );
}

#[test]
fn explain_usage_names_the_exemption() {
    let files = [
        PACKAGE,
        ("index.ts", "import { View } from './view';\nconsole.log(new View());\n"),
        (
            "view.ts",
            "import { Component } from 'react';\nexport class View extends Component {\n  render() { return null; }\n}\n",
        ),
    ];
    let text = explain(&files, "view.ts", "View.render");
    assert!(
        text.contains("it overrides a member of a known library base class, which the library calls"),
        "{text}"
    );
    assert!(text.contains("=> not judged"), "{text}");
}
