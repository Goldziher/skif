use serde_json::Value;
use skif_core::{
    Category, Fix, FixOp, FixSafety, Fixability, JsonAction, Location, PathSeg, RuleMeta, Severity, YamlAction,
    diagnostic,
};
use skif_lint::{Emitter, LintContext, Rule};
use skif_workspace::{PatternFact, Workspace, WorkspaceSource};

use super::support::key;

/// Metadata of `unmatched-workspace-patterns`.
pub static META: RuleMeta = RuleMeta {
    id: "unmatched-workspace-patterns",
    category: Category::Workspace,
    default: Some(Severity::Warning),
    fixable: Fixability::Safe,
    summary: "A workspace pattern does not match any directory.",
};

/// Reports positive workspace patterns that select no directory.
///
/// Negations (`!pattern`) that remove nothing are harmless and not reported.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnmatchedWorkspacePatterns;

/// Where the pattern at `position` among the declared strings sits in its file.
fn pointer(workspace: &Workspace, position: usize) -> Option<Vec<PathSeg>> {
    let base = match workspace.facts.source {
        WorkspaceSource::PnpmWorkspaceYaml => vec![key("packages")],
        WorkspaceSource::YarnNohoistWorkspaces => vec![key("workspaces"), key("packages")],
        WorkspaceSource::PackageJsonWorkspaces => vec![key("workspaces")],
    };
    let index = if workspace.facts.source == WorkspaceSource::PnpmWorkspaceYaml {
        Some(position)
    } else {
        let items = base
            .iter()
            .try_fold(&workspace.root_package.manifest.value, |value, seg| match seg {
                PathSeg::Key(name) => value.get(name),
                PathSeg::Index(index) => value.get(*index),
            });
        items
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| item.is_string())
                    .nth(position)
            })
            .map(|(index, _)| index)
    };
    index.map(|index| base.into_iter().chain([PathSeg::Index(index)]).collect())
}

fn remove_pattern(workspace: &Workspace, fact: &PatternFact, position: usize) -> Option<FixOp> {
    let route = pointer(workspace, position)?;
    Some(match workspace.facts.source {
        WorkspaceSource::PnpmWorkspaceYaml => FixOp::Yaml {
            path: fact.file.clone(),
            route,
            action: YamlAction::Remove,
        },
        _ => FixOp::Json {
            path: fact.file.clone(),
            pointer: route,
            action: JsonAction::Remove,
        },
    })
}

impl Rule for UnmatchedWorkspacePatterns {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        let workspace = cx.workspace;
        for (position, fact) in workspace.facts.patterns.iter().enumerate() {
            if fact.negated || fact.matched > 0 {
                continue;
            }
            let mut builder = diagnostic(
                &META,
                format!("workspace pattern `{}` does not match any directory", fact.pattern),
                Location {
                    path: fact.file.clone(),
                    span: fact.span,
                },
            )
            .help("remove the pattern, or create the directories it is meant to match");
            if let Some(op) = remove_pattern(workspace, fact, position) {
                builder = builder.fix(Fix {
                    title: format!("Remove the pattern `{}`", fact.pattern),
                    safety: FixSafety::Safe,
                    ops: vec![op],
                });
            }
            out.emit(builder.build());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, assert_fixes_clear, config, lint, located};

    const MEMBER: &str = r#"{"name":"a"}"#;

