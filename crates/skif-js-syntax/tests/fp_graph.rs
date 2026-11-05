//! Module facts behind the graph false-positive fixes: promise callbacks, ambient modules, signature references.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use skif_js_syntax::{ImportedKind, ModuleInfo, ParseOptions, parse_module};

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
fn only_members_read_by_name_in_the_declaring_file_are_recorded() {
    let info = parse(
        "a.ts",
        "export enum Color { Red, Green, Blue }\n\
         export const first = Color.Red;\n\
         export const second = Color['Green'];\n\
         export function describe(c: Color, k: keyof typeof Color): string { return String(c) + k; }\n\
         export enum Mode { A, B }\n\
         export const modes = Object.values(Mode);\n\
         export const copy = { ...Mode };\n\
         export const bare = Mode;\n\
         export const dynamic = (k: string) => Mode[k];\n\
         export function each() { for (const k in Mode) { console.log(k); } }\n\
         export enum Hidden { A, B }\n\
         export { Hidden as Renamed };\n\
         export const { A } = Mode;\n",
    );
    let read = |name: &str| {
        info.exports
            .iter()
            .find(|export| export.name == name)
            .map(|export| export.members_read_in_file.clone())
            .unwrap()
    };
    assert_eq!(read("Color"), ["Green", "Red"]);
    assert_eq!(read("Mode"), ["A"]);
    assert_eq!(read("Hidden"), Vec::<String>::new());
}

#[test]
fn a_constant_records_the_identifier_it_is_initialised_with() {
    let info = parse("a.ts", "export function f() {}\nexport const g = f;\nexport const h = f();\n");
    let alias = |name: &str| {
        info.exports
            .iter()
            .find(|export| export.name == name)
            .and_then(|export| export.alias_of.clone())
    };
    assert_eq!(alias("g"), Some("f".to_owned()));
    assert_eq!(alias("h"), None);
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
