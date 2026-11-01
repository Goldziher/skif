//! Property test: editing text with skif-edit equals applying the same operation to the parsed value.

#![allow(clippy::expect_used)]

mod common;

use proptest::prelude::*;
use serde_json::{Map, Value};
use skif_core::{FixOp, InsertPosition, JsonAction, PathSeg};
use skif_edit::{FileKind, apply};

fn key() -> impl Strategy<Value = String> {
    "[a-e0-9éключ]{1,3}"
}

fn scalar() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i32>().prop_map(Value::from),
        "[a-z \"\\\\é🎉/]{0,4}".prop_map(Value::String),
    ]
}

fn dedup(entries: Vec<(String, Value)>) -> Map<String, Value> {
    let mut map = Map::new();
    for (key, value) in entries {
        map.entry(key).or_insert(value);
    }
    map
}

fn value() -> impl Strategy<Value = Value> {
    scalar().prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::vec((key(), inner), 0..4).prop_map(|entries| Value::Object(dedup(entries))),
        ]
    })
}

fn document() -> impl Strategy<Value = Value> {
    prop::collection::vec((key(), value()), 0..5).prop_map(|entries| Value::Object(dedup(entries)))
}

#[derive(Debug, Clone)]
struct Layout {
    unit: &'static str,
    crlf: bool,
    final_newline: bool,
}

fn layout() -> impl Strategy<Value = Layout> {
    (
        prop::sample::select(vec!["  ", "    ", "\t"]),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(|(unit, crlf, final_newline)| Layout {
            unit,
            crlf,
            final_newline,
        })
}

fn format_document(value: &Value, layout: &Layout) -> String {
    let pretty = serde_json::to_string_pretty(value).expect("serialize");
    let mut lines: Vec<String> = pretty
        .lines()
        .map(|line| {
            let width = line.len() - line.trim_start_matches(' ').len();
            format!("{}{}", layout.unit.repeat(width / 2), &line[width..])
        })
        .collect();
    if layout.final_newline {
        lines.push(String::new());
    }
    lines.join(if layout.crlf { "\r\n" } else { "\n" })
}

type Node<'v> = (Vec<PathSeg>, &'v Value, bool);

/// Every node as `(path, value, parent_is_object)`.
fn nodes(value: &Value) -> Vec<Node<'_>> {
    fn walk<'v>(
        value: &'v Value,
        path: &mut Vec<PathSeg>,
        parent_object: bool,
        out: &mut Vec<(Vec<PathSeg>, &'v Value, bool)>,
    ) {
        out.push((path.clone(), value, parent_object));
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    path.push(PathSeg::Key(key.clone()));
                    walk(child, path, true, out);
                    path.pop();
                }
            }
            Value::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    path.push(PathSeg::Index(index));
                    walk(child, path, false, out);
                    path.pop();
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, &mut Vec::new(), false, &mut out);
    out
}

fn at_mut<'v>(root: &'v mut Value, path: &[PathSeg]) -> &'v mut Value {
    path.iter().fold(root, |current, seg| match (seg, current) {
        (PathSeg::Key(key), Value::Object(map)) => map.get_mut(key).expect("key"),
        (PathSeg::Index(index), Value::Array(items)) => &mut items[*index],
        _ => unreachable!("generated paths are valid"),
    })
}

fn entries(map: &Map<String, Value>) -> Vec<(String, Value)> {
    map.iter().map(|(key, value)| (key.clone(), value.clone())).collect()
}

fn from_entries(entries: Vec<(String, Value)>) -> Value {
    Value::Object(entries.into_iter().collect())
}

/// The same operation applied to the parsed value.
fn reference(root: &Value, path: &[PathSeg], action: &JsonAction) -> Value {
    let mut result = root.clone();
    match action {
        JsonAction::Set(value) => *at_mut(&mut result, path) = value.clone(),
        JsonAction::Remove | JsonAction::RenameKey(_) => {
            let (last, parent_path) = path.split_last().expect("non-root");
            let parent = at_mut(&mut result, parent_path);
            match (parent, last) {
                (Value::Array(items), PathSeg::Index(index)) => {
                    items.remove(*index);
                }
                (parent @ Value::Object(_), PathSeg::Key(key)) => {
                    let Value::Object(map) = &*parent else { unreachable!() };
                    let rebuilt = entries(map)
                        .into_iter()
                        .filter_map(|(name, value)| match action {
                            JsonAction::RenameKey(new) if &name == key => Some((new.clone(), value)),
                            JsonAction::Remove if &name == key => None,
                            _ => Some((name, value)),
                        })
                        .collect();
                    *parent = from_entries(rebuilt);
                }
                _ => unreachable!("generated paths are valid"),
            }
        }
        JsonAction::Insert { key, value, position } => {
            let target = at_mut(&mut result, path);
            let Value::Object(map) = &*target else { unreachable!() };
            let mut list = entries(map);
            if let Some(existing) = list.iter_mut().find(|(name, _)| name == key) {
                existing.1 = value.clone();
            } else {
                let slot = match position {
                    InsertPosition::Start => 0,
                    InsertPosition::End => list.len(),
                    InsertPosition::Sorted => list.iter().position(|(name, _)| name > key).unwrap_or(list.len()),
                };
                list.insert(slot, (key.clone(), value.clone()));
            }
            *target = from_entries(list);
        }
        JsonAction::SortKeys => {
            let target = at_mut(&mut result, path);
            let Value::Object(map) = &*target else { unreachable!() };
            let mut list = entries(map);
            list.sort_by(|a, b| a.0.cmp(&b.0));
            *target = from_entries(list);
        }
    }
    result
}

