//! Table-driven tests for `parse_module`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use skif_js_syntax::{
    ExportKind, ImportKind, ImportedKind, ModuleInfo, ParseOptions, RemovalKind, SyntaxError, parse_module,
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
        "import(name); require(name); import(`./x/${n}.js`); import.meta.glob(pattern);",
    );
    assert_eq!(info.imports, [] as [skif_js_syntax::Import; 0]);
    assert_eq!(info.unresolved_dynamic.len(), 4);
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
fn namespace_that_escapes_is_flagged() {
    for source in [
        "import * as ns from 'x'; use(ns);",
        "import * as ns from 'x'; ns[key];",
        "import * as ns from 'x'; const { a, ...rest } = ns;",
        "import * as ns from 'x'; export { ns };",
    ] {
        let info = parse("a.ts", source);
        assert!(info.imports[0].names[0].escapes, "{source}");
    }
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
    let ts_only = parse_module(
        Path::new("a.js"),
        "export const a: number = 1;",
        &ParseOptions::default(),
    );
    assert!(matches!(ts_only, Err(SyntaxError::Parse { .. })));
}

#[test]
fn syntax_errors_carry_a_location() {
    let source = "const ok = 1;\nconst = ;\n";
    let err = parse_module(Path::new("a.js"), source, &ParseOptions::default()).unwrap_err();
    let SyntaxError::Parse { message, span } = err else {
        panic!("expected a parse error");
    };
    assert_ne!(message, "");
    assert_eq!(span.start_line, 2);
}

#[test]
fn unsupported_extensions() {
    for path in ["a.json", "a.mdx", "a", "a.css"] {
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
    let info = parse("a.cjs", "const a = require('a');\nif (!a) return;\nmodule.exports = a;\n");
    assert!(info.has_cjs_exports);
    assert_eq!(info.imports[0].specifier, "a");
}

#[test]
fn typescript_keeps_angle_bracket_assertions() {
    let info = parse("a.ts", "export const a = <number>(<unknown>1);\n");
    assert!(!info.has_jsx);
    assert_eq!(info.exports.len(), 1);
}
