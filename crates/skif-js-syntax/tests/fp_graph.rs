//! Module facts behind the graph false-positive fixes: promise callbacks, ambient modules, signature references.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use skif_js_syntax::{ImportKind, ImportedKind, MockCall, ModuleInfo, ParseOptions, parse_module};

fn parse(path: &str, source: &str) -> ModuleInfo {
    parse_module(Path::new(path), source, &ParseOptions::default()).expect("parses")
}

fn imported(info: &ModuleInfo, specifier: &str) -> Vec<String> {
    info.imports
        .iter()
        .filter(|import| import.specifier == specifier)
        .flat_map(|import| &import.names)
        .map(|name| match &name.kind {
            ImportedKind::Default => "default".to_owned(),
            ImportedKind::Namespace => "*".to_owned(),
            ImportedKind::Named(name) => name.clone(),
        })
        .collect()
}

#[test]
fn then_callbacks_name_the_exports_they_read() {
    let cases: &[(&str, &[&str])] = &[
        ("import('./x').then((m) => m.Page)", &["Page"]),
        ("import('./x').then(function (m) { return m['Page']; })", &["Page"]),
        ("import('./x').then(({ Page, Other: o }) => Page)", &["Page", "Other"]),
        ("import('./x').then((m) => ({ default: m.Page }))", &["Page"]),
        ("import('./x').then((m) => m.default)", &["default"]),
        ("import('./x').then((m) => use(m))", &["*"]),
        ("import('./x').then(handler)", &["*"]),
        ("import('./x').then(() => 1)", &["*"]),
    ];
    for (expression, expected) in cases {
        let info = parse("a.ts", &format!("export const p = {expression};\n"));
        assert_eq!(imported(&info, "./x"), *expected, "{expression}");
    }
}

#[test]
fn awaited_import_members_are_still_read_directly() {
    let info = parse("a.ts", "export const then = (await import('./x')).then;\n");
    assert_eq!(imported(&info, "./x"), ["then"]);
}

#[test]
fn declare_module_names_are_collected() {
    let info = parse(
        "types.d.ts",
        "declare module 'flags' { export const on: boolean; }\ndeclare module '@scope/pkg/sub';\ndeclare module Plain {}\n",
    );
    assert_eq!(info.ambient_modules, ["flags", "@scope/pkg/sub"]);
}

#[test]
fn jsdoc_imports_only_count_inside_tag_braces() {
    let info = parse(
        "a.ts",
        "/** Calls `lazy(() => import('./prose'))`.\n * @type {import('./typed').T}\n */\nexport const a = 1;\n",
    );
    let specifiers: Vec<&str> = info.imports.iter().map(|import| import.specifier.as_str()).collect();
    assert_eq!(specifiers, ["./typed"]);
}

#[test]
fn signature_references_name_the_containing_exports() {
    let info = parse(
        "a.ts",
        "export interface Props { a: string }\n\
         export interface Inner { a: string }\n\
         export interface Body { a: string }\n\
         export interface Skipped { a: string }\n\
         export function Widget(p: Props): void { const x: Body = null as never; }\n\
         export const make = (): Inner => ({ a: '' });\n\
         export interface Holder { p: Skipped }\n",
    );
    let containing = |name: &str| {
        info.exports
            .iter()
            .find(|export| export.name == name)
            .map(|export| export.referenced_in.clone())
            .unwrap()
    };
    assert_eq!(containing("Props"), ["Widget"]);
    assert_eq!(containing("Inner"), ["make"]);
    assert_eq!(containing("Body"), [] as [std::string::String; 0]);
    assert_eq!(containing("Skipped"), [] as [std::string::String; 0]);
}

#[test]
fn a_type_that_shares_its_name_with_a_value_exported_by_a_clause_contains_what_it_names() {
    let info = parse(
        "a.ts",
        "export interface Options { a: string }\nexport interface Result { b: string }\n\
         export interface Other { c: string }\n\
         type Service = { run(options: Options): Result };\nconst Service = make();\nexport { Service };\n\
         export type Alone = { other: Other };\n",
    );
    let containing = |name: &str| {
        info.exports
            .iter()
            .find(|export| export.name == name)
            .map(|export| export.referenced_in.clone())
            .unwrap()
    };
    assert_eq!(containing("Options"), ["Service"]);
    assert_eq!(containing("Result"), ["Service"]);
    assert_eq!(
        containing("Other"),
        [] as [String; 0],
        "a type without a value of its name is not followed"
    );
}

