//! YAML edits through `yamlpatch`, which edits a document in place and keeps comments and layout.
//!
//! `yamlpatch` returns a whole new document instead of byte ranges, so the operations are applied in order and the
//! difference to the original source is reported as one [`Planned`] edit (common prefix and suffix trimmed). A YAML
//! file therefore never yields conflicts between operations; later operations see the result of earlier ones.

use std::borrow::Cow;

use skif_core::{FixOp, PathSeg, YamlAction};
use yamlpatch::{Op, Patch, apply_yaml_patches};
use yamlpath::{Component, Document, Route};

use crate::describe;
use crate::{EditPlanError, FileKind, Planned};

pub(crate) fn plan(source: &str, ops: &[&FixOp]) -> Result<Vec<Planned>, EditPlanError> {
    let mut document = Document::new(source).map_err(|error| EditPlanError::Parse {
        kind: FileKind::Yaml,
        message: error.to_string(),
    })?;
    for op in ops {
        let FixOp::Yaml { route, action, .. } = op else {
            return Err(EditPlanError::Unsupported {
                op: describe::op(op),
                kind: FileKind::Yaml,
            });
        };
        document = apply_one(&document, route, action).map_err(|message| EditPlanError::Yaml {
            op: describe::op(op),
            message,
        })?;
    }
    Ok(diff(source, document.source()).into_iter().collect())
}

fn components(path: &[PathSeg]) -> Vec<Component<'static>> {
    path.iter()
        .map(|seg| match seg {
            PathSeg::Key(key) => Component::Key(Cow::Owned(key.clone())),
            PathSeg::Index(index) => Component::Index(*index),
        })
        .collect()
}

fn yaml_value(value: &serde_json::Value) -> Result<yaml_serde::Value, String> {
    yaml_serde::to_value(value).map_err(|error| error.to_string())
}

fn apply_one(document: &Document, path: &[PathSeg], action: &YamlAction) -> Result<Document, String> {
    if path.is_empty() && !matches!(action, YamlAction::Append(_)) {
        return Err("the document root cannot be replaced or removed".to_owned());
    }
    let route = Route::from(components(path));
    let step = match action {
        YamlAction::Remove => Patch {
            route,
            operation: Op::Remove,
        },
        YamlAction::Append(value) => Patch {
            route,
            operation: Op::Append {
                value: yaml_value(value)?,
            },
        },
        YamlAction::Set(value) if document.query_exists(&route) => Patch {
            route,
            operation: Op::Replace(yaml_value(value)?),
        },
        YamlAction::Set(value) => match path.split_last() {
            Some((PathSeg::Key(key), parent)) => {
                let parent = Route::from(components(parent));
                if !document.query_exists(&parent) {
                    return Err("path not found in document".to_owned());
                }
                Patch {
                    route: parent,
                    operation: Op::Add {
                        key: key.clone(),
                        value: yaml_value(value)?,
                    },
                }
            }
            _ => return Err("path not found in document".to_owned()),
        },
    };
    apply_yaml_patches(document, &[step]).map_err(|error| error.to_string())
}

/// The single edit turning `before` into `after`, or `None` when they are equal.
fn diff(before: &str, after: &str) -> Option<Planned> {
    if before == after {
        return None;
    }
    let mut prefix = before.bytes().zip(after.bytes()).take_while(|(a, b)| a == b).count();
    while !(before.is_char_boundary(prefix) && after.is_char_boundary(prefix)) {
        prefix -= 1;
    }
    let max_suffix = before.len().min(after.len()) - prefix;
    let mut suffix = before
        .bytes()
        .rev()
        .zip(after.bytes().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    while !(before.is_char_boundary(before.len() - suffix) && after.is_char_boundary(after.len() - suffix)) {
        suffix -= 1;
    }
    Some(Planned {
        op: 0,
        start: prefix,
        end: before.len() - suffix,
        replacement: after[prefix..after.len() - suffix].to_owned(),
    })
}
