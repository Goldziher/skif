//! `package.json` formatting (ADR 0009) on the fixtures under `tests/fixtures/package-json`: a before-and-after
//! snapshot per case, and formatting twice equals formatting once.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;
use std::path::PathBuf;

use proptest::prelude::*;
use serde_json::{Map, Value};
use skif_edit::{FormatOptions, format_package_json};

fn options(case: &str) -> FormatOptions {
    match case {
        "shorthands" => FormatOptions::default()
            .with_format_bugs(true)
            .with_format_repository(true),
        "indent-tab" => FormatOptions::default().with_indent(Some("\t".to_owned())),
        "crlf" | "mixed-newlines-indent" => FormatOptions::default().with_indent(Some("  ".to_owned())),
        _ => FormatOptions::default(),
    }
}

fn fixture(case: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/package-json")
        .join(case)
        .join("package.json");
    std::fs::read_to_string(path).unwrap()
}

/// Line endings are made visible so a CRLF case shows them in the snapshot.
fn visible(text: &str) -> String {
    text.replace("\r\n", "\\r\\n\n").replace('\t', "\\t")
}

fn report(case: &str) -> String {
    let source = fixture(case);
    let options = options(case);
    let formatted = format_package_json(&source, &options).unwrap();
    let again = format_package_json(&formatted.output, &options).unwrap();
    assert_eq!(again.issues, [], "{case}: formatting the output finds issues");
    assert_eq!(
        again.output, formatted.output,
        "{case}: formatting twice differs from once"
    );
    if formatted.issues.is_empty() {
        assert_eq!(formatted.output, source, "{case}: no issue but a changed file");
    }
    let mut out = String::new();
    let _ = writeln!(out, "issues:");
    for issue in &formatted.issues {
        let _ = writeln!(out, "  - {}", issue.describe());
    }
    let _ = writeln!(
        out,
        "--- before\n{}--- after\n{}",
        visible(&source),
        visible(&formatted.output)
    );
    out
}

macro_rules! case {
    ($test:ident, $case:literal) => {
        #[test]
        fn $test() {
            insta::assert_snapshot!($case, report($case));
        }
    };
}

case!(syncpack_defaults, "syncpack-defaults");
case!(shorthands, "shorthands");
case!(exports, "exports");
case!(indent_tab, "indent-tab");
case!(crlf, "crlf");
case!(formatted, "formatted");
case!(comments, "comments");
case!(mixed_newlines, "mixed-newlines");
case!(mixed_newlines_indent, "mixed-newlines-indent");

fn value() -> impl Strategy<Value = Value> {
    let scalar = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i32>().prop_map(Value::from),
        "[a-zB-D@ é]{0,3}".prop_map(Value::String),
    ];
    scalar.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::vec(("[a-cA-C@./]{1,3}|default|types|import|scripts|keywords", inner), 0..5)
                .prop_map(|entries| Value::Object(entries.into_iter().collect::<Map<_, _>>())),
        ]
    })
}

proptest! {
    #[test]
    fn formatting_is_idempotent(
        entries in prop::collection::vec(("[a-c]{1,2}|name|version|exports|scripts|keywords|bugs|repository", value()), 0..6),
        pretty in any::<bool>(),
        indent in prop::option::of(prop_oneof![Just("  ".to_owned()), Just("\t".to_owned())]),
    ) {
        let root = Value::Object(entries.into_iter().collect::<Map<_, _>>());
        let source = if pretty { serde_json::to_string_pretty(&root).unwrap() } else { root.to_string() };
        let options = FormatOptions::default()
            .with_format_bugs(true)
            .with_format_repository(true)
            .with_indent(indent);
        let once = format_package_json(&source, &options).unwrap();
        let twice = format_package_json(&once.output, &options).unwrap();
        prop_assert_eq!(twice.issues, vec![]);
        prop_assert_eq!(&twice.output, &once.output);
        let reparsed: Value = serde_json::from_str(&once.output).unwrap();
        prop_assert!(reparsed.is_object());
    }
}
