//! Table-driven JSON edit tests asserting exact output strings.

mod common;

use common::{insert, json_op, remove, rename, set, sort_keys};
use jsonc_parser::{CollectOptions, ParseOptions, parse_to_ast};
use serde_json::json;
use skif_core::{FixOp, InsertPosition, JsonAction, TextEdit, apply_text_edits};
use skif_edit::{EditPlanError, FileKind, apply, plan_edits};

struct Case {
    name: &'static str,
    input: &'static str,
    ops: Vec<FixOp>,
    expected: &'static str,
}

fn run(cases: Vec<Case>) {
    for case in cases {
        let ops: Vec<&FixOp> = case.ops.iter().collect();
        let actual = apply(case.input, FileKind::Json, &ops).unwrap_or_else(|error| panic!("{}: {error}", case.name));
        assert_eq!(actual, case.expected, "case `{}`", case.name);
        parse_to_ast(&actual, &CollectOptions::default(), &ParseOptions::default())
            .unwrap_or_else(|error| panic!("{}: output is not valid JSONC: {error}", case.name));
    }
}

#[test]
fn set_replaces_only_the_value_span() {
    run(vec![
        Case {
            name: "scalar, 2 spaces",
            input: "{\n  \"name\": \"a\",\n  \"version\": \"1.0.0\"\n}\n",
            ops: vec![set(&["version"], json!("2.0.0"))],
            expected: "{\n  \"name\": \"a\",\n  \"version\": \"2.0.0\"\n}\n",
        },
        Case {
            name: "odd spacing survives",
            input: "{ \"a\":   1 ,\n\"b\"  :2 }",
            ops: vec![set(&["b"], json!(true))],
            expected: "{ \"a\":   1 ,\n\"b\"  :true }",
        },
        Case {
            name: "object value, 4 spaces",
            input: "{\n    \"name\": \"x\",\n    \"scripts\": {}\n}\n",
            ops: vec![set(
                &["scripts"],
                json!({"a": "b", "c": {"d": 1}, "e": [], "f": [1, "x"]}),
            )],
            expected: "{\n    \"name\": \"x\",\n    \"scripts\": {\n        \"a\": \"b\",\n        \"c\": {\n            \"d\": 1\n        },\n        \"e\": [],\n        \"f\": [\n            1,\n            \"x\"\n        ]\n    }\n}\n",
        },
        Case {
            name: "object value, tabs",
            input: "{\n\t\"a\": {\n\t\t\"b\": 1\n\t}\n}\n",
            ops: vec![set(&["a", "b"], json!({"c": 2}))],
            expected: "{\n\t\"a\": {\n\t\t\"b\": {\n\t\t\t\"c\": 2\n\t\t}\n\t}\n}\n",
        },
        Case {
            name: "CRLF object value",
            input: "{\r\n  \"a\": 1\r\n}\r\n",
            ops: vec![set(&["a"], json!({"b": 2}))],
            expected: "{\r\n  \"a\": {\r\n    \"b\": 2\r\n  }\r\n}\r\n",
        },
        Case {
            name: "no final newline",
            input: "{\n  \"a\": 1\n}",
            ops: vec![set(&["a"], json!(2))],
            expected: "{\n  \"a\": 2\n}",
        },
        Case {
            name: "unicode values and keys",
            input: "{\n  \"ключ\": \"значение\",\n  \"emoji\": \"🎉\"\n}\n",
            ops: vec![set(&["emoji"], json!("🚀")), set(&["ключ"], json!("новое"))],
            expected: "{\n  \"ключ\": \"новое\",\n  \"emoji\": \"🚀\"\n}\n",
        },
        Case {
            name: "nested pointer",
            input: "{\n  \"a\": {\n    \"b\": {\n      \"c\": 1\n    }\n  }\n}\n",
            ops: vec![set(&["a", "b", "c"], json!(null))],
            expected: "{\n  \"a\": {\n    \"b\": {\n      \"c\": null\n    }\n  }\n}\n",
        },
        Case {
            name: "array index path",
            input: "{\n  \"list\": [\n    {\n      \"name\": \"x\"\n    },\n    {\n      \"name\": \"y\"\n    }\n  ]\n}\n",
            ops: vec![set(&["list", "#1", "name"], json!("z"))],
            expected: "{\n  \"list\": [\n    {\n      \"name\": \"x\"\n    },\n    {\n      \"name\": \"z\"\n    }\n  ]\n}\n",
        },
        Case {
            name: "root replaced",
            input: "{\"a\":1}\n",
            ops: vec![set(&[], json!([1]))],
            expected: "[\n  1\n]\n",
        },
        Case {
            name: "comments survive",
            input: "// header\n{\n  /* keep */ \"a\": 1, // trailing\n  \"b\": 2\n}\n",
            ops: vec![set(&["b"], json!(3))],
            expected: "// header\n{\n  /* keep */ \"a\": 1, // trailing\n  \"b\": 3\n}\n",
        },
    ]);
}