    fn root_with(workspaces: &str) -> String {
        format!(r#"{{"name":"root","private":true,"packageManager":"npm@10.0.0","workspaces":{workspaces}}}"#)
    }

    #[test]
    fn pattern_table() {
        let cases = [
            ("all match", r#"["packages/*"]"#, vec![]),
            ("exact directory", r#"["packages/a"]"#, vec![]),
            ("missing directory", r#"["packages/a","docs"]"#, vec!["docs"]),
            ("missing glob", r#"["packages/*","apps/*"]"#, vec!["apps/*"]),
            ("two missing", r#"["x","packages/*","y/*"]"#, vec!["x", "y/*"]),
            (
                "negation that removes nothing is fine",
                r#"["packages/*","!nothing/*"]"#,
                vec![],
            ),
            (
                "deep glob with no match",
                r#"["packages/*","tools/**"]"#,
                vec!["tools/**"],
            ),
        ];
        for (name, workspaces, expected) in cases {
            let root_json = root_with(workspaces);
            let files = [
                ("package.json", root_json.as_str()),
                ("packages/a/package.json", MEMBER),
            ];
            let found: Vec<String> = lint(META.id, &files, &config(""))
                .iter()
                .map(|d| located(&files, d).trim_matches('"').to_owned())
                .collect();
            assert_eq!(found, expected, "{name}");
        }
    }

    #[test]
    fn a_directory_without_manifest_still_counts_as_matched() {
        let root_json = root_with(r#"["packages/*","bare"]"#);
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", MEMBER),
            ("bare/README.md", "hi"),
        ];
        assert_eq!(lint(META.id, &files, &config("")), [] as [skif_core::Diagnostic; 0]);
    }

    #[test]
    fn the_span_covers_the_pattern_string_in_the_root_manifest() {
        let root_json = root_with(r#"["packages/*","docs"]"#);
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", MEMBER),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(located(&files, &diagnostics[0]), r#""docs""#);
        assert_eq!(diagnostics[0].location.path.to_str(), Some("package.json"));
        assert!(diagnostics[0].package.is_none());
    }

    #[test]
    fn the_span_covers_the_pattern_in_pnpm_workspace_yaml() {
        let root_json = r#"{"name":"root","private":true,"packageManager":"pnpm@9.0.0"}"#;
        let yaml = "packages:\n  - 'packages/*'\n  - 'docs'\n";
        let files = [
            ("package.json", root_json),
            ("pnpm-workspace.yaml", yaml),
            ("packages/a/package.json", MEMBER),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].location.path.to_str(), Some("pnpm-workspace.yaml"));
        assert!(located(&files, &diagnostics[0]).contains("docs"));
    }

    #[test]
    fn json_fix_removes_every_unmatched_pattern() {
        let root_json = root_with(r#"["x","packages/*","y/*","!z"]"#);
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", MEMBER),
        ];
        assert_fixes_clear(META.id, &files, &config(""));
        let fixed = testing::fixed_json(META.id, &files, &config(""), "package.json");
        assert_eq!(fixed["workspaces"], serde_json::json!(["packages/*", "!z"]));
    }

    #[test]
    fn yarn_nohoist_object_form_is_addressed_through_packages() {
        let root_json = r#"{"name":"root","private":true,"packageManager":"yarn@1.22.0","workspaces":{"packages":["packages/*","gone"],"nohoist":["**/x"]}}"#;
        let files = [("package.json", root_json), ("packages/a/package.json", MEMBER)];
        assert_fixes_clear(META.id, &files, &config(""));
        let fixed = testing::fixed_json(META.id, &files, &config(""), "package.json");
        assert_eq!(fixed["workspaces"]["packages"], serde_json::json!(["packages/*"]));
        assert_eq!(fixed["workspaces"]["nohoist"], serde_json::json!(["**/x"]));
    }

    #[test]
    fn yaml_fix_removes_the_pattern_from_pnpm_workspace_yaml() {
        let root_json = r#"{"name":"root","private":true,"packageManager":"pnpm@9.0.0"}"#;
        let yaml = "packages:\n  - 'packages/*'\n  - 'docs'\n";
        let files = [
            ("package.json", root_json),
            ("pnpm-workspace.yaml", yaml),
            ("packages/a/package.json", MEMBER),
        ];
        assert_fixes_clear(META.id, &files, &config(""));
        let fixed = testing::apply_fixes(&files, &lint(META.id, &files, &config("")));
        let (_, text) = fixed.iter().find(|(path, _)| path == "pnpm-workspace.yaml").unwrap();
        assert!(text.contains("packages/*") && !text.contains("docs"), "{text}");
    }

    #[test]
    fn fixes_address_the_workspace_files_not_the_working_directory() {
        let root_json = root_with(r#"["gone","packages/*"]"#);
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", MEMBER),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        let FixOp::Json { path, pointer, .. } = &diagnostics[0].fixes[0].ops[0] else {
            panic!("expected a JSON operation");
        };
        assert_eq!(path.to_str(), Some("package.json"));
        assert_eq!(pointer, &[key("workspaces"), PathSeg::Index(0)]);
    }
}