#[test]
fn every_binding_of_a_destructured_export_contains_the_types_of_its_initialiser() {
    let info = parse(
        "a.ts",
        "export type Props = { a: string };\n\
         export const { use: useCtx, provider: Provider } = make({ init: (props: Props) => props });\n",
    );
    let containing = info
        .exports
        .iter()
        .find(|export| export.name == "Props")
        .map(|export| export.referenced_in.clone())
        .unwrap();
    assert_eq!(containing, ["Provider", "useCtx"]);
}

#[test]
fn re_exports_and_type_containers_name_every_containing_value_export() {
    let info = parse(
        "a.ts",
        "import { Props, Theme as T } from './props';\n\
         import Base from './base';\n\
         export { Props, Theme } from './props';\n\
         export { default } from './base';\n\
         export type Config = { props: Props };\n\
         type Store = { theme: T; base: Base };\n\
         const Store = create();\n\
         export { Store, Store as Alias };\n\
         export function render(p: Props): Base { return p as never; }\n",
    );
    let containing: Vec<(&str, Vec<String>)> = info
        .exports
        .iter()
        .map(|export| (export.name.as_str(), export.referenced_in.clone()))
        .collect();
    assert_eq!(
        containing,
        [
            ("Props", vec!["render".to_owned()]),
            ("Theme", vec!["Alias".to_owned(), "Store".to_owned()]),
            (
                "default",
                vec!["Alias".to_owned(), "Store".to_owned(), "render".to_owned()]
            ),
            ("Config", vec![]),
            ("Store", vec![]),
            ("Alias", vec![]),
            ("render", vec![]),
        ]
    );
}