#[test]
fn remove_cleans_up_separators() {
    const ABC: &str = "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}\n";
    run(vec![
        Case {
            name: "first",
            input: ABC,
            ops: vec![remove(&["a"])],
            expected: "{\n  \"b\": 2,\n  \"c\": 3\n}\n",
        },
        Case {
            name: "middle",
            input: ABC,
            ops: vec![remove(&["b"])],
            expected: "{\n  \"a\": 1,\n  \"c\": 3\n}\n",
        },
        Case {
            name: "last",
            input: ABC,
            ops: vec![remove(&["c"])],
            expected: "{\n  \"a\": 1,\n  \"b\": 2\n}\n",
        },
        Case {
            name: "only member becomes {}",
            input: "{\n  \"deps\": {\n    \"x\": \"1\"\n  },\n  \"n\": 1\n}\n",
            ops: vec![remove(&["deps", "x"])],
            expected: "{\n  \"deps\": {},\n  \"n\": 1\n}\n",
        },
        Case {
            name: "first, CRLF",
            input: "{\r\n  \"a\": 1,\r\n  \"b\": 2\r\n}\r\n",
            ops: vec![remove(&["a"])],
            expected: "{\r\n  \"b\": 2\r\n}\r\n",
        },
        Case {
            name: "last, tabs",
            input: "{\n\t\"a\": 1,\n\t\"b\": 2\n}",
            ops: vec![remove(&["b"])],
            expected: "{\n\t\"a\": 1\n}",
        },
        Case {
            name: "inline first",
            input: "{ \"a\": 1, \"b\": 2 }",
            ops: vec![remove(&["a"])],
            expected: "{ \"b\": 2 }",
        },
        Case {
            name: "inline last",
            input: "{ \"a\": 1, \"b\": 2 }",
            ops: vec![remove(&["b"])],
            expected: "{ \"a\": 1 }",
        },
        Case {
            name: "comment on the next member stays",
            input: "{\n  \"a\": 1,\n  // about b\n  \"b\": 2\n}\n",
            ops: vec![remove(&["a"])],
            expected: "{\n  // about b\n  \"b\": 2\n}\n",
        },
        Case {
            name: "line comment after the removed middle member",
            input: "{\n  \"a\": 1, // one\n  \"b\": 2,\n  \"c\": 3\n}\n",
            ops: vec![remove(&["b"])],
            expected: "{\n  \"a\": 1, // one\n  \"c\": 3\n}\n",
        },
        Case {
            name: "array first, middle and last",
            input: "{\n  \"l\": [\n    1,\n    2,\n    3,\n    4\n  ]\n}\n",
            ops: vec![remove(&["l", "#0"]), remove(&["l", "#2"]), remove(&["l", "#3"])],
            expected: "{\n  \"l\": [\n    2\n  ]\n}\n",
        },
        Case {
            name: "array removals in descending op order",
            input: "{\n  \"l\": [\n    1,\n    2,\n    3,\n    4,\n    5\n  ]\n}\n",
            ops: vec![remove(&["l", "#3"]), remove(&["l", "#1"])],
            expected: "{\n  \"l\": [\n    1,\n    3,\n    5\n  ]\n}\n",
        },
        Case {
            name: "three array removals address the original indices",
            input: "{ \"l\": [\"a\", \"b\", \"c\", \"d\", \"e\"] }",
            ops: vec![remove(&["l", "#0"]), remove(&["l", "#1"]), remove(&["l", "#3"])],
            expected: "{ \"l\": [\"c\", \"e\"] }",
        },
        Case {
            name: "adjacent middle members",
            input: "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3,\n  \"d\": 4\n}\n",
            ops: vec![remove(&["b"]), remove(&["c"])],
            expected: "{\n  \"a\": 1,\n  \"d\": 4\n}\n",
        },
        Case {
            name: "unicode member",
            input: "{\n  \"ключ\": \"🎉\",\n  \"b\": 2\n}\n",
            ops: vec![remove(&["ключ"])],
            expected: "{\n  \"b\": 2\n}\n",
        },
    ]);
}