fn choose(
    root: &Value,
    selector: usize,
    kind: u8,
    new_key: String,
    new_value: Value,
    position: u8,
) -> (Vec<PathSeg>, JsonAction) {
    let all = nodes(root);
    let pick = |filter: &dyn Fn(&Node) -> bool| {
        let candidates: Vec<_> = all.iter().filter(|node| filter(node)).collect();
        (!candidates.is_empty()).then(|| candidates[selector % candidates.len()].0.clone())
    };
    let position = match position % 3 {
        0 => InsertPosition::Start,
        1 => InsertPosition::End,
        _ => InsertPosition::Sorted,
    };
    match kind % 5 {
        1 => pick(&|node| !node.0.is_empty()).map(|path| (path, JsonAction::Remove)),
        2 => pick(&|node| node.1.is_object()).map(|path| {
            (
                path,
                JsonAction::Insert {
                    key: new_key,
                    value: new_value.clone(),
                    position,
                },
            )
        }),
        3 => pick(&|node| node.1.is_object()).map(|path| (path, JsonAction::SortKeys)),
        4 => pick(&|node| node.2).map(|path| (path, JsonAction::RenameKey("zz".to_owned()))),
        _ => None,
    }
    .unwrap_or_else(|| (pick(&|_| true).unwrap_or_default(), JsonAction::Set(new_value)))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn edit_matches_value_semantics(
        doc in document(),
        layout in layout(),
        selector in any::<usize>(),
        kind in any::<u8>(),
        new_key in key(),
        new_value in value(),
        position in any::<u8>(),
    ) {
        let source = format_document(&doc, &layout);
        let (path, action) = choose(&doc, selector, kind, new_key, new_value, position);
        let expected = reference(&doc, &path, &action);
        let op = FixOp::Json { path: "package.json".into(), pointer: path, action };

        let edited = apply(&source, FileKind::Json, &[&op]).expect("edit applies");

        let parsed: Value = serde_json::from_str(&edited).expect("output is valid JSON");
        prop_assert_eq!(serde_json::to_string(&parsed).unwrap(), serde_json::to_string(&expected).unwrap());
        prop_assert_eq!(edited.ends_with('\n'), layout.final_newline);
        // A document without any line break (`{}`) carries no line-ending evidence.
        if source.contains("\r\n") {
            prop_assert!(!edited.replace("\r\n", "").contains('\n'), "bare LF in CRLF output: {edited:?}");
        } else if !layout.crlf {
            prop_assert!(!edited.contains('\r'));
        }
    }

    #[test]
    fn removing_every_member_leaves_an_empty_container(doc in document(), layout in layout()) {
        let source = format_document(&doc, &layout);
        let Value::Object(map) = &doc else { unreachable!() };
        let ops: Vec<FixOp> = map.keys().map(|key| common::remove(&[key.as_str()])).collect();
        let refs: Vec<&FixOp> = ops.iter().collect();
        let edited = apply(&source, FileKind::Json, &refs).expect("edit applies");
        prop_assert_eq!(serde_json::from_str::<Value>(&edited).unwrap(), Value::Object(Map::new()));
    }

    #[test]
    fn removing_a_subset_keeps_the_rest(doc in document(), layout in layout(), mask in any::<u8>()) {
        let source = format_document(&doc, &layout);
        let Value::Object(map) = &doc else { unreachable!() };
        let doomed: Vec<&String> = map.keys().enumerate().filter(|(i, _)| mask >> (i % 8) & 1 == 1).map(|(_, k)| k).collect();
        let ops: Vec<FixOp> = doomed.iter().map(|key| common::remove(&[key.as_str()])).collect();
        let refs: Vec<&FixOp> = ops.iter().collect();
        let edited = apply(&source, FileKind::Json, &refs).expect("edit applies");
        let expected: Map<String, Value> = map.iter().filter(|(k, _)| !doomed.contains(k)).map(|(k, v)| (k.clone(), v.clone())).collect();
        let parsed: Value = serde_json::from_str(&edited).expect("output is valid JSON");
        prop_assert_eq!(serde_json::to_string(&parsed).unwrap(), serde_json::to_string(&Value::Object(expected)).unwrap());
    }
}
