//! Typed enum and namespace reads (ADR 0046): reads through aliases, `this` and qualified names credit the member they
//! reach, and a declaration that escapes so that any member may be read is used as a whole.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{Expected, TSCONFIG, Workspace, assert_case, package, query, run, typed};
use skif_js_types::DeclKind;

const COLORS: &str = "\
export enum Color {
  Red,
  Green,
  Blue,
}
";

fn analyze_colors(main: &str) -> skif_js_types::TypeFacts {
    let workspace = Workspace::new(&[
        ("tsconfig.json", TSCONFIG),
        ("src/colors.ts", COLORS),
        ("src/main.ts", main),
    ]);
    let query = query(vec![workspace.decl(
        "src/colors.ts",
        DeclKind::Enum,
        "Color",
        &["Red", "Green", "Blue"],
    )]);
    run(
        &workspace,
        &[package("", "app", &["src/colors.ts", "src/main.ts"])],
        &query,
    )
}

#[test]
fn an_enum_member_read_through_an_alias_or_a_literal_key_credits_that_member() {
    for (case, main, member) in [
        (
            "a const alias",
            "import { Color } from './colors';\n\nconst Palette = Color;\nexport const red = Palette.Red;\n",
            "Red",
        ),
        (
            "a literal element access",
            "import { Color } from './colors';\n\nexport const green = Color['Green'];\n",
            "Green",
        ),
        (
            "a destructuring of an alias",
            "import { Color } from './colors';\n\nconst Palette = Color;\nexport const { Blue } = Palette;\n",
            "Blue",
        ),
    ] {
        assert_case(
            case,
            &analyze_colors(main),
            &Expected {
                reads: vec![(0, vec![member])],
                covered: vec!["src/colors.ts", "src/main.ts"],
                coverage: vec![typed("", "app")],
                ..Expected::default()
            },
        );
    }
}

#[test]
fn an_enum_that_escapes_is_used_as_a_whole() {
    for (case, main) in [
        (
            "passed to a function",
            "import { Color } from './colors';\n\ndeclare function register(value: object): void;\nregister(Color);\n",
        ),
        (
            "Object.values",
            "import { Color } from './colors';\n\nexport const all = Object.values(Color);\n",
        ),
        (
            "an alias passed on",
            "import { Color } from './colors';\n\ndeclare function register(value: object): void;\nconst Palette = Color;\nregister(Palette);\n",
        ),
        (
            "keyof typeof",
            "import { Color } from './colors';\n\nexport type ColorName = keyof typeof Color;\n",
        ),
        (
            "a spread",
            "import { Color } from './colors';\n\nexport const copy = { ...Color };\n",
        ),
        (
            "a shorthand property",
            "import { Color } from './colors';\n\nexport const palette = { Color };\n",
        ),
    ] {
        assert_case(
            case,
            &analyze_colors(main),
            &Expected {
                whole: vec![0],
                covered: vec!["src/colors.ts", "src/main.ts"],
                coverage: vec![typed("", "app")],
                ..Expected::default()
            },
        );
    }
}

const SETTINGS: &str = "\
export namespace Settings {
  export const port = 8080;
  export const host = 'localhost';
}
";

#[test]
fn a_namespace_member_read_through_this_or_a_qualified_name_credits_that_member() {
    for (case, main) in [
        (
            "this typed as the namespace",
            "import { Settings } from './settings';\n\nexport function port(this: typeof Settings): number {\n  return this.port;\n}\n",
        ),
        (
            "a qualified name in a type query",
            "import { Settings } from './settings';\n\nexport type Port = typeof Settings.port;\n",
        ),
    ] {
        let workspace = Workspace::new(&[
            ("tsconfig.json", TSCONFIG),
            ("src/settings.ts", SETTINGS),
            ("src/main.ts", main),
        ]);
        let query = query(vec![workspace.decl(
            "src/settings.ts",
            DeclKind::Namespace,
            "Settings",
            &["port", "host"],
        )]);
        let facts = run(
            &workspace,
            &[package("", "app", &["src/settings.ts", "src/main.ts"])],
            &query,
        );
        assert_case(
            case,
            &facts,
            &Expected {
                reads: vec![(0, vec!["port"])],
                covered: vec!["src/settings.ts", "src/main.ts"],
                coverage: vec![typed("", "app")],
                ..Expected::default()
            },
        );
    }
}