#[test]
fn insert_places_keys() {
    const DEPS: &str = "{\n  \"dependencies\": {\n    \"a\": \"1\",\n    \"c\": \"3\"\n  }\n}\n";
    let dep = |key: &str, position| insert(&["dependencies"], key, json!("2"), position);
    run(vec![
        Case {
            name: "sorted between",
            input: DEPS,
            ops: vec![dep("b", InsertPosition::Sorted)],
            expected: "{\n  \"dependencies\": {\n    \"a\": \"1\",\n    \"b\": \"2\",\n    \"c\": \"3\"\n  }\n}\n",
        },
        Case {
            name: "sorted at the end",
            input: DEPS,
            ops: vec![dep("d", InsertPosition::Sorted)],
            expected: "{\n  \"dependencies\": {\n    \"a\": \"1\",\n    \"c\": \"3\",\n    \"d\": \"2\"\n  }\n}\n",
        },
        Case {
            name: "sorted at the start",
            input: DEPS,
            ops: vec![dep("0", InsertPosition::Sorted)],
            expected: "{\n  \"dependencies\": {\n    \"0\": \"2\",\n    \"a\": \"1\",\n    \"c\": \"3\"\n  }\n}\n",
        },
        Case {
            name: "start",
            input: DEPS,
            ops: vec![dep("z", InsertPosition::Start)],
            expected: "{\n  \"dependencies\": {\n    \"z\": \"2\",\n    \"a\": \"1\",\n    \"c\": \"3\"\n  }\n}\n",
        },
        Case {
            name: "end, tabs",
            input: "{\n\t\"d\": {\n\t\t\"a\": 1\n\t}\n}\n",
            ops: vec![insert(&["d"], "b", json!({"x": [1]}), InsertPosition::End)],
            expected: "{\n\t\"d\": {\n\t\t\"a\": 1,\n\t\t\"b\": {\n\t\t\t\"x\": [\n\t\t\t\t1\n\t\t\t]\n\t\t}\n\t}\n}\n",
        },
        Case {
            name: "end, CRLF",
            input: "{\r\n  \"a\": 1\r\n}\r\n",
            ops: vec![insert(&[], "b", json!(2), InsertPosition::End)],
            expected: "{\r\n  \"a\": 1,\r\n  \"b\": 2\r\n}\r\n",
        },
        Case {
            name: "into an empty object",
            input: "{\n    \"name\": \"x\",\n    \"dependencies\": {}\n}\n",
            ops: vec![insert(&["dependencies"], "a", json!("^1"), InsertPosition::Sorted)],
            expected: "{\n    \"name\": \"x\",\n    \"dependencies\": {\n        \"a\": \"^1\"\n    }\n}\n",
        },
        Case {
            name: "into an empty root, no final newline",
            input: "{}",
            ops: vec![insert(&[], "a", json!(1), InsertPosition::End)],
            expected: "{\n  \"a\": 1\n}",
        },
        Case {
            name: "inline object",
            input: "{ \"a\": 1, \"c\": 3 }",
            ops: vec![
                insert(&[], "b", json!(2), InsertPosition::Sorted),
                insert(&[], "d", json!(4), InsertPosition::End),
            ],
            expected: "{ \"a\": 1, \"b\": 2, \"c\": 3, \"d\": 4 }",
        },
        Case {
            name: "existing key is replaced in place",
            input: DEPS,
            ops: vec![dep("a", InsertPosition::End)],
            expected: "{\n  \"dependencies\": {\n    \"a\": \"2\",\n    \"c\": \"3\"\n  }\n}\n",
        },
        Case {
            name: "unicode key",
            input: "{\n  \"a\": 1\n}\n",
            ops: vec![insert(&[], "ключ", json!("🎉"), InsertPosition::Sorted)],
            expected: "{\n  \"a\": 1,\n  \"ключ\": \"🎉\"\n}\n",
        },
        Case {
            name: "after a trailing comma",
            input: "{\n  \"a\": 1,\n}\n",
            ops: vec![insert(&[], "b", json!(2), InsertPosition::End)],
            expected: "{\n  \"a\": 1,\n  \"b\": 2,\n}\n",
        },
    ]);
}

