//! The unit of output of every rule.

use serde::{Deserialize, Serialize};

use crate::fix::Fix;
use crate::package_id::PackageId;
use crate::rule::{RuleId, Severity};
use crate::span::{Location, Related};

/// A single finding reported by a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct Diagnostic {
    /// Rule that produced the finding.
    pub rule: RuleId,
    /// Effective severity.
    pub severity: Severity,
    /// Primary message.
    pub message: String,
    /// Suggestion on how to resolve the finding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Primary location.
    pub location: Location,
    /// Secondary locations that explain the finding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<Related>,
    /// Display name of the affected package, for output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// Identity of the affected package within the workspace being linted. Used by suppressions in place of the
    /// display name, which is not unique; never serialized, because ids are meaningful only in-process.
    #[serde(skip)]
    #[cfg_attr(feature = "schema", schemars(skip))]
    pub package_id: Option<PackageId>,
    /// Name of the affected dependency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependency: Option<String>,
    /// Extra machine-readable context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Alternative fixes; the first is preferred.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fixes: Vec<Fix>,
}

impl Diagnostic {
    /// Whether at least one fix is attached.
    #[must_use]
    pub fn is_fixable(&self) -> bool {
        !self.fixes.is_empty()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::fix::{FixOp, FixSafety, InsertPosition, JsonAction, PathSeg, TextEdit, YamlAction};
    use crate::span::Span;

    fn all_ops() -> Vec<FixOp> {
        let json_op = |action| FixOp::Json {
            path: "package.json".into(),
            pointer: vec![PathSeg::Key("dependencies".into()), PathSeg::Index(0)],
            action,
        };
        let yaml_op = |action| FixOp::Yaml {
            path: "pnpm-workspace.yaml".into(),
            route: vec![PathSeg::Key("catalog".into())],
            action,
        };
        vec![
            json_op(JsonAction::Set(json!(1))),
            json_op(JsonAction::Remove),
            json_op(JsonAction::Insert {
                key: "k".into(),
                value: json!("v"),
                position: InsertPosition::Start,
            }),
            json_op(JsonAction::Insert {
                key: "k".into(),
                value: json!(null),
                position: InsertPosition::End,
            }),
            json_op(JsonAction::Insert {
                key: "k".into(),
                value: json!([1]),
                position: InsertPosition::Sorted,
            }),
            json_op(JsonAction::SortKeys),
            json_op(JsonAction::RenameKey("new".into())),
            yaml_op(YamlAction::Set(json!({ "a": true }))),
            yaml_op(YamlAction::Remove),
            yaml_op(YamlAction::Append(json!("x"))),
            FixOp::Text {
                path: "a.txt".into(),
                edit: TextEdit {
                    start: 0,
                    end: 1,
                    replacement: "z".into(),
                },
            },
            FixOp::CreateFile {
                path: "new.txt".into(),
                contents: "hi".into(),
            },
            FixOp::DeleteFile { path: "old.txt".into() },
        ]
    }

    fn diagnostic() -> Diagnostic {
        Diagnostic {
            rule: RuleId::from_static("unused-dependency"),
            severity: Severity::Warning,
            message: "unused".into(),
            help: Some("remove it".into()),
            location: Location {
                path: "package.json".into(),
                span: Some(Span {
                    start: 0,
                    end: 3,
                    start_line: 1,
                    start_col: 1,
                    end_line: 1,
                    end_col: 4,
                }),
            },
            related: Vec::new(),
            package: Some("pkg".into()),
            package_id: None,
            dependency: None,
            detail: None,
            fixes: vec![Fix {
                title: "apply".into(),
                safety: FixSafety::Unsafe,
                ops: all_ops(),
            }],
        }
    }

    #[test]
    fn serializes_every_fix_op_variant() {
        let expected = json!({
            "rule": "unused-dependency",
            "severity": "warning",
            "message": "unused",
            "help": "remove it",
            "location": {
                "path": "package.json",
                "span": { "start": 0, "end": 3, "start-line": 1, "start-col": 1, "end-line": 1, "end-col": 4 },
            },
            "package": "pkg",
            "fixes": [{
                "title": "apply",
                "safety": "unsafe",
                "ops": [
                    { "json": { "path": "package.json", "pointer": [{ "key": "dependencies" }, { "index": 0 }], "action": { "set": 1 } } },
                    { "json": { "path": "package.json", "pointer": [{ "key": "dependencies" }, { "index": 0 }], "action": "remove" } },
                    { "json": { "path": "package.json", "pointer": [{ "key": "dependencies" }, { "index": 0 }], "action": { "insert": { "key": "k", "value": "v", "position": "start" } } } },
                    { "json": { "path": "package.json", "pointer": [{ "key": "dependencies" }, { "index": 0 }], "action": { "insert": { "key": "k", "value": null, "position": "end" } } } },
                    { "json": { "path": "package.json", "pointer": [{ "key": "dependencies" }, { "index": 0 }], "action": { "insert": { "key": "k", "value": [1], "position": "sorted" } } } },
                    { "json": { "path": "package.json", "pointer": [{ "key": "dependencies" }, { "index": 0 }], "action": "sort-keys" } },
                    { "json": { "path": "package.json", "pointer": [{ "key": "dependencies" }, { "index": 0 }], "action": { "rename-key": "new" } } },
                    { "yaml": { "path": "pnpm-workspace.yaml", "route": [{ "key": "catalog" }], "action": { "set": { "a": true } } } },
                    { "yaml": { "path": "pnpm-workspace.yaml", "route": [{ "key": "catalog" }], "action": "remove" } },
                    { "yaml": { "path": "pnpm-workspace.yaml", "route": [{ "key": "catalog" }], "action": { "append": "x" } } },
                    { "text": { "path": "a.txt", "edit": { "start": 0, "end": 1, "replacement": "z" } } },
                    { "create-file": { "path": "new.txt", "contents": "hi" } },
                    { "delete-file": { "path": "old.txt" } },
                ],
            }],
        });
        assert_eq!(serde_json::to_value(diagnostic()).unwrap(), expected);
    }

    #[test]
    fn round_trips_through_json() {
        let original = diagnostic();
        let text = serde_json::to_string(&original).unwrap();
        assert_eq!(serde_json::from_str::<Diagnostic>(&text).unwrap(), original);
    }

    #[test]
    fn package_id_is_not_serialized() {
        let mut with_id = diagnostic();
        with_id.package_id = Some(PackageId::from_index(2));
        let value = serde_json::to_value(&with_id).unwrap();
        assert!(value.get("package-id").is_none() && value.get("package_id").is_none(), "{value}");
        assert_eq!(value, serde_json::to_value(diagnostic()).unwrap());
        let back: Diagnostic = serde_json::from_value(value).unwrap();
        assert_eq!(back.package_id, None);
    }

    #[test]
    fn minimal_diagnostic_deserializes_with_defaults() {
        let parsed: Diagnostic = serde_json::from_value(json!({
            "rule": "r",
            "severity": "info",
            "message": "m",
            "location": { "path": "p" },
        }))
        .unwrap();
        assert_eq!(parsed.help, None);
        assert!(parsed.fixes.is_empty() && parsed.related.is_empty());
        assert!(!parsed.is_fixable());
        assert!(diagnostic().is_fixable());
    }
}