#[test]
fn signature_references_follow_the_first_import_binding_and_exported_containers_only() {
    type Expected<'a> = &'a [(&'a str, &'a [&'a str])];
    let cases: &[(&str, &str, Expected<'_>)] = &[
        (
            "a re-export follows the first import of its name",
            "import { a } from './x';\nimport { a as b } from './x';\nexport { a } from './x';\n\
             export function f(p: b): void {}\n",
            &[("a", &[]), ("f", &[])],
        ),
        (
            "a container that is not exported contributes nothing",
            "function g(p: P) {}\nexport interface P {}\nexport function h(p: P): void {}\n",
            &[("P", &["h"]), ("h", &[])],
        ),
        (
            "a container exported under several names contains a re-export under each",
            "import { R } from './r';\nexport { R } from './r';\nfunction make(p: R): void {}\n\
             export { make, make as build };\n",
            &[("R", &["build", "make"]), ("make", &[]), ("build", &[])],
        ),
    ];
    for (case, source, expected) in cases {
        let info = parse("a.ts", source);
        let containing: Vec<(&str, Vec<&str>)> = info
            .exports
            .iter()
            .map(|export| {
                (
                    export.name.as_str(),
                    export.referenced_in.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        let expected: Vec<(&str, Vec<&str>)> = expected.iter().map(|(name, within)| (*name, within.to_vec())).collect();
        assert_eq!(containing, expected, "{case}");
    }
}

#[test]
fn members_read_by_name_in_the_declaring_file_are_recorded() {
    let info = parse(
        "a.ts",
        "export enum Color { Red, Green, Blue }\n\
         export const first = Color.Red;\n\
         export const second = Color['Green'];\n\
         export function describe(c: Color): string { return String(c); }\n\
         export enum Mode { A, B }\n\
         export const { A } = Mode;\n\
         export const modes = Object.values(Mode);\n\
         export enum Hidden { A, B }\n\
         export { Hidden as Renamed };\n",
    );
    let facts = |name: &str| {
        info.exports
            .iter()
            .find(|export| export.name == name)
            .map(|export| (export.members_read_in_file.clone(), export.used_whole_in_file))
            .unwrap()
    };
    assert_eq!(facts("Color"), (vec!["Green".to_owned(), "Red".to_owned()], false));
    assert_eq!(facts("Mode"), (vec!["A".to_owned()], true));
    assert_eq!(facts("Hidden"), (Vec::<String>::new(), false));
}

#[test]
fn an_enum_used_as_a_whole_in_its_own_file_is_flagged_and_one_read_by_name_or_annotated_is_not() {
    let cases: &[(&str, bool)] = &[
        ("export const all = Object.values(E);", true),
        ("export const names = Object.keys(E);", true),
        ("declare function use(e: unknown): void;\nuse(E);", true),
        ("export const copy = { ...E };", true),
        (
            "export function each() { for (const k in E) { console.log(k); } }",
            true,
        ),
        ("export const pick = (k: string) => E[k as keyof typeof E];", true),
        ("export const dynamic = (k: 'A' | 'B') => E[k];", true),
        ("export const bare = E;", true),
        ("export function get() { return E; }", true),
        ("export type Shape = typeof E;", true),
        ("export type Keys = keyof typeof E;", true),
        ("export type Values = `${E}`;", true),
        ("export type Prefixed = `on-${E}`;", true),
        ("export function annotated(e: E): E { return e; }", false),
        ("export const read = E.A;", false),
        ("export const literal = E['A'];", false),
        ("export const { A } = E;", false),
        ("export type One = `${E.A}`;", false),
        ("export type OneValue = typeof E.A;", false),
        ("export { E as Renamed };", false),
    ];
    for (code, whole) in cases {
        let info = parse(
            "a.ts",
            &format!("enum E {{ A = 'a', B = 'b' }}\nexport {{ E }};\n{code}\n"),
        );
        let export = info.exports.iter().find(|export| export.name == "E").unwrap();
        assert_eq!(export.used_whole_in_file, *whole, "{code}");
    }
}

#[test]
fn a_namespace_used_as_a_whole_in_its_own_file_is_not_flagged() {
    let info = parse(
        "a.ts",
        "export namespace N { export const a = 1; export const b = 2; }\nexport const all = Object.values(N);\n",
    );
    let export = info.exports.iter().find(|export| export.name == "N").unwrap();
    assert!(!export.used_whole_in_file, "{export:?}");
}

#[test]
fn namespace_members_read_by_their_bare_name_inside_the_namespace_are_recorded() {
    let info = parse(
        "a.ts",
        "export namespace Values {\n\
           export function findVisible() { return 1; }\n\
           export function show() { return findVisible(); }\n\
           export const unread = 1;\n\
         }\n\
         export namespace Types {\n\
           export type ProgressToken = string | number;\n\
           export interface Progress { token: ProgressToken }\n\
           export type Unread = string;\n\
         }\n\
         export namespace Shadowed {\n\
           export const a = 1;\n\
           export const b = 2;\n\
           export const c = 3;\n\
           export type T = string;\n\
           export function f(a: number) { return a; }\n\
           export function g() { const b = 1; return b; }\n\
           export function h<T>(x: T): T { return x; }\n\
           namespace Inner { export const c = 4; console.log(c); }\n\
         }\n\
         export namespace Outer {\n\
           export const v = 1;\n\
           namespace Inner { console.log(v); }\n\
         }\n\
         export namespace Merged { export const one = 1; export const p = 1; }\n\
         export namespace Merged { export const two = one; const p = 2; console.log(p); }\n\
         const unread = 2;\n\
         console.log(unread);\n",
    );
    let read = |name: &str| {
        info.exports
            .iter()
            .filter(|export| export.name == name)
            .map(|export| export.members_read_in_file.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(read("Values"), [["findVisible"]]);
    assert_eq!(read("Types"), [["ProgressToken"]]);
    assert_eq!(read("Shadowed"), [Vec::<String>::new()]);
    assert_eq!(read("Outer"), [["v"]]);
    assert_eq!(read("Merged"), [["one"], ["one"]]);
}

#[test]
fn a_constant_records_the_identifier_it_is_initialised_with() {
    let info = parse(
        "a.ts",
        "export function f() {}\nexport const g = f;\nexport const h = f();\nexport const cast = f as any;\nexport const bang = f!;\n",
    );
    let alias = |name: &str| {
        info.exports
            .iter()
            .find(|export| export.name == name)
            .and_then(|export| export.alias_of.clone())
    };
    assert_eq!(alias("g"), Some("f".to_owned()));
    assert_eq!(alias("h"), None);
    assert_eq!(alias("cast"), None);
    assert_eq!(alias("bang"), None);
    assert_eq!(alias("f"), None);
}

#[test]
fn typeof_import_asserting_a_require_is_not_a_use_of_the_whole_module() {
    let info = parse(
        "a.ts",
        "const { used } = require('./x') as typeof import('./x');\nconst whole: typeof import('./y') = null as never;\n",
    );
    assert_eq!(imported(&info, "./x"), ["used"]);
    assert_eq!(imported(&info, "./y"), ["*"]);
}

#[test]
fn test_runner_mocks_are_mock_imports_and_strings_are_not() {
    let info = parse(
        "a.test.ts",
        "vi.mock('./dep');\njest.requireActual<typeof import('x')>('pkg');\njest.mock('virtual', () => ({}), { virtual: true });\nconst code = `vi.mock('inv1')`;\nconst text = \"jest.mock('inv2')\";\nrs.mock('./rs-dep', () => ({}));\nrstest.importActual('rs-pkg');\nrs.doUnmockRequire('rs-cjs');\n",
    );
    let mocks: Vec<&str> = info
        .imports
        .iter()
        .filter(|import| import.kind == ImportKind::Mock)
        .map(|import| import.specifier.as_str())
        .collect();
    assert_eq!(mocks, ["./dep", "pkg", "./rs-dep", "rs-pkg", "rs-cjs"]);
}

/// A mock records the call as written, so a rule can name it (#111); other imports record none.
#[test]
fn mock_imports_record_the_call_as_written() {
    let info = parse(
        "a.test.ts",
        "vi.mock('./a');\nvi.doMock('./b');\nvi.unmock('./c');\nvi.importActual('./d');\njest.mock('./e');\n\
         jest.doMock('./f');\njest.requireActual('./g');\nimport './h';\n",
    );
    let calls: Vec<(&str, Option<&str>)> = info
        .imports
        .iter()
        .map(|import| (import.specifier.as_str(), import.call.map(MockCall::as_str)))
        .collect();
    assert_eq!(
        calls,
        [
            ("./a", Some("vi.mock")),
            ("./b", Some("vi.doMock")),
            ("./c", Some("vi.unmock")),
            ("./d", Some("vi.importActual")),
            ("./e", Some("jest.mock")),
            ("./f", Some("jest.doMock")),
            ("./g", Some("jest.requireActual")),
            ("./h", None),
        ]
    );
}

/// Vitest's typed form names the module through an `import()`, which is the mock's and not a dynamic import (#111).
#[test]
fn a_mock_of_an_import_expression_is_a_mock_import_only() {
    let info = parse(
        "a.test.ts",
        "vi.mock(import('./gone'), () => ({}));\nvi.doMock(import('./also-gone'));\nimport('./lazy');\n",
    );
    let imports: Vec<(&str, ImportKind, Option<&str>)> = info
        .imports
        .iter()
        .map(|import| {
            (
                import.specifier.as_str(),
                import.kind,
                import.call.map(MockCall::as_str),
            )
        })
        .collect();
    assert_eq!(
        imports,
        [
            ("./gone", ImportKind::Mock, Some("vi.mock")),
            ("./also-gone", ImportKind::Mock, Some("vi.doMock")),
            ("./lazy", ImportKind::Dynamic, None),
        ]
    );
}

/// The parse cache stores a mock's call as it is written.
#[test]
fn a_mock_call_serializes_as_written() {
    for call in MockCall::ALL {
        let json = serde_json::to_string(&call).unwrap();
        assert_eq!(json, format!("\"{}\"", call.as_str()));
        assert_eq!(serde_json::from_str::<MockCall>(&json).unwrap(), call);
        let (object, property) = call.as_str().split_once('.').unwrap();
        assert_eq!(MockCall::find(object, property), Some(call));
    }
    assert_eq!(MockCall::find("vi", "fn"), None);
    assert_eq!(MockCall::find("jest", "importActual"), None);
}

#[test]
fn imports_inside_declare_module_bodies_are_imports() {
    let info = parse(
        "types.d.ts",
        "declare module 'compiled/accept' {\n  import m from '@hapi/accept';\n  export = m;\n}\n",
    );
    assert_eq!(imported(&info, "@hapi/accept"), ["default"]);
}

#[test]
fn reexports_inside_declare_module_bodies_are_type_imports_but_no_exports() {
    let info = parse(
        "types.d.ts",
        "declare module 'commander' {\n  export * from '@commander-js/extra-typings';\n}\n\
         declare module 'compiled/is' {\n  export { isElement as is } from 'react-is';\n  \
         export type { Kind } from 'kinds';\n}\n",
    );
    assert_eq!(imported(&info, "@commander-js/extra-typings"), ["*"]);
    assert_eq!(imported(&info, "react-is"), ["isElement"]);
    assert_eq!(imported(&info, "kinds"), ["Kind"]);
    let kinds: Vec<ImportKind> = info.imports.iter().map(|import| import.kind).collect();
    // The block declares types only, so what it re-exports is a type-only import.
    assert_eq!(kinds, [ImportKind::TypeOnly; 3]);
    let types: Vec<bool> = info
        .imports
        .iter()
        .flat_map(|import| &import.names)
        .map(|name| name.is_type)
        .collect();
    assert_eq!(types, [true; 3]);
    assert!(info.exports.is_empty(), "{:?}", info.exports);
    assert_eq!(info.ambient_modules, ["commander", "compiled/is"]);
}

#[test]
fn relative_reexports_inside_declare_module_bodies_are_no_imports() {
    let info = parse(
        "types.d.ts",
        "declare module 'kit' {\n  export * from './local';\n  export { a } from '../up';\n  \
         export * from '/abs/types';\n  export * from 'kit-types';\n}\n",
    );
    let specifiers: Vec<&str> = info.imports.iter().map(|import| import.specifier.as_str()).collect();
    assert_eq!(specifiers, ["kit-types"]);
    assert!(info.exports.is_empty(), "{:?}", info.exports);
}

#[test]
fn declare_module_in_a_module_file_is_an_augmentation() {
    let info = parse(
        "next-auth.d.ts",
        "import 'next-auth';\ndeclare module 'next-auth' { interface Session { id: string } }\n",
    );
    assert_eq!(info.ambient_modules, [] as [&str; 0]);
    assert_eq!(info.augmented_modules, ["next-auth"]);
    for source in [
        "export {};\ndeclare module 'dayjs' {}\n",
        "import x = require('y');\ndeclare module 'dayjs' {}\n",
    ] {
        let info = parse("types.d.ts", source);
        assert_eq!(info.augmented_modules, ["dayjs"], "{source}");
    }
}

#[test]
fn only_the_declarations_of_value_exports_contain_types() {
    type Case<'a> = (&'a str, &'a str, &'a [(&'a str, &'a [&'a str])]);
    let cases: &[Case<'_>] = &[
        (
            "a local function and a type-only module",
            "export type P = { a: string };\nfunction local(p: P): P { return p; }\nexport type Q = { p: P };\n",
            &[("P", &[]), ("Q", &[])],
        ),
        (
            "a local function next to a value export",
            "export type P = { a: string };\nfunction local(p: P): P { return p; }\nexport const v = 1;\n",
            &[("P", &[]), ("v", &[])],
        ),
        (
            "a local declaration exported by a clause",
            "export type P = { a: string };\nfunction local(p: P): P { return p; }\nexport { local as run };\n",
            &[("P", &["run"]), ("run", &[])],
        ),
    ];
    for (label, source, expected) in cases {
        let info = parse("a.ts", source);
        let containing: Vec<(&str, Vec<&str>)> = info
            .exports
            .iter()
            .map(|export| {
                (
                    export.name.as_str(),
                    export.referenced_in.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        let expected: Vec<(&str, Vec<&str>)> = expected.iter().map(|(name, list)| (*name, list.to_vec())).collect();
        assert_eq!(containing, expected, "{label}");
    }
}