#[test]
fn sort_keys_reorders_member_text() {
    run(vec![
        Case {
            name: "root only, nested objects untouched",
            input: "{\n  \"scripts\": {\n    \"z\": 1,\n    \"a\": 2\n  },\n  \"name\": \"x\",\n  \"dependencies\": {\n    \"b\": 1,\n    \"a\": 2\n  }\n}\n",
            ops: vec![sort_keys(&[])],
            expected: "{\n  \"dependencies\": {\n    \"b\": 1,\n    \"a\": 2\n  },\n  \"name\": \"x\",\n  \"scripts\": {\n    \"z\": 1,\n    \"a\": 2\n  }\n}\n",
        },
        Case {
            name: "nested only",
            input: "{\n  \"b\": 1,\n  \"deps\": {\n    \"y\": \"1\",\n    \"x\": \"2\"\n  },\n  \"a\": 2\n}\n",
            ops: vec![sort_keys(&["deps"])],
            expected: "{\n  \"b\": 1,\n  \"deps\": {\n    \"x\": \"2\",\n    \"y\": \"1\"\n  },\n  \"a\": 2\n}\n",
        },
        Case {
            name: "already sorted is a no-op",
            input: "{\n  \"a\": 1,\n  \"b\": 2\n}\n",
            ops: vec![sort_keys(&[])],
            expected: "{\n  \"a\": 1,\n  \"b\": 2\n}\n",
        },
        Case {
            name: "inner formatting and comments between members stay",
            input: "{ \"b\":  [1,2], // two\n  \"a\" : {\"x\":1} }",
            ops: vec![sort_keys(&[])],
            expected: "{ \"a\" : {\"x\":1}, // two\n  \"b\":  [1,2] }",
        },
        Case {
            name: "unicode keys sort by bytes",
            input: "{\n  \"я\": 1,\n  \"b\": 2,\n  \"é\": 3\n}\n",
            ops: vec![sort_keys(&[])],
            expected: "{\n  \"b\": 2,\n  \"é\": 3,\n  \"я\": 1\n}\n",
        },
    ]);
}

#[test]
fn rename_replaces_only_the_key() {
    run(vec![
        Case {
            name: "plain",
            input: "{\n  \"old\"  :  1,\n  \"b\": 2\n}\n",
            ops: vec![rename(&["old"], "new")],
            expected: "{\n  \"new\"  :  1,\n  \"b\": 2\n}\n",
        },
        Case {
            name: "unicode",
            input: "{\n  \"ключ\": {\n    \"x\": 1\n  }\n}\n",
            ops: vec![rename(&["ключ"], "κλειδί \"q\"")],
            expected: "{\n  \"κλειδί \\\"q\\\"\": {\n    \"x\": 1\n  }\n}\n",
        },
        Case {
            name: "same name is a no-op",
            input: "{\"a\": 1}",
            ops: vec![rename(&["a"], "a")],
            expected: "{\"a\": 1}",
        },
    ]);
}

#[test]
fn several_ops_in_one_file() {
    run(vec![Case {
        name: "set, insert, remove and sort",
        input: "{\n  \"version\": \"1.0.0\",\n  \"scripts\": {\n    \"build\": \"x\",\n    \"test\": \"y\"\n  },\n  \"dependencies\": {\n    \"b\": \"1\",\n    \"a\": \"1\"\n  }\n}\n",
        ops: vec![
            sort_keys(&["dependencies"]),
            remove(&["scripts", "build"]),
            set(&["version"], json!("2.0.0")),
            insert(&[], "private", json!(true), InsertPosition::End),
        ],
        expected: "{\n  \"version\": \"2.0.0\",\n  \"scripts\": {\n    \"test\": \"y\"\n  },\n  \"dependencies\": {\n    \"a\": \"1\",\n    \"b\": \"1\"\n  },\n  \"private\": true\n}\n",
    }]);
}

