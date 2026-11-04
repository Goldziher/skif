#![allow(dead_code, clippy::expect_used)]

use std::path::PathBuf;

use serde_json::{Value, json};
use skif_core::{FixOp, InsertPosition, JsonAction, PathSeg, YamlAction};

/// Builds a path from segments; a segment starting with `#` is an array index.
pub fn path(segments: &[&str]) -> Vec<PathSeg> {
    segments
        .iter()
        .map(|segment| match segment.strip_prefix('#') {
            Some(index) => PathSeg::Index(index.parse().expect("index segment")),
            None => PathSeg::Key((*segment).to_owned()),
        })
        .collect()
}

pub fn json_op(pointer: &[&str], action: JsonAction) -> FixOp {
    FixOp::json(
        PathBuf::from("package.json"),
        path(pointer),
        action,
    )
}

pub fn set(pointer: &[&str], value: Value) -> FixOp {
    json_op(pointer, JsonAction::Set(value))
}

pub fn remove(pointer: &[&str]) -> FixOp {
    json_op(pointer, JsonAction::Remove)
}

pub fn insert(pointer: &[&str], key: &str, value: Value, position: InsertPosition) -> FixOp {
    json_op(
        pointer,
        JsonAction::insert(
            key.to_owned(),
            value,
            position,
        ),
    )
}

pub fn sort_keys(pointer: &[&str]) -> FixOp {
    json_op(pointer, JsonAction::SortKeys)
}

pub fn rename(pointer: &[&str], new_key: &str) -> FixOp {
    json_op(pointer, JsonAction::RenameKey(new_key.to_owned()))
}

pub fn yaml_op(route: &[&str], action: YamlAction) -> FixOp {
    FixOp::yaml(
        PathBuf::from("pnpm-workspace.yaml"),
        path(route),
        action,
    )
}

pub fn sample() -> Value {
    json!({"a": 1})
}
