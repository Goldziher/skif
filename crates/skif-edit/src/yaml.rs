//! YAML edits through `yamlpatch`, which edits a document in place and keeps comments and layout.
//!
//! `yamlpatch` returns a whole new document instead of byte ranges, so the operations are applied in order and the
//! difference to the original source is reported as one [`Planned`] edit (common prefix and suffix trimmed). A YAML
//! file therefore never yields conflicts between operations; later operations see the result of earlier ones.
//!
//! Every operation addresses the original document: sequence indices of a later operation are shifted past the
//! elements earlier operations removed, so removing `#0` and `#2` of one sequence removes the original first and
//! third element.

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
    let mut removed: Vec<&[PathSeg]> = Vec::new();
    for op in ops {
        let FixOp::Yaml { route, action, .. } = op else {
            return Err(EditPlanError::Unsupported {
                op: describe::op(op),
                kind: FileKind::Yaml,
            });
        };
        let fail = |message: String| EditPlanError::Yaml {
            op: describe::op(op),
            message,
        };
        let shifted = shift_past_removals(route, &removed)
            .ok_or_else(|| fail("path was removed by an earlier operation on the same document".to_owned()))?;
        document = apply_one(&document, &shifted, action).map_err(fail)?;
        if matches!(action, YamlAction::Remove) && matches!(route.last(), Some(PathSeg::Index(_))) {
            removed.push(route);
        }
    }
    Ok(diff(source, document.source()).into_iter().collect())
}

/// Checks that `after` is still a YAML document.
pub(crate) fn validate(after: &str) -> Result<(), EditPlanError> {
    Document::new(after)
        .map(drop)
        .map_err(|error| EditPlanError::InvalidOutput {
            kind: FileKind::Yaml,
            message: error.to_string(),
        })
}

/// Re-addresses `route`, written against the original document, after the sequence elements in `removed` (original
/// routes) were deleted. `None` when `route` runs through a removed element.
fn shift_past_removals(route: &[PathSeg], removed: &[&[PathSeg]]) -> Option<Vec<PathSeg>> {
    route
        .iter()
        .enumerate()
        .map(|(depth, seg)| {
            let PathSeg::Index(index) = seg else {
                return Some(seg.clone());
            };
            let mut shifted = *index;
            for gone in removed {
                if gone.len() != depth + 1 || gone[..depth] != route[..depth] {
                    continue;
                }
                match gone[depth] {
                    PathSeg::Index(gone) if gone == *index => return None,
                    PathSeg::Index(gone) if gone < *index => shifted -= 1,
                    _ => {}
                }
            }
            Some(PathSeg::Index(shifted))
        })
        .collect()
}

fn components(path: &[PathSeg]) -> Result<Vec<Component<'static>>, String> {
    path.iter()
        .map(|seg| match seg {
            PathSeg::Key(key) => Ok(Component::Key(Cow::Owned(key.clone()))),
            PathSeg::Index(index) => Ok(Component::Index(*index)),
            _ => Err("unsupported path segment".to_owned()),
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
    let route = Route::from(components(path)?);
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
                let parent = Route::from(components(parent)?);
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
        _ => return Err("unsupported yaml action".to_owned()),
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
    let (mut start, mut end) = (prefix, before.len() - suffix);
    let replacement = &after[prefix..after.len() - suffix];
    // A deletion can sit anywhere along a run of repeated text: removing `catalog:` before `catalogs:` matches the
    // `catalog` of the next key. Slide it back to the start of a line, so that removing whole entries gives
    // line-aligned edits that do not overlap the removal of a neighbouring entry.
    if replacement.is_empty() {
        let bytes = before.as_bytes();
        while start > 0 && bytes[start - 1] != b'\n' && bytes[start - 1] == bytes[end - 1] {
            start -= 1;
            end -= 1;
        }
        if (start > 0 && bytes[start - 1] != b'\n') || !before.is_char_boundary(start) {
            (start, end) = (prefix, before.len() - suffix);
        }
    }
    Some(Planned {
        op: 0,
        start,
        end,
        replacement: replacement.to_owned(),
    })
}