#[test]
fn text_ops_pass_through() {
    let op = FixOp::text(
        "package.json",
        TextEdit::new(
            1,
            1,
            " ".to_owned(),
        ),
    );
    assert_eq!(apply("{}", FileKind::Json, &[&op]).unwrap(), "{ }");
}

#[test]
fn overlapping_ops_conflict_and_name_both() {
    let first = remove(&["a"]);
    let second = set(&["a"], json!(2));
    let source = "{\n  \"a\": 1,\n  \"b\": 2\n}\n";
    let error = plan_edits(source, FileKind::Json, &[&first, &second]).unwrap_err();
    assert_eq!(
        error,
        EditPlanError::Conflict {
            first: "json remove /a".to_owned(),
            second: "json set /a".to_owned()
        }
    );
    assert_eq!(
        error.to_string(),
        "operations conflict: `json remove /a` overlaps `json set /a`"
    );

    // Sorting an object moves its members; what an operation changes inside a member moves with it.
    let sort = sort_keys(&[]);
    let unsorted = "{\n  \"b\": 1,\n  \"a\": 2\n}\n";
    assert_eq!(
        apply(unsorted, FileKind::Json, &[&sort, &set(&["a"], json!(3))]).unwrap(),
        "{\n  \"a\": 3,\n  \"b\": 1\n}\n"
    );
    let remove_a = remove(&["a"]);
    let error = plan_edits(unsorted, FileKind::Json, &[&sort, &remove_a]).unwrap_err();
    assert!(matches!(error, EditPlanError::Conflict { .. }), "{error}");
}

#[test]
fn sorting_carries_the_edits_inside_the_moved_members() {
    run(vec![
        Case {
            name: "set inside a nested object that moves",
            input: "{\n  \"z\": {\n    \"x\": 1\n  },\n  \"a\": 2\n}\n",
            ops: vec![sort_keys(&[]), set(&["z", "x"], json!(5))],
            expected: "{\n  \"a\": 2,\n  \"z\": {\n    \"x\": 5\n  }\n}\n",
        },
        Case {
            name: "remove inside a member that moves",
            input: "{\n  \"z\": {\n    \"x\": 1,\n    \"y\": 2\n  },\n  \"a\": 2\n}\n",
            ops: vec![sort_keys(&[]), remove(&["z", "x"])],
            expected: "{\n  \"a\": 2,\n  \"z\": {\n    \"y\": 2\n  }\n}\n",
        },
        Case {
            name: "rename the key of a member that moves",
            input: "{\n  \"b\": 1,\n  \"a\": 2\n}\n",
            ops: vec![sort_keys(&[]), rename(&["b"], "c")],
            expected: "{\n  \"a\": 2,\n  \"c\": 1\n}\n",
        },
        Case {
            name: "nested sorts travel with the member that holds them",
            input: "{\n  \"z\": {\n    \"y\": 1,\n    \"x\": 2\n  },\n  \"a\": 2\n}\n",
            ops: vec![sort_keys(&[]), sort_keys(&["z"])],
            expected: "{\n  \"a\": 2,\n  \"z\": {\n    \"x\": 2,\n    \"y\": 1\n  }\n}\n",
        },
        Case {
            name: "edits in members that stay put are untouched",
            input: "{\n  \"a\": {\n    \"k\": 1\n  },\n  \"c\": 3,\n  \"b\": 2\n}\n",
            ops: vec![sort_keys(&[]), set(&["a", "k"], json!(9))],
            expected: "{\n  \"a\": {\n    \"k\": 9\n  },\n  \"b\": 2,\n  \"c\": 3\n}\n",
        },
    ]);
}

/// What the facade does with fixes that each remove one member: every fix is planned on its own, identical edits count
/// once and the rest must not overlap.
fn combined_single_plans(source: &str, ops: &[FixOp]) -> Result<String, String> {
    let mut edits: Vec<TextEdit> = Vec::new();
    for op in ops {
        for edit in plan_edits(source, FileKind::Json, &[op]).map_err(|error| error.to_string())? {
            if !edits.contains(&edit) {
                edits.push(edit);
            }
        }
    }
    edits.sort_by_key(|edit| (edit.start, edit.end));
    if let Some(pair) = edits.windows(2).find(|pair| pair[0].end > pair[1].start) {
        return Err(format!("overlap: {pair:?}"));
    }
    apply_text_edits(source, &edits).map_err(|error| error.to_string())
}

