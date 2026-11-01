//! Human-readable operation descriptions for error messages.

use std::fmt::Write as _;

use skif_core::{FixOp, JsonAction, PathSeg, YamlAction};

pub(crate) fn op(op: &FixOp) -> String {
    match op {
        FixOp::Json { pointer, action, .. } => {
            let verb = match action {
                JsonAction::Set(_) => "set",
                JsonAction::Remove => "remove",
                JsonAction::Insert { .. } => "insert into",
                JsonAction::SortKeys => "sort keys of",
                JsonAction::RenameKey(_) => "rename",
            };
            format!("json {verb} {}", pointer_text(pointer))
        }
        FixOp::Yaml { route, action, .. } => {
            let verb = match action {
                YamlAction::Set(_) => "set",
                YamlAction::Remove => "remove",
                YamlAction::Append(_) => "append to",
            };
            format!("yaml {verb} {}", pointer_text(route))
        }
        FixOp::Text { path, edit } => format!("text edit {}..{} in {}", edit.start, edit.end, path.display()),
        FixOp::CreateFile { path, .. } => format!("create {}", path.display()),
        FixOp::DeleteFile { path } => format!("delete {}", path.display()),
    }
}

pub(crate) fn pointer_text(path: &[PathSeg]) -> String {
    if path.is_empty() {
        return "/".to_owned();
    }
    path.iter().fold(String::new(), |mut out, seg| {
        match seg {
            PathSeg::Key(key) => {
                let _ = write!(out, "/{}", key.replace('~', "~0").replace('/', "~1"));
            }
            PathSeg::Index(index) => {
                let _ = write!(out, "/{index}");
            }
        }
        out
    })
}
