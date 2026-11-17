//! `unused-class-members` (ADR 0009): which members of used exported classes count as read, which are not judged,
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

fn explain_report(files: &[(&str, &str)], file: &str, export: &str) -> skif_js::UsageReport {
    let memory = memory(files);
    let config = Config::from_toml_str("").unwrap();
    let workspace = Workspace::discover_with_fs(Path::new(ROOT), &config, &memory).unwrap();
    let fs: Arc<dyn FileSystem> = Arc::new(memory);
    let sources: Vec<Arc<dyn InputSource>> = Vec::new();
    JsAnalysis::build(&workspace, &config, &fs, &sources, true).explain_usage(Path::new(file), Some(export))
}

fn explain(files: &[(&str, &str)], file: &str, export: &str) -> String {
    explain_report(files, file, export).render()
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

/// The forms of a default-exported class in `lib.ts` with members `used` and `unused`, by the name a finding gives
/// the class.
const DEFAULT_CLASSES: [(&str, &str); 6] = [
    ("Lib", "export default class Lib {\n  used() {}\n  unused() {}\n}\n"),
    ("default", "export default class {\n  used() {}\n  unused() {}\n}\n"),
    (
        "Lib",
        "class Lib {\n  used() {}\n  unused() {}\n}\nexport default Lib;\n",
    ),
    (
        "Lib",
        "class Lib {\n  used() {}\n  unused() {}\n}\nexport { Lib as default };\n",
    ),
    (
        "Lib",
        "export class Lib {\n  used() {}\n  unused() {}\n}\nexport default Lib;\n",
    ),
    (
        "Lib",
        "class Lib {\n  used() {}\n  unused() {}\n}\nexport { Lib, Lib as default };\n",
    ),
];

/// A file of a fixture: its path and its content.
type File = (&'static str, &'static str);

/// The ways `index.ts` reads `used` of the default export of `lib.ts`, with the re-exporting module each one needs.
const DEFAULT_READERS: [(&str, &str, Option<File>); 4] = [
    (
        "a default import",
        "import Lib from './lib';\nnew Lib().used();\n",
        None,
    ),
    (
        "a namespace import",
        "import * as lib from './lib';\nnew lib.default().used();\n",
        None,
    ),
    (
        "a default re-export",
        "import Lib from './barrel';\nnew Lib().used();\n",
        Some(("barrel.ts", "export { default } from './lib';\n")),
    ),
    (
        "a re-export as a name",
        "import { Lib } from './barrel';\nnew Lib().used();\n",
        Some(("barrel.ts", "export { default as Lib } from './lib';\n")),
    ),
];

#[test]
fn a_default_exported_class_is_judged_like_a_named_one() {
    for (label, class) in DEFAULT_CLASSES {
        for (reader, index, barrel) in DEFAULT_READERS {
            let mut files = vec![PACKAGE, ("index.ts", index), ("lib.ts", class)];
            files.extend(barrel);
            assert_eq!(
                found(&files),
                [format!("lib.ts: {label}.unused")],
                "{reader} of {class:?}"
            );
        }
    }
}

#[test]
fn a_class_exported_under_two_names_is_reported_once() {
    let index = "import Lib, { Lib as Named } from './lib';\nnew Lib().used();\nconsole.log(new Named());\n";
    for &(_, class) in &DEFAULT_CLASSES[4..] {
        assert_eq!(
            found(&[PACKAGE, ("index.ts", index), ("lib.ts", class)]),
            ["lib.ts: Lib.unused"],
            "{class:?}"
        );
    }
}

#[test]
fn a_class_an_entry_exposes_under_any_name_is_public_api() {
    let reader = ("reader.ts", "import Lib from './lib';\nnew Lib().used();\n");
    for &(_, class) in &DEFAULT_CLASSES[4..] {
        let index = ("index.ts", "export { Lib } from './lib';\nimport './reader';\n");
        assert_eq!(
            found(&[PACKAGE, index, reader, ("lib.ts", class)]),
            none(),
            "the entry exposes `Lib` and `reader.ts` imports `default` of {class:?}"
        );
    }
    let named = ("lib.ts", "export class Lib {\n  used() {}\n  unused() {}\n}\n");
    let defaulted = ("lib.ts", DEFAULT_CLASSES[0].1);
    for (index, lib) in [
        ("import { Lib } from './lib';\nexport default Lib;\n", named),
        ("import Lib from './lib';\nexport default Lib;\n", defaulted),
        ("import { Lib } from './lib';\nexport { Lib as default };\n", named),
        ("export { Lib as default } from './lib';\n", named),
        ("export { default } from './lib';\n", defaulted),
    ] {
        assert_eq!(found(&[PACKAGE, ("index.ts", index), lib]), none(), "{index:?}");
    }
}

#[test]
fn explain_usage_finds_a_default_exported_class_by_the_name_its_findings_give() {
    let files = [
        PACKAGE,
        ("index.ts", DEFAULT_READERS[0].1),
        ("lib.ts", DEFAULT_CLASSES[0].1),
    ];
    let unused = explain(&files, "lib.ts", "Lib.unused");
    assert!(unused.contains("class member `Lib.unused`:"), "{unused}");
    assert!(
        unused.contains("=> never read: reported by `unused-class-members`"),
        "{unused}"
    );
    let anonymous = [
        PACKAGE,
        ("index.ts", DEFAULT_READERS[0].1),
        ("lib.ts", DEFAULT_CLASSES[1].1),
    ];
    let unused = explain(&anonymous, "lib.ts", "default.unused");
    assert!(
        unused.contains("=> never read: reported by `unused-class-members`"),
        "{unused}"
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

const BRAND: &str = "type Options = { a: number };\nfunction run(): void {}\nexport class Brand {\n  \
                     private readonly _cursorContextBrand: void = undefined;\n  \
                     readonly _rgbaBrand: void = undefined;\n  \
                     declare protected readonly _Options: Options;\n  \
                     readonly __brand!: unique symbol;\n  \
                     readonly initialised: void = run();\n  \
                     plain = 1;\n}\n";

#[test]
fn type_only_fields_are_not_judged() {
    let files = [
        PACKAGE,
        (
            "index.ts",
            "import { Brand } from './brand';\nconsole.log(new Brand());\n",
        ),
        ("brand.ts", BRAND),
    ];
    assert_eq!(
        found(&files),
        ["brand.ts: Brand.initialised", "brand.ts: Brand.plain"],
        "a `declare` field and a brand typed `void` or `unique symbol` exist only for the type checker; a `void` field \
         with a real initialiser runs code and is judged"
    );
}

const REACTIVE: &str = "export class Reactive {\n  __v_skip = true;\n  __v_raw = 1;\n  __v_key = 2;\n  \
                        __v_unread = 3;\n}\n";

#[test]
fn an_element_access_through_a_constant_key_reads_its_string() {
    let files = [
        PACKAGE,
        (
            "index.ts",
            "import { Reactive } from './reactive';\n\
             enum ReactiveFlags { SKIP = '__v_skip' }\n\
             const Keys = { RAW: '__v_raw' } as const;\n\
             const KEY = '__v_key';\n\
             const target = new Reactive();\n\
             console.log(target[ReactiveFlags.SKIP], target[Keys.RAW], target[KEY]);\n",
        ),
        ("reactive.ts", REACTIVE),
    ];
    assert_eq!(found(&files), ["reactive.ts: Reactive.__v_unread"]);
}

#[test]
fn an_element_access_through_a_key_that_is_no_constant_reads_nothing() {
    let files = [
        PACKAGE,
        (
            "index.ts",
            "import { Reactive } from './reactive';\n\
             let someVariable = '__v_skip';\n\
             const Mutable = { RAW: '__v_raw' };\n\
             const KEY = '__v_key';\n\
             function read(target: Reactive, KEY: string) { return target[KEY]; }\n\
             const target = new Reactive();\n\
             console.log(target[someVariable], target[Mutable.RAW], read(target, someVariable));\n",
        ),
        ("reactive.ts", REACTIVE),
    ];
    assert_eq!(
        found(&files),
        [
            "reactive.ts: Reactive.__v_key",
            "reactive.ts: Reactive.__v_raw",
            "reactive.ts: Reactive.__v_skip",
            "reactive.ts: Reactive.__v_unread",
        ],
        "a `let`, a plain object's property and a parameter that shadows a constant can hold any key"
    );
}

#[test]
fn an_element_access_through_an_imported_constant_reads_its_string() {
    let files = [
        PACKAGE,
        (
            "index.ts",
            "import { Reactive } from './reactive';\n\
             import { ReactiveFlags, Mutable } from './flags';\n\
             import * as barrel from './barrel';\n\
             const target = new Reactive();\n\
             console.log(target[ReactiveFlags.SKIP], target[barrel.RAW], target[barrel.Keys.KEY], target[Mutable]);\n",
        ),
        (
            "flags.ts",
            "export enum ReactiveFlags { SKIP = '__v_skip' }\n\
             export const RAW = '__v_raw';\n\
             export const Keys = { KEY: '__v_key' } as const;\n\
             export let Mutable = '__v_unread';\n",
        ),
        ("barrel.ts", "export * from './flags';\n"),
        ("reactive.ts", REACTIVE),
    ];
    assert_eq!(
        found(&files),
        ["reactive.ts: Reactive.__v_unread"],
        "an enum member, a string constant and an `as const` property count through an import, also through a barrel; \
         an exported `let` does not"
    );
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

#[test]
fn explain_usage_names_the_type_only_exemption() {
    let files = [
        PACKAGE,
        (
            "index.ts",
            "import { Brand } from './brand';\nconsole.log(new Brand());\n",
        ),
        ("brand.ts", BRAND),
    ];
    let text = explain(&files, "brand.ts", "Brand._rgbaBrand");
    assert!(
        text.contains("it is a field that exists only for the type checker, never read at runtime"),
        "{text}"
    );
    assert!(text.contains("=> not judged"), "{text}");
}

const WIDGETS: (&str, &str) = (
    "widgets.ts",
    "export class Widget {\n  render(): void {}\n  paint(): void {}\n}\n\n\
     export class Gadget {\n  render(): void {}\n}\n",
);

#[test]
fn a_read_by_name_counts_for_the_same_named_member_of_every_class() {
    let typed_receiver = (
        "index.ts",
        "import { Gadget, Widget } from './widgets';\n\n\
         export function draw(widget: Widget): void {\n  widget.render();\n}\n\n\
         export const gadget = new Gadget();\n",
    );
    assert_eq!(
        found(&[PACKAGE, WIDGETS, typed_receiver]),
        ["widgets.ts: Widget.paint"],
        "`widget.render()` reads `render` of `Gadget` too: the receiver's type is not consulted"
    );
    let foreign_receiver = (
        "index.ts",
        "import { mount } from 'ui-lib';\nimport { Gadget, Widget } from './widgets';\n\n\
         export const widget = new Widget();\nexport const gadget = new Gadget();\nmount().render();\n",
    );
    let package = (
        "package.json",
        r#"{"name":"app","private":true,"main":"index.ts","dependencies":{"ui-lib":"1"}}"#,
    );
    assert_eq!(
        found(&[package, WIDGETS, foreign_receiver]),
        ["widgets.ts: Widget.paint"],
        "`render` read on a value of a dependency counts for every `render`"
    );
}

#[test]
fn a_read_through_an_interface_counts_for_the_member_an_implementing_class_inherits() {
    let files = [
        PACKAGE,
        (
            "cell.ts",
            "export interface ICell {\n  getRange(): number;\n}\n\n\
             export abstract class BaseCell {\n  getRange(): number {\n    return 1;\n  }\n\n  \
             getSize(): number {\n    return 2;\n  }\n}\n\n\
             export class CodeCell extends BaseCell implements ICell {}\n",
        ),
        (
            "index.ts",
            "import type { ICell } from './cell';\nimport { BaseCell, CodeCell } from './cell';\n\n\
             export function f(c: ICell): number {\n  return c.getRange();\n}\n\n\
             export const make = (): BaseCell => new CodeCell();\n",
        ),
    ];
    assert_eq!(
        found(&files),
        ["cell.ts: BaseCell.getSize"],
        "by name, `getRange` is read and `getSize` is not"
    );
}

#[test]
fn a_member_of_a_default_exported_class_counts_as_read_when_another_class_has_the_read_name() {
    let other = ("other.ts", "export class Other {\n  unused() {}\n}\n");
    for (_, class) in DEFAULT_CLASSES {
        for (reader, index, barrel) in DEFAULT_READERS {
            let index = format!("{index}import {{ Other }} from './other';\nnew Other().unused();\n");
            let mut files = vec![PACKAGE, other, ("index.ts", index.as_str()), ("lib.ts", class)];
            files.extend(barrel);
            assert_eq!(
                found(&files),
                none(),
                "by name, `unused` is read on every class: {reader} of {class:?}"
            );
        }
    }
}

#[test]
fn enum_and_namespace_members_are_judged_by_name() {
    let files = [
        PACKAGE,
        (
            "colors.ts",
            "export enum Color {\n  Red,\n  Green,\n  Blue,\n}\n\n\
             export namespace Shapes {\n  export const circle = 1;\n  export const square = 2;\n}\n",
        ),
        (
            "index.ts",
            "import { Color, Shapes } from './colors';\n\n\
             const Palette = Color;\nexport const red = Palette.Red;\n\
             export const green = Color.Green;\nexport const circle = Shapes.circle;\n",
        ),
    ];
    let diagnostics = lint_with(&files, "");
    assert_eq!(
        found_by(&diagnostics, "unused-enum-members"),
        none(),
        "`const Palette = Color` uses the whole enum, so `Blue` counts as used although nothing reads it"
    );
    assert_eq!(
        found_by(&diagnostics, "unused-namespace-members"),
        ["colors.ts: Shapes.square"],
        "a member nothing reads is reported"
    );
}

#[test]
fn explain_usage_says_whether_an_enum_member_is_used() {
    let files = [
        PACKAGE,
        ("colors.ts", "export enum Color {\n  Red,\n  Green,\n}\n"),
        (
            "index.ts",
            "import { Color } from './colors';\n\nexport const red = Color.Red;\n",
        ),
    ];
    let report = explain_report(&files, "colors.ts", "Color.Red");
    let text = report.render();
    assert!(text.contains("enum member `Color.Red`:"), "{text}");
    assert!(
        text.contains("used: read through an import or in its own file, or the whole enum is used"),
        "{text}"
    );
    assert!(text.contains("\n    => used\n"), "{text}");
    let red = serde_json::to_value(&report).unwrap();
    assert_eq!(red["class-member"]["of"], "enum");
    assert_eq!(red["class-member"]["used"], true);
    assert_eq!(red["class-member"]["verdict"], "used");
    let green = serde_json::to_value(explain_report(&files, "colors.ts", "Color.Green")).unwrap();
    assert_eq!(green["class-member"].get("used"), None, "false is left out");
    assert_eq!(green["class-member"]["verdict"], "unused");
    for removed in ["used-without-types", "typed", "untyped-because"] {
        assert_eq!(red["class-member"].get(removed), None, "{removed}");
        assert_eq!(green["class-member"].get(removed), None, "{removed}");
    }
    assert_eq!(
        found_by(&lint_with(&files, ""), "unused-enum-members"),
        ["colors.ts: Color.Green"]
    );
}