#[test]
fn single_member_removals_planned_alone_commute() {
    const ABCD: &str = "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3,\n  \"d\": 4\n}\n";
    let cases: [(&[&str], &str); 5] = [
        (&["a", "b"], "{\n  \"c\": 3,\n  \"d\": 4\n}\n"),
        (&["b", "c"], "{\n  \"a\": 1,\n  \"d\": 4\n}\n"),
        (&["a", "c"], "{\n  \"b\": 2,\n  \"d\": 4\n}\n"),
        (&["a", "d"], "{\n  \"b\": 2,\n  \"c\": 3\n}\n"),
        (&["b", "d"], "{\n  \"a\": 1,\n  \"c\": 3\n}\n"),
    ];
    for (keys, expected) in cases {
        let ops: Vec<FixOp> = keys.iter().map(|key| remove(&[key])).collect();
        assert_eq!(combined_single_plans(ABCD, &ops).as_deref(), Ok(expected), "{keys:?}");
        // The order in which the fixes are planned does not matter.
        let reversed: Vec<FixOp> = ops.into_iter().rev().collect();
        assert_eq!(
            combined_single_plans(ABCD, &reversed).as_deref(),
            Ok(expected),
            "{keys:?}"
        );
    }
    let inline = "{ \"a\": 1, \"b\": 2, \"c\": 3 }";
    let ops = [remove(&["a"]), remove(&["c"])];
    assert_eq!(combined_single_plans(inline, &ops).as_deref(), Ok("{ \"b\": 2 }"));
}

#[test]
fn the_last_two_members_removed_alone_agree_on_their_separator() {
    const ABC: &str = "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}\n";
    let ops = [remove(&["b"]), remove(&["c"])];
    assert!(combined_single_plans(ABC, &ops).is_ok());
}

#[test]
fn removing_the_last_member_keeps_the_comment_after_the_previous_comma() {
    run(vec![
        Case {
            name: "comment after the comma",
            input: "{\n  \"a\": 1, // keep\n  \"b\": 2\n}\n",
            ops: vec![remove(&["b"])],
            expected: "{\n  \"a\": 1 // keep\n}\n",
        },
        Case {
            name: "block comment after the comma, inline",
            input: "{ \"a\": 1, /* keep */ \"b\": 2 }",
            ops: vec![remove(&["b"])],
            expected: "{ \"a\": 1 /* keep */  }",
        },
        Case {
            name: "several trailing members",
            input: "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}\n",
            ops: vec![remove(&["b"]), remove(&["c"])],
            expected: "{\n  \"a\": 1\n}\n",
        },
    ]);
}

#[test]
fn independent_ops_do_not_conflict() {
    let ops = [set(&["a"], json!(2)), set(&["b"], json!(3))];
    let refs: Vec<&FixOp> = ops.iter().collect();
    let edits = plan_edits("{\n  \"a\": 1,\n  \"b\": 2\n}\n", FileKind::Json, &refs).unwrap();
    assert_eq!(
        edits,
        vec![
            TextEdit::new(
                9,
                10,
                "2".to_owned(),
            ),
            TextEdit::new(
                19,
                20,
                "3".to_owned(),
            ),
        ]
    );
}

#[test]
fn planning_errors() {
    let source = "{\n  \"a\": 1\n}\n";
    let plan = |op: FixOp| plan_edits(source, FileKind::Json, &[&op]).unwrap_err();
    assert_eq!(
        plan(set(&["missing"], json!(1))),
        EditPlanError::PathNotFound {
            op: "json set /missing".to_owned()
        }
    );
    assert!(matches!(
        plan(set(&["a", "b"], json!(1))),
        EditPlanError::PathNotFound { .. }
    ));
    assert!(matches!(plan(remove(&[])), EditPlanError::WrongNodeKind { .. }));
    assert!(matches!(
        plan(insert(&["a"], "k", json!(1), InsertPosition::End)),
        EditPlanError::WrongNodeKind { .. }
    ));
    assert!(matches!(plan(sort_keys(&["a"])), EditPlanError::WrongNodeKind { .. }));
    assert!(matches!(plan(rename(&[], "x")), EditPlanError::WrongNodeKind { .. }));
    let two = "{\"a\": 1, \"b\": 2}";
    let error = plan_edits(two, FileKind::Json, &[&rename(&["a"], "b")]).unwrap_err();
    assert_eq!(
        error,
        EditPlanError::DuplicateKey {
            op: "json rename /a".to_owned(),
            key: "b".to_owned()
        }
    );
    assert!(matches!(
        plan_edits("{", FileKind::Json, &[&json_op(&[], JsonAction::SortKeys)]).unwrap_err(),
        EditPlanError::Parse { .. }
    ));
    let create = FixOp::create_file(
        "x",
        String::new(),
    );
    assert!(matches!(
        plan_edits(source, FileKind::Json, &[&create]).unwrap_err(),
        EditPlanError::Unsupported { .. }
    ));
}

