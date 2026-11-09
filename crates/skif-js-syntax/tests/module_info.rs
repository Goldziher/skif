//! Table-driven tests for `parse_module`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use skif_js_syntax::{
    ExportKind, ImportKind, ImportedKind, ModuleInfo, ParseOptions, Registration, RemovalKind, SyntaxError,
    parse_module,
};

fn parse(path: &str, source: &str) -> ModuleInfo {
    parse_module(Path::new(path), source, &ParseOptions::default()).expect("parses")
}

fn slice(source: &str, start: u32, end: u32) -> &str {
    &source[start as usize..end as usize]
}

#[test]
fn import_forms() {
    struct Case {
        name: &'static str,
        path: &'static str,
        source: &'static str,
        specifier: &'static str,
        kind: ImportKind,
        names: Vec<(ImportedKind, Option<&'static str>, bool)>,
    }
    use ImportedKind::{Default, Named, Namespace};
    let cases = vec![
        Case {
            name: "default",
            path: "a.js",
            source: "import a from 'x';",
            specifier: "x",
            kind: ImportKind::Static,
            names: vec![(Default, Some("a"), false)],
        },
        Case {
            name: "named with rename",
            path: "a.js",
            source: "import { a as b, default as c } from 'x';",
            specifier: "x",
            kind: ImportKind::Static,
            names: vec![(Named("a".into()), Some("b"), false), (Default, Some("c"), false)],
        },
        Case {
            name: "namespace",
            path: "a.js",
            source: "import * as ns from 'x';",
            specifier: "x",
            kind: ImportKind::Static,
            names: vec![(Namespace, Some("ns"), false)],
        },
        Case {
            name: "type only declaration",
            path: "a.ts",
            source: "import type { T } from 'x';",
            specifier: "x",
            kind: ImportKind::TypeOnly,
            names: vec![(Named("T".into()), Some("T"), true)],
        },
        Case {
            name: "inline type specifier",
            path: "a.ts",
            source: "import { type T, v } from 'x';",
            specifier: "x",
            kind: ImportKind::Static,
            names: vec![
                (Named("T".into()), Some("T"), true),
                (Named("v".into()), Some("v"), false),
            ],
        },
        Case {
            name: "side effect",
            path: "a.js",
            source: "import 'x/polyfill';",
            specifier: "x/polyfill",
            kind: ImportKind::SideEffect,
            names: vec![],
        },
        Case {
            name: "dynamic literal",
            path: "a.js",
            source: "const m = import('x');",
            specifier: "x",
            kind: ImportKind::Dynamic,
            names: vec![(Namespace, Some("m"), false)],
        },
        Case {
            name: "dynamic template without expressions",
            path: "a.js",
            source: "import(`x`);",
            specifier: "x",
            kind: ImportKind::Dynamic,
            names: vec![(Namespace, None, false)],
        },
        Case {
            name: "awaited dynamic destructure",
            path: "a.js",
            source: "async function f() { const { a, default: d } = await import('x'); }",
            specifier: "x",
            kind: ImportKind::Dynamic,
            names: vec![(Named("a".into()), Some("a"), false), (Default, Some("d"), false)],
        },
        Case {
            name: "require bound",
            path: "a.cjs",
            source: "const x = require('x');",
            specifier: "x",
            kind: ImportKind::Require,
            names: vec![(Namespace, Some("x"), false)],
        },
        Case {
            name: "require destructured",
            path: "a.cjs",
            source: "const { a: b } = require('x');",
            specifier: "x",
            kind: ImportKind::Require,
            names: vec![(Named("a".into()), Some("b"), false)],
        },
        Case {
            name: "require member",
            path: "a.cjs",
            source: "require('x').thing();",
            specifier: "x",
            kind: ImportKind::Require,
            names: vec![(Named("thing".into()), None, false)],
        },
        Case {
            name: "require resolve",
            path: "a.cjs",
            source: "require.resolve('x');",
            specifier: "x",
            kind: ImportKind::Require,
            names: vec![],
        },
        Case {
            name: "import meta resolve",
            path: "a.mjs",
            source: "const url = import.meta.resolve('x');",
            specifier: "x",
            kind: ImportKind::Require,
            names: vec![],
        },
        Case {
            name: "import equals require",
            path: "a.ts",
            source: "import fs = require('fs');",
            specifier: "fs",
            kind: ImportKind::Require,
            names: vec![(Namespace, Some("fs"), false)],
        },
        Case {
            name: "export from",
            path: "a.js",
            source: "export { a as b, default } from 'x';",
            specifier: "x",
            kind: ImportKind::ExportFrom,
            names: vec![(Named("a".into()), None, false), (Default, None, false)],
        },
        Case {
            name: "export star from",
            path: "a.js",
            source: "export * from 'x';",
            specifier: "x",
            kind: ImportKind::ExportFrom,
            names: vec![(Namespace, None, false)],
        },
    ];
    for case in cases {
        let info = parse(case.path, case.source);
        assert_eq!(info.imports.len(), 1, "{}", case.name);
        let import = &info.imports[0];
        assert_eq!(import.specifier, case.specifier, "{}", case.name);
        assert_eq!(import.kind, case.kind, "{}", case.name);
        let got: Vec<_> = import
            .names
            .iter()
            .map(|n| (n.kind.clone(), n.local.as_deref(), n.is_type))
            .collect();
        assert_eq!(got, case.names, "{}", case.name);
    }
}

#[test]
fn unresolved_dynamic_references_are_recorded() {
    let info = parse(
        "a.js",
        "import(name); require(name); import(`${n}/x.js`); import.meta.glob(pattern);",
    );
    assert_eq!(info.imports, [] as [skif_js_syntax::Import; 0]);
    assert_eq!(info.unresolved_dynamic.len(), 4);
}

#[test]
fn computed_relative_specifiers_are_globs() {
    let info = parse(
        "a.js",
        "require(`./commands/${name}`); import(`../pages/${a}/${b}.js`); require(`${root}/x.js`);",
    );
    let got: Vec<_> = info.imports.iter().map(|i| (i.specifier.as_str(), i.kind)).collect();
    assert_eq!(
        got,
        [
            ("./commands/*", ImportKind::ImportMetaGlob),
            ("../pages/*/*.js", ImportKind::ImportMetaGlob),
        ]
    );
    assert_eq!(info.unresolved_dynamic.len(), 1);
}

#[test]
fn import_meta_glob_yields_one_import_per_pattern() {
    let info = parse(
        "a.ts",
        "const m = import.meta.glob(['./a/*.ts', '!./a/skip.ts'], { eager: true });",
    );
    let got: Vec<_> = info.imports.iter().map(|i| (i.specifier.as_str(), i.kind)).collect();
    assert_eq!(
        got,
        [
            ("./a/*.ts", ImportKind::ImportMetaGlob),
            ("!./a/skip.ts", ImportKind::ImportMetaGlob)
        ]
    );
    let single = parse("a.ts", "import.meta.glob('./b/*.ts');");
    assert_eq!(single.imports.len(), 1);
}

#[test]
fn import_meta_glob_options_that_read_source_text() {
    let cases = [
        ("{ query: '?raw', eager: true }", ImportKind::SourceGlob),
        ("{ query: 'raw&inline' }", ImportKind::SourceGlob),
        ("{ query: { raw: '' } }", ImportKind::SourceGlob),
        ("{ as: 'raw' }", ImportKind::SourceGlob),
        // Vite writes `{ raw: true }` as `raw=true`, which is no `?raw`.
        ("{ query: { raw: true } }", ImportKind::ImportMetaGlob),
        ("{ query: '?url' }", ImportKind::ImportMetaGlob),
        ("{ as: 'url' }", ImportKind::ImportMetaGlob),
        ("{ query: '?inline' }", ImportKind::ImportMetaGlob),
        ("{ query: '?worker' }", ImportKind::ImportMetaGlob),
        ("{ eager: true }", ImportKind::ImportMetaGlob),
        // Options that are not static stay module imports.
        ("{ query: q }", ImportKind::ImportMetaGlob),
        ("{ ...options, query: '?raw' }", ImportKind::ImportMetaGlob),
        ("options", ImportKind::ImportMetaGlob),
    ];
    for (options, kind) in cases {
        let info = parse("a.ts", &format!("import.meta.glob('./b/*.vue', {options});"));
        let got: Vec<_> = info.imports.iter().map(|i| (i.specifier.as_str(), i.kind)).collect();
        assert_eq!(got, [("./b/*.vue", kind)], "{options}");
    }
}

#[test]
fn export_forms() {
    struct Case {
        name: &'static str,
        path: &'static str,
        source: &'static str,
        expected: Vec<(&'static str, ExportKind, bool)>,
    }
    use ExportKind::{Class, Default, Enum, Function, Namespace, ReExport, StarReExport, Type, Value};
    let cases = vec![
        Case {
            name: "declarations",
            path: "a.ts",
            source: "export const a = 1, b = 2;\nexport function f() {}\nexport class C {}\nexport interface I {}\nexport type T = string;\nexport enum E { A }\nexport namespace N { export const x = 1; }",
            expected: vec![
                ("a", Value, false),
                ("b", Value, false),
                ("f", Function, false),
                ("C", Class, false),
                ("I", Type, true),
                ("T", Type, true),
                ("E", Enum, false),
                ("N", Namespace, false),
            ],
        },
        Case {
            name: "destructured binding",
            path: "a.js",
            source: "export const { a, b: [c], ...d } = obj;",
            expected: vec![("a", Value, false), ("c", Value, false), ("d", Value, false)],
        },
        Case {
            name: "clause with rename and kind lookup",
            path: "a.ts",
            source: "function f() {}\ninterface I {}\nconst v = 1;\nexport { f as g, I, v };",
            expected: vec![("g", Function, false), ("I", Type, true), ("v", Value, false)],
        },
        Case {
            name: "type clause",
            path: "a.ts",
            source: "const v = 1;\nexport type { v };",
            expected: vec![("v", Value, true)],
        },
        Case {
            name: "default forms",
            path: "a.js",
            source: "export default function () {}",
            expected: vec![("default", Default, false)],
        },
        Case {
            name: "default expression",
            path: "a.js",
            source: "const x = 1;\nexport default x;",
            expected: vec![("default", Default, false)],
        },
        Case {
            name: "as default",
            path: "a.js",
            source: "const x = 1;\nexport { x as default };",
            expected: vec![("default", Default, false)],
        },
        Case {
            name: "re-exports",
            path: "a.ts",
            source: "export * from 'a';\nexport * as ns from 'b';\nexport { a as b } from 'c';\nexport type { T } from 'd';",
            expected: vec![
                ("*", StarReExport, false),
                ("ns", StarReExport, false),
                ("b", ReExport, false),
                ("T", ReExport, true),
            ],
        },
    ];
    for case in cases {
        let info = parse(case.path, case.source);
        let got: Vec<_> = info
            .exports
            .iter()
            .map(|e| (e.name.as_str(), e.kind, e.is_type))
            .collect();
        assert_eq!(got, case.expected, "{}", case.name);
    }
}

#[test]
fn re_export_records_origin() {
    let info = parse("a.ts", "export { a as b } from './c';\nexport * as ns from './d';");
    assert_eq!(info.exports[0].from.as_deref(), Some("./c"));
    assert_eq!(info.exports[0].imported_name.as_deref(), Some("a"));
    assert_eq!(info.exports[1].from.as_deref(), Some("./d"));
    assert_eq!(info.exports[1].imported_name.as_deref(), Some("*"));
}

#[test]
fn enum_and_namespace_members() {
    let source = "/** @public */\nexport enum E {\n  /** @internal */\n  A = 1,\n  'b-c' = 2,\n}\nexport namespace N {\n  export const x = 1;\n  export function f() {}\n  const hidden = 2;\n}\n";
    let info = parse("a.ts", source);
    let e = &info.exports[0];
    assert_eq!(e.tags, ["public"]);
    let members: Vec<_> = e.enum_members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(members, ["A", "b-c"]);
    assert_eq!(e.enum_members[0].tags, ["internal"]);
    assert_eq!(
        slice(source, e.enum_members[0].span.start, e.enum_members[0].span.start + 1),
        "A"
    );
    let n = &info.exports[1];
    let members: Vec<_> = n.namespace_members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(members, ["x", "f"]);
}

/// What deleting each member's removal range leaves of `source`, per member name; `None` for a member without one.
fn member_removals(source: &str, members: &[skif_js_syntax::Member]) -> Vec<(String, Option<String>)> {
    members
        .iter()
        .map(|member| {
            let left = member
                .removal
                .map(|(start, end)| format!("{}{}", &source[..start as usize], &source[end as usize..]));
            (member.name.clone(), left)
        })
        .collect()
}

#[test]
fn member_removal_ranges_keep_the_layout() {
    let source = "export enum E {\n  A = 1,\n  B = A + 1,\n  C\n}\nexport enum F { X, Y }\nexport namespace N {\n  export const x = 1;\n  export const y = x, z = 2;\n}\n";
    let info = parse("a.ts", source);
    let enum_e = member_removals(source, &info.exports[0].enum_members);
    assert_eq!(enum_e[0], ("A".to_owned(), None), "B mentions A");
    assert_eq!(enum_e[1], ("B".to_owned(), None), "C would be renumbered");
    assert_eq!(
        enum_e[2].1.as_deref(),
        Some(
            "export enum E {\n  A = 1,\n  B = A + 1,\n}\nexport enum F { X, Y }\nexport namespace N {\n  export const x = 1;\n  export const y = x, z = 2;\n}\n"
        )
    );
    let enum_f = member_removals(source, &info.exports[1].enum_members);
    assert_eq!(enum_f[0], ("X".to_owned(), None), "Y would be renumbered");
    assert!(
        enum_f[1].1.as_deref().unwrap().contains("export enum F { X, }"),
        "{enum_f:?}"
    );
    let namespace = member_removals(source, &info.exports[2].namespace_members);
    let names: Vec<(&str, bool)> = namespace
        .iter()
        .map(|(name, left)| (name.as_str(), left.is_some()))
        .collect();
    assert_eq!(
        names,
        [("x", false), ("y", false), ("z", false)],
        "x is read by y; y and z share a statement"
    );
    let source = "export namespace N {\n  export const a = 1;\n  export function b() {}\n}\n";
    let info = parse("b.ts", source);
    let namespace = member_removals(source, &info.exports[0].namespace_members);
    assert_eq!(
        namespace[0].1.as_deref(),
        Some("export namespace N {\n  export function b() {}\n}\n")
    );
    assert_eq!(
        namespace[1].1.as_deref(),
        Some("export namespace N {\n  export const a = 1;\n}\n")
    );
}

#[test]
fn enum_member_removal_never_renumbers_a_later_member() {
    let source = "export enum Color { Red, Green, Blue }\nexport enum Level { Low = 1, Mid, High = 10 }\nexport enum Name { A = \"a\", B = \"b\" }\nexport enum Flag { On = 1, Off = 0 }\n";
    let info = parse("a.ts", source);
    let fixable = |index: usize| -> Vec<(String, bool)> {
        info.exports[index]
            .enum_members
            .iter()
            .map(|member| (member.name.clone(), member.removal.is_some()))
            .collect()
    };
    let pairs = |names: &[(&str, bool)]| -> Vec<(String, bool)> {
        names.iter().map(|(name, fix)| ((*name).to_owned(), *fix)).collect()
    };
    assert_eq!(fixable(0), pairs(&[("Red", false), ("Green", false), ("Blue", true)]));
    assert_eq!(fixable(1), pairs(&[("Low", false), ("Mid", true), ("High", true)]));
    assert_eq!(fixable(2), pairs(&[("A", true), ("B", true)]));
    assert_eq!(fixable(3), pairs(&[("On", true), ("Off", true)]));
}

#[test]
fn member_removal_takes_the_comments_of_the_member() {
    let source = "export enum E {\n  A = 1,\n  /**\n   * Docs of B.\n   */\n  B = 2,\n  // note on C\n  C = 3, // trailing\n  D = 4,\n}\n";
    let info = parse("a.ts", source);
    let left = member_removals(source, &info.exports[0].enum_members);
    assert_eq!(
        left[1].1.as_deref(),
        Some("export enum E {\n  A = 1,\n  // note on C\n  C = 3, // trailing\n  D = 4,\n}\n")
    );
    assert_eq!(
        left[2].1.as_deref(),
        Some("export enum E {\n  A = 1,\n  /**\n   * Docs of B.\n   */\n  B = 2,\n  D = 4,\n}\n")
    );

    let source = "export namespace N {\n  /** First. */\n  export const a = 1; // trailing\n\n  // Second.\n  export const b = 2;\n}\n";
    let info = parse("b.ts", source);
    let left = member_removals(source, &info.exports[0].namespace_members);
    assert_eq!(
        left[0].1.as_deref(),
        Some("export namespace N {\n\n  // Second.\n  export const b = 2;\n}\n")
    );
    assert_eq!(
        left[1].1.as_deref(),
        Some("export namespace N {\n  /** First. */\n  export const a = 1; // trailing\n\n}\n")
    );

    let source = "export enum F { X = 1, /** y */ Y = 2, Z = 3 /* z */ }\n";
    let info = parse("c.ts", source);
    let fixable: Vec<bool> = info.exports[0]
        .enum_members
        .iter()
        .map(|member| member.removal.is_some())
        .collect();
    assert_eq!(
        fixable,
        [true, false, false],
        "a comment next to a member on a shared line keeps it"
    );
}

#[test]
fn default_expression_removal_keeps_side_effects() {
    let removable = |source: &str| parse("a.ts", source).exports[0].removal.is_some();
    for source in [
        "export default 42;\n",
        "export default -1;\n",
        "export default `plain`;\n",
        "export default value;\n",
        "export default () => {};\n",
        "export default function () {}\n",
        "export default class { method() {} static x = 1; }\n",
        "export default [1, \"a\", , { b: [c], d() {} }];\n",
        "export default ({ a: 1 } as const);\n",
    ] {
        assert!(removable(source), "{source}");
    }
    for source in [
        "export default registerPlugin();\n",
        "export default new Thing();\n",
        "export default a.b;\n",
        "export default `x${y}`;\n",
        "export default [...items];\n",
        "export default { ...rest };\n",
        "export default { [key()]: 1 };\n",
        "export default { a: setup() };\n",
        "export default class extends mixin(Base) {}\n",
        "export default class { static { init(); } }\n",
        "export default class { static x = init(); }\n",
        "export default (x = 1);\n",
    ] {
        assert!(!removable(source), "{source}");
    }
}

#[test]
fn jsdoc_tags_attach_to_exports() {
    let source = "/**\n * Docs.\n * @public\n * @alias foo\n */\nexport const a = 1;\n\n// plain\nexport const b = 2;\n/** @internal */ export function c() {}\n";
    let info = parse("a.ts", source);
    assert_eq!(info.exports[0].tags, ["public", "alias"]);
    assert_eq!(info.exports[1].tags, [] as [std::string::String; 0]);
    assert_eq!(info.exports[2].tags, ["internal"]);
}

#[test]
fn in_file_references() {
    let source = "export const used = 1;\nexport const unused = 2;\nexport function f() { return used; }\nconst local = 3;\nexport { local };\nexport interface I {}\nexport const typed: I = 1;\n";
    let info = parse("a.ts", source);
    assert_eq!(info.in_file_references, ["I", "used"]);
    let flags: Vec<_> = info
        .exports
        .iter()
        .map(|e| (e.name.as_str(), e.referenced_in_file))
        .collect();
    assert_eq!(
        flags,
        [
            ("used", true),
            ("unused", false),
            ("f", false),
            ("local", false),
            ("I", true),
            ("typed", false)
        ]
    );
}

#[test]
fn in_file_references_can_be_disabled() {
    let options = ParseOptions {
        in_file_references: false,
        ..ParseOptions::default()
    };
    let info = parse_module(Path::new("a.js"), "export const a = 1; a;", &options).unwrap();
    assert_eq!(info.in_file_references, [] as [std::string::String; 0]);
}

#[test]
fn removal_ranges_slice_the_source() {
    struct Case {
        name: &'static str,
        source: &'static str,
        export: usize,
        kind: RemovalKind,
        removed: &'static str,
        siblings: u32,
    }
    let cases = vec![
        Case {
            name: "declaration keyword",
            source: "export const a = 1;\n",
            export: 0,
            kind: RemovalKind::ExportKeyword,
            removed: "export ",
            siblings: 1,
        },
        Case {
            name: "shared keyword",
            source: "export const a = 1, b = 2;\n",
            export: 1,
            kind: RemovalKind::SharedExportKeyword,
            removed: "export ",
            siblings: 2,
        },
        Case {
            name: "default declaration",
            source: "export default function f() {}\n",
            export: 0,
            kind: RemovalKind::ExportKeyword,
            removed: "export default ",
            siblings: 1,
        },
        Case {
            name: "default expression is whole statement",
            source: "export default 42;\nconst z = 1;\n",
            export: 0,
            kind: RemovalKind::Statement,
            removed: "export default 42;\n",
            siblings: 1,
        },
        Case {
            name: "anonymous default function is whole statement",
            source: "export default function () {}\n",
            export: 0,
            kind: RemovalKind::Statement,
            removed: "export default function () {}\n",
            siblings: 1,
        },
        Case {
            name: "sole specifier removes statement",
            source: "const a = 1;\nexport { a };\nconst z = 1;\n",
            export: 0,
            kind: RemovalKind::Statement,
            removed: "export { a };\n",
            siblings: 1,
        },
        Case {
            name: "first of several specifiers",
            source: "const a = 1, b = 2;\nexport { a, b };\n",
            export: 0,
            kind: RemovalKind::Specifier,
            removed: "a, ",
            siblings: 2,
        },
        Case {
            name: "last of several specifiers",
            source: "const a = 1, b = 2;\nexport { a, b };\n",
            export: 1,
            kind: RemovalKind::Specifier,
            removed: ", b",
            siblings: 2,
        },
        Case {
            name: "re-export statement",
            source: "export * from 'x';\n",
            export: 0,
            kind: RemovalKind::Statement,
            removed: "export * from 'x';\n",
            siblings: 1,
        },
        Case {
            name: "multi-byte content before the export",
            source: "const π = 'ß→';\nexport const café = π;\n",
            export: 0,
            kind: RemovalKind::ExportKeyword,
            removed: "export ",
            siblings: 1,
        },
    ];
    for case in cases {
        let info = parse("a.ts", case.source);
        let removal = info.exports[case.export].removal.expect(case.name);
        assert_eq!(removal.kind, case.kind, "{}", case.name);
        assert_eq!(
            slice(case.source, removal.start, removal.end),
            case.removed,
            "{}",
            case.name
        );
        assert_eq!(removal.siblings, case.siblings, "{}", case.name);
    }
}

#[test]
fn spans_are_utf8_byte_offsets_with_scalar_columns() {
    let source = "const s = 'héllo→';\nexport const ünï = 1;\n";
    let info = parse("a.ts", source);
    let span = info.exports[0].span;
    assert_eq!(slice(source, span.start, span.end), "ünï");
    assert_eq!((span.start_line, span.start_col), (2, 14));
}

#[test]
fn member_accesses_through_imports() {
    let source = "import * as ns from 'x';\nimport { E } from 'y';\nimport def from 'z';\nns.a; ns['b']; E.A; const { c, d } = ns; let t: ns.T;\nuse(def);\n";
    let info = parse("a.ts", source);
    let ns = &info.imports[0].names[0];
    assert_eq!(ns.accessed_members, ["T", "a", "b", "c", "d"]);
    assert!(!ns.escapes);
    let e = &info.imports[1].names[0];
    assert_eq!(e.accessed_members, ["A"]);
    assert!(!e.escapes);
    let def = &info.imports[2].names[0];
    assert_eq!(def.accessed_members, [] as [std::string::String; 0]);
    assert!(def.escapes);
}

#[test]
fn imports_awaited_together_bind_the_element_of_the_array_pattern_at_their_position() {
    let source = "async function f() {\n  const [{ a }, ns, , skipped, ...rest] = await Promise.all([\n    import('./x'),\n    import('./y'),\n    import('./hole'),\n    import('./skipped'),\n    import('./rest'),\n  ]);\n  ns.b;\n  use(a, rest);\n}\n";
    let info = parse("a.ts", source);
    let by_specifier = |specifier: &str| {
        info.imports
            .iter()
            .find(|import| import.specifier == specifier)
            .unwrap_or_else(|| panic!("no import of {specifier}"))
    };
    let x = &by_specifier("./x").names;
    assert_eq!(x.len(), 1);
    assert_eq!(x[0].kind, ImportedKind::Named("a".into()));
    assert_eq!(x[0].local.as_deref(), Some("a"));
    let y = &by_specifier("./y").names[0];
    assert_eq!(y.kind, ImportedKind::Namespace);
    assert_eq!(y.accessed_members, ["b"]);
    assert!(!y.escapes, "`ns.b` reads one member");
    let skipped = &by_specifier("./skipped").names[0];
    assert_eq!(skipped.kind, ImportedKind::Namespace);
    assert_eq!(skipped.local.as_deref(), Some("skipped"));
    assert!(
        skipped.accessed_members.is_empty() && !skipped.escapes,
        "a binding never read uses nothing"
    );
    for whole in ["./hole", "./rest"] {
        let names = &by_specifier(whole).names;
        assert!(
            names.len() == 1 && names[0].kind == ImportedKind::Namespace && names[0].escapes,
            "{whole}: {names:?}"
        );
    }
}

#[test]
fn imports_awaited_together_after_a_spread_are_used_as_a_whole() {
    let source = "async function f() {\n  const [a, b] = await Promise.all([...others, import('./x')]);\n  a.c;\n}\n";
    let info = parse("a.ts", source);
    let names = &info.imports[0].names;
    assert!(names.len() == 1 && names[0].escapes, "{names:?}");
}

#[test]
fn a_spy_on_a_namespace_member_reads_that_member() {
    for source in [
        "import * as ns from 'x'; vi.spyOn(ns, 'a').mockReturnValue(1);",
        "import * as ns from 'x'; jest.spyOn(ns, 'a');",
        "import * as ns from 'x'; spyOn(ns, `a`);",
    ] {
        let info = parse("a.test.ts", source);
        let ns = &info.imports[0].names[0];
        assert_eq!(ns.accessed_members, ["a"], "{source}");
        assert!(!ns.escapes, "{source}");
    }
    for source in [
        "import * as ns from 'x'; vi.spyOn(ns, name);",
        "import * as ns from 'x'; other.spyOn(ns, 'a');",
    ] {
        let info = parse("a.test.ts", source);
        assert!(info.imports[0].names[0].escapes, "{source}");
    }
}

#[test]
fn namespace_that_escapes_is_flagged() {
    for source in [
        "import * as ns from 'x'; use(ns);",
        "import * as ns from 'x'; ns[key];",
        "import * as ns from 'x'; const { a, ...rest } = ns;",
        "import * as ns from 'x'; export default ns;",
        "import * as ns from 'x'; export { ns }; use(ns);",
    ] {
        let info = parse("a.ts", source);
        assert!(info.imports[0].names[0].escapes, "{source}");
    }
}

#[test]
fn an_export_clause_of_an_imported_binding_is_a_re_export_edge() {
    let info = parse(
        "a.ts",
        "import * as ns from './ns';\nimport d, { n, type T } from './m';\nexport { ns, d as renamed, n, T };\n",
    );
    let exports: Vec<_> = info
        .exports
        .iter()
        .map(|e| {
            (
                e.name.as_str(),
                e.kind,
                e.from.as_deref(),
                e.imported_name.as_deref(),
                e.is_type,
            )
        })
        .collect();
    assert_eq!(
        exports,
        [
            ("ns", ExportKind::StarReExport, Some("./ns"), Some("*"), false),
            ("renamed", ExportKind::ReExport, Some("./m"), Some("default"), false),
            ("n", ExportKind::ReExport, Some("./m"), Some("n"), false),
            ("T", ExportKind::ReExport, Some("./m"), Some("T"), true),
        ]
    );
    let namespace = &info.imports[0].names[0];
    assert!(!namespace.escapes, "the export clause alone is not an escaping use");
    assert_eq!(info.imports.len(), 2, "no import is added for the re-export");
}

#[test]
fn a_local_export_clause_stays_a_local_export() {
    let info = parse("a.ts", "const a = 1;\nexport { a };\n");
    assert_eq!(info.exports[0].kind, ExportKind::Value);
    assert_eq!(info.exports[0].from, None);
}

#[test]
fn commonjs_and_jsx_and_directives() {
    assert!(parse("a.js", "module.exports = {};").has_cjs_exports);
    assert!(parse("a.js", "module.exports.a = 1;").has_cjs_exports);
    assert!(parse("a.js", "exports.a = 1;").has_cjs_exports);
    assert!(!parse("a.js", "const exports2 = {}; exports2.a = 1;").has_cjs_exports);
    assert!(parse("a.jsx", "const a = <div />;").has_jsx);
    assert!(parse("a.tsx", "const a = <></>;").has_jsx);
    assert!(!parse("a.ts", "const a = 1;").has_jsx);
    let info = parse(
        "a.tsx",
        "/** @jsxImportSource preact */\n'use client';\nexport const a = 1;",
    );
    assert_eq!(info.jsx_import_source.as_deref(), Some("preact"));
    assert_eq!(info.directives, ["use client"]);
}

#[test]
fn source_type_follows_extension() {
    for (path, source) in [
        ("a.js", "export const a = 1;"),
        ("a.mjs", "export const a = 1;"),
        ("a.cjs", "const a = 1; module.exports = a;"),
        ("a.jsx", "export const a = <b />;"),
        ("a.ts", "export const a: number = 1;"),
        ("a.mts", "export const a: number = 1;"),
        ("a.cts", "const a: number = 1; export = a;"),
        ("a.tsx", "export const a = <b />;"),
        ("a.d.ts", "export declare const a: number;"),
    ] {
        let info = parse(path, source);
        assert!(!info.exports.is_empty() || info.has_cjs_exports, "{path}");
    }
    let ts_only = parse("a.js", "export const a: number = 1;");
    assert_ne!(ts_only.parse_diagnostics, [] as [skif_js_syntax::ParseIssue; 0]);
}

#[test]
fn syntax_errors_are_reported_and_the_recovered_program_is_used() {
    // Mixing `??` and `||` without parentheses is a recoverable error: oxc reports it and keeps the program.
    let source = "import a from 'a';\nx ?? y || z;\nexport const kept = 1;\n";
    let info = parse("a.js", source);
    assert!(!info.fatal);
    assert_eq!(info.imports[0].specifier, "a");
    assert_eq!(info.exports[0].name, "kept");
    let issue = &info.parse_diagnostics[0];
    assert_ne!(issue.message, "");
    assert_eq!(issue.span.start_line, 2);
}

#[test]
fn clean_files_have_no_diagnostics() {
    let info = parse("a.js", "export const a = 1;");
    assert_eq!(info.parse_diagnostics, [] as [skif_js_syntax::ParseIssue; 0]);
    assert!(!info.fatal);
}

#[test]
fn unrecoverable_input_is_flagged_fatal() {
    // A missing binding name is a fatal error in oxc: the program comes back empty.
    let info = parse("a.js", "import a from 'a';\nconst = ;\n");
    assert!(info.fatal);
    assert_ne!(info.parse_diagnostics, [] as [skif_js_syntax::ParseIssue; 0]);
    assert!(info.imports.is_empty() && info.exports.is_empty());
}

#[test]
fn component_blocks_report_their_diagnostics_with_original_offsets() {
    let vue = "<template/>\n<script>\nx ?? y || z;\nexport const ok = 1;\n</script>\n";
    let info = parse("A.vue", vue);
    assert_eq!(info.exports[0].name, "ok");
    assert_eq!(info.parse_diagnostics[0].span.start_line, 3);
}

#[test]
fn unsupported_extensions() {
    for path in ["a.json", "a", "a.css"] {
        let err = parse_module(Path::new(path), "", &ParseOptions::default()).unwrap_err();
        assert!(matches!(err, SyntaxError::Unsupported { .. }), "{path}");
    }
}

#[test]
fn single_file_components_map_spans_back() {
    let vue = "<template><p/></template>\n<script setup lang=\"ts\">\nimport { ref } from 'vue';\nexport const answer = 42;\n</script>\n";
    let info = parse("App.vue", vue);
    assert_eq!(info.imports[0].specifier, "vue");
    let export = &info.exports[0];
    assert_eq!(slice(vue, export.span.start, export.span.end), "answer");
    let removal = export.removal.unwrap();
    assert_eq!(slice(vue, removal.start, removal.end), "export ");

    let astro = "---\nimport Layout from '../Layout.astro';\n---\n<Layout/>\n<script>import './client';</script>\n";
    let info = parse("page.astro", astro);
    let specifiers: Vec<_> = info.imports.iter().map(|i| i.specifier.as_str()).collect();
    assert_eq!(specifiers, ["../Layout.astro", "./client"]);

    let svelte = "<script>\n  import x from './x.js';\n</script>\n<p>{x}</p>";
    assert_eq!(parse("A.svelte", svelte).imports[0].specifier, "./x.js");
    assert_eq!(parse("B.vue", "<template/>").imports, [] as [skif_js_syntax::Import; 0]);
}

#[test]
fn mdx_yields_its_top_level_esm_with_original_offsets() {
    let mdx = "---\ntitle: Intro\n---\nimport { Card } from '@site/components';\nimport * as ui from './ui';\n\n# Intro\n\n```js\nimport fenced from 'not-a-dependency';\n```\n\n<Card><ui.Button/></Card>\n\nexport const meta = { draft: true };\n";
    let info = parse("docs/intro.mdx", mdx);
    let specifiers: Vec<_> = info.imports.iter().map(|i| i.specifier.as_str()).collect();
    assert_eq!(specifiers, ["@site/components", "./ui"]);
    let card = &info.imports[0];
    assert_eq!(
        slice(mdx, card.span.start, card.span.end),
        "import { Card } from '@site/components';"
    );
    assert_eq!(info.imports[0].span.start_line, 4);
    assert_eq!(info.exports[0].name, "meta");
    assert!(info.parse_diagnostics.is_empty(), "{:?}", info.parse_diagnostics);
    assert_eq!(accessed(&info, "ui"), (vec!["Button".to_owned()], false));
}

#[test]
fn mdx_inline_code_is_not_a_use() {
    let mdx = "import * as ui from './ui';\n\nWrite `<ui.Card/>` or ``{ui.format(x)}`` to get <ui.Button/>.\n";
    let info = parse("docs/usage.mdx", mdx);
    assert_eq!(accessed(&info, "ui"), (vec!["Button".to_owned()], false));
}

#[test]
fn mdx_jsx_text_that_looks_like_a_comment_does_not_hide_the_prose() {
    let mdx = "import * as ui from './ui';\nexport const Glob = () => <code>dist/*</code>\n\n# Build\n\n<ui.Button/>\n";
    let info = parse("docs/build.mdx", mdx);
    assert!(info.parse_diagnostics.is_empty(), "{:?}", info.parse_diagnostics);
    assert_eq!(accessed(&info, "ui"), (vec!["Button".to_owned()], false));
}

#[test]
fn mdx_a_url_in_jsx_text_does_not_hide_the_prose() {
    let mdx = "import * as ui from './ui';\nexport const L = () => (<a>http://x</a>)\n\n# Links\n\n<ui.Button/>\n";
    let info = parse("docs/links.mdx", mdx);
    assert!(info.parse_diagnostics.is_empty(), "{:?}", info.parse_diagnostics);
    assert_eq!(accessed(&info, "ui"), (vec!["Button".to_owned()], false));
}

#[test]
fn mdx_a_template_literal_in_an_expression_is_not_inline_code() {
    let mdx = "import * as ui from './ui';\n\n{ui.label(`x ${ui.count}`)}\n";
    let info = parse("docs/count.mdx", mdx);
    let (mut members, escapes) = accessed(&info, "ui");
    members.sort();
    assert_eq!(
        (members, escapes),
        (vec!["count".to_owned(), "label".to_owned()], false)
    );
}

#[test]
fn angular_component_resources_are_side_effect_imports() {
    let source = "import { Component as Ng } from '@angular/core';\nimport { Other } from './other';\n@Ng({ templateUrl: './a.html', styleUrl: 'a.scss', styleUrls: ['./b.css', `c.less`, ...more] })\nexport class A {}\n@Other({ styleUrls: ['./ignored.css'] })\nexport class B {}\n";
    let info = parse("a.component.ts", source);
    let resources: Vec<_> = specifiers(&info)
        .into_iter()
        .filter(|(_, kind)| *kind == ImportKind::SideEffect)
        .map(|(specifier, _)| specifier)
        .collect();
    assert_eq!(resources, ["./a.html", "./a.scss", "./b.css", "./c.less"]);
    let style = info.imports.iter().find(|i| i.specifier == "./a.scss").unwrap();
    assert_eq!(slice(source, style.span.start, style.span.end), "'a.scss'");
}

#[test]
fn stencil_mode_style_urls_are_side_effect_imports() {
    let source = "import { Component, h } from '@stencil/core';\n@Component({ tag: 'my-a', styleUrls: { ios: 'a.ios.css', md: ['./a.md.css', 'b.md.css'], [mode]: 'x.css' }, assetsDirs: ['assets'] })\nexport class A {}\n";
    let info = parse("my-a.tsx", source);
    let resources: Vec<_> = specifiers(&info)
        .into_iter()
        .filter(|(_, kind)| *kind == ImportKind::SideEffect)
        .map(|(specifier, _)| specifier)
        .collect();
    assert_eq!(resources, ["./a.ios.css", "./a.md.css", "./b.md.css", "./x.css"]);
}

#[test]
fn tsrx_yields_its_imports_only() {
    let source = "import * as ui from './ui';\nimport { useState } from '@tsrx/react';\n\nexport component App() {\n  <ui.Button />\n}\n";
    let info = parse("App.tsrx", source);
    assert_eq!(
        specifiers(&info),
        [("./ui", ImportKind::Static), ("@tsrx/react", ImportKind::Static)]
    );
    assert_eq!(info.imports[1].span.start_line, 2);
    assert_eq!(info.exports.len(), 0);
    // The template is not read, so a namespace counts as used as a whole.
    assert_eq!(accessed(&info, "ui"), (Vec::new(), true));
}

#[test]
fn stylus_style_blocks_are_imports() {
    let vue = "<template/>\n<style lang=\"stylus\">\n@require 'nib'\n@import './theme'\n</style>\n";
    let found: Vec<_> = parse("A.vue", vue).imports.into_iter().map(|i| i.specifier).collect();
    assert_eq!(found, ["nib", "./theme"]);
}

#[test]
fn public_types_are_send_and_sync() {
    fn check<T: Send + Sync>() {}
    check::<ModuleInfo>();
    check::<SyntaxError>();
    check::<ParseOptions>();
    check::<skif_js_syntax::ConfigValue>();
}

#[test]
fn javascript_files_accept_jsx_and_top_level_return() {
    for path in ["a.js", "a.mjs", "a.cjs", "a.jsx"] {
        let info = parse(path, "import x from 'x';\nexport const a = <x.Y prop={1} />;\n");
        assert!(info.has_jsx, "{path}");
        assert_eq!(info.imports[0].specifier, "x", "{path}");
    }
    let info = parse(
        "a.cjs",
        "const a = require('a');\nif (!a) return;\nmodule.exports = a;\n",
    );
    assert!(info.has_cjs_exports);
    assert_eq!(info.imports[0].specifier, "a");
}

#[test]
fn typescript_keeps_angle_bracket_assertions() {
    let info = parse("a.ts", "export const a = <number>(<unknown>1);\n");
    assert!(!info.has_jsx);
    assert_eq!(info.exports.len(), 1);
}

fn specifiers(info: &ModuleInfo) -> Vec<(&str, ImportKind)> {
    info.imports.iter().map(|i| (i.specifier.as_str(), i.kind)).collect()
}

#[test]
fn import_types_are_type_only_imports() {
    let info = parse("a.ts", "export type A = import('x').T;\nlet v: typeof import('y');\n");
    assert_eq!(
        specifiers(&info),
        [("x", ImportKind::TypeOnly), ("y", ImportKind::TypeOnly)]
    );
    assert_eq!(info.imports[0].names[0].kind, ImportedKind::Named("T".into()));
    assert!(info.imports[0].names[0].is_type);
    assert_eq!(info.imports[1].names[0].kind, ImportedKind::Namespace);
    assert!(
        !info.imports[1].names[0].escapes,
        "a bare `typeof import` reads no export"
    );
}

#[test]
fn qualified_import_types_name_one_export_and_the_member_read() {
    let info = parse(
        "a.ts",
        "type A = typeof import('./m').A;
type B = typeof import('./m').E.B;
type C = import('./m').N.C.D;
",
    );
    let names: Vec<_> = info
        .imports
        .iter()
        .map(|import| {
            let name = &import.names[0];
            (name.kind.clone(), name.accessed_members.clone(), name.escapes)
        })
        .collect();
    assert_eq!(
        names,
        [
            (ImportedKind::Named("A".into()), vec![], true),
            (ImportedKind::Named("E".into()), vec!["B".to_owned()], false),
            (ImportedKind::Named("N".into()), vec!["C".to_owned()], false),
        ]
    );
}

#[test]
fn jsdoc_imports_are_type_only_imports() {
    let source = "/** @import {A, B as C} from 'pkg-a' */\n/** @type {import('pkg-b').Thing} */\nconst x = 1;\n/**\n * @import {\n *   D,\n * } from \"pkg-c\"\n */\n";
    let info = parse("a.js", source);
    assert_eq!(
        specifiers(&info),
        [
            ("pkg-a", ImportKind::TypeOnly),
            ("pkg-b", ImportKind::TypeOnly),
            ("pkg-c", ImportKind::TypeOnly)
        ]
    );
    let names: Vec<_> = info.imports[0].names.iter().map(|n| n.local.as_deref()).collect();
    assert_eq!(names, [Some("A"), Some("C")]);
    assert_eq!(info.imports[1].names[0].kind, ImportedKind::Named("Thing".into()));
}

#[test]
fn bare_jsdoc_import_types_read_no_export() {
    let info = parse("a.js", "/** @type {import('./m')} */\nconst m = load();\n");
    let name = &info.imports[0].names[0];
    assert_eq!(name.kind, ImportedKind::Namespace);
    assert!(
        !name.escapes,
        "a bare `import('./m')` type reads no export, as in TypeScript"
    );
}

#[test]
fn an_indexed_typeof_import_reads_the_export_it_names() {
    // unplugin-auto-import writes `const useCounter: typeof import('./counter')['useCounter']`.
    let info = parse(
        "auto-imports.d.ts",
        "declare global {\n  const useCounter: typeof import('./counter')['useCounter']\n  const all: typeof import('./all')\n}\nexport {}\n",
    );
    let named = &info.imports[0].names[0];
    assert_eq!(named.kind, ImportedKind::Named("useCounter".to_owned()));
    assert!(named.escapes, "the indexed export is used as a whole");
    let bare = &info.imports[1].names[0];
    assert_eq!(bare.kind, ImportedKind::Namespace);
    assert!(!bare.escapes);
}

#[test]
fn triple_slash_references_are_imports_only_in_the_header() {
    let source = "/// <reference types=\"node\" />\n/// <reference path=\"extra.d.ts\" />\nexport const a = 1;\n/// <reference types=\"late\" />\n";
    let info = parse("a.ts", source);
    assert_eq!(
        specifiers(&info),
        [("node", ImportKind::TypeOnly), ("./extra.d.ts", ImportKind::SideEffect)]
    );
}

#[test]
fn jsx_import_source_only_counts_in_leading_comments() {
    let leading = parse("a.jsx", "// @jsxImportSource preact\nexport const a = <b />;\n");
    assert_eq!(leading.jsx_import_source.as_deref(), Some("preact"));
    let span = leading.jsx_import_source_span.unwrap();
    assert_eq!(slice("// @jsxImportSource preact\n", span.start, span.end), "preact");

    let late = parse(
        "a.jsx",
        "export const a = <b />;\n// @jsxImportSource preact\nconst s = '@jsxImportSource x';\n",
    );
    assert_eq!(late.jsx_import_source, None);
    assert_eq!(late.jsx_import_source_span, None);
}

#[test]
fn export_keyword_is_anchored_at_the_statement_start() {
    let source = "@Component({ selector: 'export-me' })\nexport class A {}\n";
    let info = parse("a.ts", source);
    let removal = info.exports[0].removal.unwrap();
    assert_eq!(slice(source, removal.start, removal.end), "export ");
    assert_eq!(removal.start as usize, source.find("export class").unwrap());

    let source = "@dec(')', \"export\") export default class B {}\n";
    let removal = parse("a.ts", source).exports[0].removal.unwrap();
    assert_eq!(slice(source, removal.start, removal.end), "export default ");
}

#[test]
fn default_expression_removal_is_marked() {
    let expr = parse("a.js", "export default 42;\n").exports[0].removal.unwrap();
    assert!(expr.default_expression);
    let decl = parse("a.js", "export default function f() {}\n").exports[0]
        .removal
        .unwrap();
    assert!(!decl.default_expression);
}

#[test]
fn removal_edits_are_disjoint() {
    let source = "const a = 1, b = 2, c = 3;\nexport { a, b, c };\nexport const x = 1, y = 2;\n";
    let info = parse("a.ts", source);
    let apply = |unused: &[&str]| {
        let mut out = source.to_string();
        for (s, e) in info
            .removal_edits(|e| unused.contains(&e.name.as_str()))
            .into_iter()
            .rev()
        {
            out.replace_range(s as usize..e as usize, "");
        }
        out
    };
    assert_eq!(apply(&["b", "c"]), source.replace("a, b, c", "a"));
    assert_eq!(apply(&["a", "b"]), source.replace("a, b, c", "c"));
    assert_eq!(apply(&["a", "b", "c"]), source.replace("export { a, b, c };\n", ""));
    assert_eq!(apply(&["x"]), source);
    assert_eq!(apply(&["x", "y"]), source.replace("export const x", "const x"));
    for pair in info.removal_edits(|_| true).windows(2) {
        assert!(pair[0].1 < pair[1].0);
    }
}

#[test]
fn coalesce_merges_overlapping_and_touching_ranges() {
    assert_eq!(
        skif_js_syntax::coalesce_ranges([(5, 9), (0, 3), (3, 4), (8, 12), (20, 20)]),
        [(0, 4), (5, 12)]
    );
}

#[test]
fn directive_helpers() {
    let client = parse("a.tsx", "'use client';\nexport const a = 1;");
    assert!(client.is_client_boundary() && !client.is_server_actions());
    let server = parse("a.ts", "\"use server\";\nexport async function a() {}");
    assert!(server.is_server_actions() && !server.is_client_boundary());
    assert!(!parse("a.ts", "const s = 'use client';").is_client_boundary());
}

fn accessed(info: &ModuleInfo, local: &str) -> (Vec<String>, bool) {
    let name = info
        .imports
        .iter()
        .flat_map(|import| &import.names)
        .find(|name| name.local.as_deref() == Some(local))
        .expect("the import exists");
    (name.accessed_members.clone(), name.escapes)
}

#[test]
fn component_templates_credit_the_imports_they_use() {
    let vue = "<script setup lang=\"ts\">\nimport * as util from './util';\nimport * as whole from './whole';\nimport MyCard from './MyCard.vue';\nimport * as unused from './unused';\n</script>\n<template><my-card :n=\"util.double(2)\" :all=\"whole\">{{ util.triple }}</my-card></template>\n";
    let info = parse("App.vue", vue);
    assert_eq!(
        accessed(&info, "util"),
        (vec!["double".to_owned(), "triple".to_owned()], false)
    );
    assert_eq!(accessed(&info, "whole"), (Vec::<String>::new(), true));
    assert_eq!(accessed(&info, "MyCard"), (Vec::<String>::new(), true));
    assert_eq!(accessed(&info, "unused"), (Vec::<String>::new(), false));

    let svelte = "<script>\n  import * as util from './util';\n  import Widget from './Widget.svelte';\n</script>\n{#if util.ready}<Widget on:click={util.go} />{/if}\n";
    let info = parse("A.svelte", svelte);
    assert_eq!(
        accessed(&info, "util"),
        (vec!["go".to_owned(), "ready".to_owned()], false)
    );
    assert!(accessed(&info, "Widget").1);

    let astro = "---\nimport * as util from './util';\nimport Layout from './Layout.astro';\n---\n<Layout title={util.title}><p>util is not an expression here</p></Layout>\n";
    let info = parse("page.astro", astro);
    assert_eq!(accessed(&info, "util"), (vec!["title".to_owned()], false));
    assert!(accessed(&info, "Layout").1);
}

#[test]
fn script_blocks_of_one_component_share_their_imports() {
    let svelte = "<script context=\"module\">\n  import * as util from './util';\n</script>\n<script>\n  export let n = util.double(1);\n</script>\n";
    let info = parse("A.svelte", svelte);
    assert_eq!(accessed(&info, "util"), (vec!["double".to_owned()], false));

    let vue = "<script>\nimport * as util from './util';\nexport default {};\n</script>\n<script setup>\nconst n = util;\n</script>\n";
    let info = parse("B.vue", vue);
    assert!(accessed(&info, "util").1, "a use as a whole in the other block escapes");
}

#[test]
fn component_style_blocks_and_src_attributes_are_imports_located_in_the_file() {
    let vue = "<template/>\n<style lang=\"scss\">\n@use 'lib/theme';\n</style>\n<style>\n@import './plain.css';\n</style>\n<style src=\"./ext.css\"></style>\n<script src=\"./logic.ts\"></script>\n";
    let info = parse("A.vue", vue);
    let found: Vec<(&str, &str)> = info
        .imports
        .iter()
        .map(|import| {
            (
                import.specifier.as_str(),
                slice(vue, import.span.start, import.span.end),
            )
        })
        .collect();
    assert_eq!(found[0], ("lib/theme", "@use 'lib/theme'"));
    assert_eq!(found[1], ("./plain.css", "@import './plain.css'"));
    assert_eq!(found[2].0, "./ext.css");
    assert_eq!(found[3].0, "./logic.ts");
    assert!(found[2].1.starts_with("<style src"));
    assert!(info.imports.iter().all(|import| import.kind == ImportKind::SideEffect));
}

#[test]
fn data_script_blocks_are_not_parsed() {
    let astro = "---\nimport a from './a';\n---\n<script type=\"application/ld+json\">{\"@type\": \"Thing\"}</script>\n<script>import './client';</script>\n";
    let info = parse("p.astro", astro);
    assert!(
        !info.fatal && info.parse_diagnostics.is_empty(),
        "{:?}",
        info.parse_diagnostics
    );
    let specifiers: Vec<&str> = info.imports.iter().map(|i| i.specifier.as_str()).collect();
    assert_eq!(specifiers, ["./a", "./client"]);
}

#[test]
fn commonjs_named_exports_are_exports() {
    let names = |source: &str| -> Vec<String> {
        parse("a.js", source)
            .exports
            .iter()
            .map(|export| export.name.clone())
            .collect()
    };
    assert_eq!(names("exports.a = 1;\nexports.b = 2;\nexports.a = 3;"), ["a", "b"]);
    assert_eq!(names("module.exports.c = function () {};"), ["c"]);
    assert_eq!(
        names("function d() {}\nfunction e() {}\nmodule.exports = { d, e };"),
        ["d", "e"]
    );
    assert_eq!(names("module.exports = { d: 1, e };"), Vec::<String>::new());
    assert_eq!(names("module.exports = function () {};"), Vec::<String>::new());
    assert_eq!(names("const exports2 = {}; exports2.a = 1;"), Vec::<String>::new());
    assert_eq!(names("exports.x += 1;"), Vec::<String>::new());
}

#[test]
fn custom_element_registrations_are_recorded_on_the_export() {
    let decorator = |module: &str, name: &str| Registration::Decorator {
        module: module.into(),
        name: name.into(),
    };
    let base = |module: &str, name: &str| Registration::BaseClassDefine {
        module: module.into(),
        name: name.into(),
    };
    let cases: Vec<(&str, &str, &str, Vec<Registration>)> = vec![
        (
            "customElements.define",
            "export class A extends HTMLElement {}\ncustomElements.define('x-a', A);",
            "A",
            vec![Registration::Define],
        ),
        (
            "window.customElements.define",
            "export class A extends HTMLElement {}\nwindow.customElements.define('x-a', A);",
            "A",
            vec![Registration::Define],
        ),
        (
            "registry variable",
            "const r = new CustomElementRegistry();\nexport class A extends HTMLElement {}\nr.define('x-a', A);",
            "A",
            vec![Registration::Define],
        ),
        (
            "this in a static block",
            "export class A extends HTMLElement { static { customElements.define('x-a', this); } }",
            "A",
            vec![Registration::Define],
        ),
        (
            "aliased export",
            "class A extends HTMLElement {}\ncustomElements.define('x-a', A);\nexport { A as B };",
            "B",
            vec![Registration::Define],
        ),
        (
            "decorator call",
            "import { customElement } from 'lit/decorators.js';\n@customElement('x-a') export class A {}",
            "A",
            vec![decorator("lit/decorators.js", "customElement")],
        ),
        (
            "renamed bare decorator",
            "import { controller as c } from '@github/catalyst';\n@c export class A {}",
            "A",
            vec![decorator("@github/catalyst", "controller")],
        ),
        (
            "namespace decorator",
            "import * as s from '@stencil/core';\n@s.Component({ tag: 'x-a' }) export default class A {}",
            "default",
            vec![decorator("@stencil/core", "Component")],
        ),
        (
            "anonymous default class",
            "import { customElement } from 'lit/decorators.js';\n@customElement('x-a') export default class {}",
            "default",
            vec![decorator("lit/decorators.js", "customElement")],
        ),
        (
            "base class define",
            "import { FASTElement } from '@microsoft/fast-element';\nexport class A extends FASTElement {}\nA.define({ name: 'x-a' });",
            "A",
            vec![base("@microsoft/fast-element", "FASTElement")],
        ),
        (
            "mixin base class defineAsync",
            "import { FASTElement as F } from '@microsoft/fast-element';\nexport class A extends Mixin(F) {}\nA.defineAsync({ name: 'x-a' });",
            "A",
            vec![base("@microsoft/fast-element", "FASTElement")],
        ),
        (
            "static define on the imported base class",
            "import { FASTElement } from '@microsoft/fast-element';\nexport class A extends FASTElement {}\nFASTElement.define(A, { name: 'x-a' });",
            "A",
            vec![base("@microsoft/fast-element", "FASTElement")],
        ),
        (
            "static define on an object that is not imported",
            "export class A extends HTMLElement {}\nOther.define(A, { name: 'x-a' });",
            "A",
            vec![],
        ),
        (
            "member registry variable in a nested scope",
            "const r = window.customElements;\nexport class A extends HTMLElement {}\nfunction f() { r.define('x-a', A); }",
            "A",
            vec![Registration::Define],
        ),
        (
            "registry variable shadowed by a parameter",
            "const r = customElements;\nexport class A extends HTMLElement {}\nfunction f(r) { r.define('x-a', A); }",
            "A",
            vec![],
        ),
        (
            "registry variable of another scope",
            "function f() { const r = customElements; }\nexport class A extends HTMLElement {}\nr.define('x-a', A);",
            "A",
            vec![],
        ),
        (
            "base class never defined",
            "import { FASTElement } from '@microsoft/fast-element';\nexport class A extends FASTElement {}",
            "A",
            vec![],
        ),
        (
            "local decorator",
            "function customElement(t) { return (c) => c; }\n@customElement('x-a') export class A {}",
            "A",
            vec![],
        ),
        (
            "define of another object",
            "export class A {}\nregistry.define('x-a', A);",
            "A",
            vec![],
        ),
    ];
    for (name, source, export, expected) in cases {
        let info = parse("a.ts", source);
        let found = info
            .exports
            .iter()
            .find(|e| e.name == export)
            .unwrap_or_else(|| panic!("{name}: no export {export}"));
        assert_eq!(found.registrations, expected, "{name}");
    }
}