#[test]
fn file_kind_from_path() {
    use std::path::Path;
    assert_eq!(FileKind::from_path(Path::new("a/package.json")), Some(FileKind::Json));
    assert_eq!(FileKind::from_path(Path::new("tsconfig.JSONC")), Some(FileKind::Json));
    assert_eq!(
        FileKind::from_path(Path::new("pnpm-workspace.yaml")),
        Some(FileKind::Yaml)
    );
    assert_eq!(FileKind::from_path(Path::new("ci.yml")), Some(FileKind::Yaml));
    assert_eq!(FileKind::from_path(Path::new("README.md")), None);
    assert_eq!(FileKind::from_path(Path::new("Makefile")), None);
}

fn apply_json(input: &str, ops: &[FixOp]) -> String {
    let refs: Vec<&FixOp> = ops.iter().collect();
    apply(input, FileKind::Json, &refs).unwrap_or_else(|error| panic!("{error}"))
}

#[test]
fn byte_order_mark_is_skipped_and_kept() {
    let input = "\u{feff}{\n  \"a\": 1,\n  \"b\": 2\n}\n";
    let ops = [
        set(&["a"], json!(9)),
        insert(&[], "c", json!(3), InsertPosition::End),
        remove(&["b"]),
    ];
    assert_eq!(apply_json(input, &ops), "\u{feff}{\n  \"a\": 9,\n  \"c\": 3\n}\n");
    let refs: Vec<&FixOp> = ops.iter().collect();
    let edits = plan_edits(input, FileKind::Json, &refs).unwrap();
    assert!(edits.iter().all(|edit| edit.start >= 3), "{edits:?}");
}

#[test]
fn text_edits_keep_their_offsets_with_a_byte_order_mark() {
    let input = "\u{feff}{\"a\": 1}";
    let op = FixOp::text(
        "package.json",
        TextEdit::new(
            5,
            6,
            "x".to_owned(),
        ),
    );
    assert_eq!(apply_json(input, &[op]), "\u{feff}{\"x\": 1}");
}

#[test]
fn several_inserts_into_one_object_form_one_valid_edit() {
    let end = |key: &str, value| insert(&[], key, value, InsertPosition::End);
    run(vec![
        Case {
            name: "two inserts into an empty object",
            input: "{}",
            ops: vec![end("a", json!(1)), end("b", json!(2))],
            expected: "{\n  \"a\": 1,\n  \"b\": 2\n}",
        },
        Case {
            name: "three inserts at the end",
            input: "{\n  \"x\": 0\n}\n",
            ops: vec![end("a", json!(1)), end("b", json!(2)), end("c", json!(3))],
            expected: "{\n  \"x\": 0,\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}\n",
        },
        Case {
            name: "inserts at the start end up in reverse order",
            input: "{\n  \"x\": 0\n}\n",
            ops: vec![
                insert(&[], "a", json!(1), InsertPosition::Start),
                insert(&[], "b", json!(2), InsertPosition::Start),
            ],
            expected: "{\n  \"b\": 2,\n  \"a\": 1,\n  \"x\": 0\n}\n",
        },
        Case {
            name: "sorted inserts into the same gap keep sorted order",
            input: "{\n  \"a\": 0,\n  \"z\": 0\n}\n",
            ops: vec![
                insert(&[], "m", json!(1), InsertPosition::Sorted),
                insert(&[], "c", json!(2), InsertPosition::Sorted),
                insert(&[], "x", json!(3), InsertPosition::Sorted),
            ],
            expected: "{\n  \"a\": 0,\n  \"c\": 2,\n  \"m\": 1,\n  \"x\": 3,\n  \"z\": 0\n}\n",
        },
        Case {
            name: "inline object",
            input: "{\"a\": 1}",
            ops: vec![end("b", json!(2)), end("c", json!(3))],
            expected: "{\"a\": 1, \"b\": 2, \"c\": 3}",
        },
        Case {
            name: "a repeated key keeps the last value",
            input: "{}",
            ops: vec![end("a", json!(1)), end("a", json!(2))],
            expected: "{\n  \"a\": 2\n}",
        },
    ]);
}

#[test]
fn inserts_and_removals_share_an_object() {
    run(vec![
        Case {
            name: "remove everything, then insert",
            input: "{\n  \"a\": 1,\n  \"b\": 2\n}\n",
            ops: vec![
                remove(&["a"]),
                remove(&["b"]),
                insert(&[], "k", json!("v"), InsertPosition::End),
            ],
            expected: "{\n  \"k\": \"v\"\n}\n",
        },
        Case {
            name: "remove the last member, insert at the end",
            input: "{\n  \"a\": 1,\n  \"b\": 2\n}\n",
            ops: vec![remove(&["b"]), insert(&[], "k", json!(3), InsertPosition::End)],
            expected: "{\n  \"a\": 1,\n  \"k\": 3\n}\n",
        },
        Case {
            name: "insert before a removed member goes before the next survivor",
            input: "{\n  \"a\": 1,\n  \"c\": 3,\n  \"d\": 4\n}\n",
            ops: vec![remove(&["c"]), insert(&[], "b", json!(2), InsertPosition::Sorted)],
            expected: "{\n  \"a\": 1,\n  \"b\": 2,\n  \"d\": 4\n}\n",
        },
        Case {
            name: "re-insert a removed key",
            input: "{\n  \"a\": 1,\n  \"b\": 2\n}\n",
            ops: vec![remove(&["a"]), insert(&[], "a", json!(5), InsertPosition::End)],
            expected: "{\n  \"b\": 2,\n  \"a\": 5\n}\n",
        },
    ]);
}

#[test]
fn inserting_into_an_empty_object_keeps_its_comments() {
    run(vec![
        Case {
            name: "block comment",
            input: "{ /* keep */ }",
            ops: vec![insert(&[], "a", json!(1), InsertPosition::End)],
            expected: "{ /* keep */\n  \"a\": 1\n}",
        },
        Case {
            name: "line comment",
            input: "{\n  // keep\n}\n",
            ops: vec![insert(&[], "a", json!(1), InsertPosition::End)],
            expected: "{\n  // keep\n  \"a\": 1\n}\n",
        },
    ]);
}

#[test]
fn duplicate_keys_resolve_to_the_last_occurrence() {
    run(vec![
        Case {
            name: "set",
            input: "{\"a\": 1, \"a\": 2}",
            ops: vec![set(&["a"], json!(9))],
            expected: "{\"a\": 1, \"a\": 9}",
        },
        Case {
            name: "insert of an existing key",
            input: "{\"a\": 1, \"a\": 2}",
            ops: vec![insert(&[], "a", json!(9), InsertPosition::End)],
            expected: "{\"a\": 1, \"a\": 9}",
        },
        Case {
            name: "nested path",
            input: "{\"a\": {\"x\": 1}, \"a\": {\"x\": 2}}",
            ops: vec![set(&["a", "x"], json!(9))],
            expected: "{\"a\": {\"x\": 1}, \"a\": {\"x\": 9}}",
        },
    ]);
}

#[test]
fn edits_that_would_break_the_document_are_rejected() {
    let source = "{\"a\": 1}";
    let op = FixOp::text(
        "package.json",
        TextEdit::new(
            1,
            1,
            "\"b\": 2 \"c\": 3,".to_owned(),
        ),
    );
    let error = apply(source, FileKind::Json, &[&op]).unwrap_err();
    assert!(matches!(error, EditPlanError::InvalidOutput { .. }), "{error}");
    let error = plan_edits(source, FileKind::Json, &[&op]).unwrap_err();
    assert!(matches!(error, EditPlanError::InvalidOutput { .. }), "{error}");
}
