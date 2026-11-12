use std::path::PathBuf;

use skif_core::{Category, Fixability, Location, RuleMeta, Severity, diagnostic, slash};
use skif_lint::{Emitter, LintContext, Rule};
use skif_workspace::WorkspaceSource;

/// Metadata of `lerna-packages-mismatch`.
pub static META: RuleMeta = RuleMeta::new(
    "lerna-packages-mismatch",
    Category::Workspace,
    Some(Severity::Warning),
    Fixability::None,
    "The `packages` of `lerna.json`, which Lerna reads before the package manager's workspaces, select other packages than the workspace declaration.",
);

/// Reports a `lerna.json` whose `packages` select other directories than the workspace declaration that skif and the
/// package manager follow (`workspaces` in `package.json`, `pnpm-workspace.yaml` or `rush.json`).
///
/// Lerna reads `packages` first, so `lerna run`, `lerna version` and `lerna publish` then act on packages the package
/// manager does not install, or skip members it does. The finding names both lists and the directories only one of
/// them selects. A `lerna.json` with `useWorkspaces: true` is not reported: Lerna before 7 then follows the package
/// manager, and `stale-lerna-config` reports the key for later releases.
#[derive(Debug, Clone, Copy, Default)]
pub struct LernaPackagesMismatch;

/// How the message names the file whose patterns decide the members.
fn declaration(source: WorkspaceSource) -> &'static str {
    match source {
        WorkspaceSource::PnpmWorkspaceYaml => "the `packages` of `pnpm-workspace.yaml`",
        WorkspaceSource::RushJson => "the projects of `rush.json`",
        WorkspaceSource::YarnNohoistWorkspaces => "the `workspaces.packages` of `package.json`",
        _ => "the `workspaces` of `package.json`",
    }
}

/// `dirs` quoted and joined, for a message.
fn quoted(dirs: &[PathBuf]) -> String {
    dirs.iter()
        .map(|dir| format!("`{}`", slash(dir)))
        .collect::<Vec<_>>()
        .join(", ")
}

impl Rule for LernaPackagesMismatch {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        let facts = &cx.workspace.facts;
        let Some(lerna) = &facts.lerna else {
            return;
        };
        if lerna.use_workspaces == Some(true) || (lerna.only_in_lerna.is_empty() && lerna.only_in_workspace.is_empty())
        {
            return;
        }
        let globs = lerna
            .packages
            .iter()
            .flatten()
            .map(|glob| format!("`{glob}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let declared = facts
            .patterns
            .iter()
            .map(|fact| format!("`{}`", fact.pattern))
            .collect::<Vec<_>>()
            .join(", ");
        let mut differences = Vec::new();
        if !lerna.only_in_lerna.is_empty() {
            differences.push(format!("only Lerna acts on {}", quoted(&lerna.only_in_lerna)));
        }
        if !lerna.only_in_workspace.is_empty() {
            differences.push(format!("Lerna skips {}", quoted(&lerna.only_in_workspace)));
        }
        out.emit(
            diagnostic(
                &META,
                format!(
                    "`lerna.json` lists packages [{globs}], but {} are [{declared}]: {}",
                    declaration(facts.source),
                    differences.join("; ")
                ),
                Location::new(lerna.path.clone(), lerna.packages_span),
            )
            .help("delete `packages` from `lerna.json` so Lerna follows the package manager's workspaces, or make both lists select the same packages")
            .build(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{config, lint, located};

    const MEMBER: &str = r#"{"name":"a"}"#;

    fn messages(files: &[(&str, &str)]) -> Vec<String> {
        lint(META.id, files, &config(""))
            .iter()
            .map(|d| d.message.clone())
            .collect()
    }

    #[test]
    fn packages_lerna_alone_selects_are_reported_at_the_list() {
        let lerna = r#"{"version": "1.0.0", "packages": ["packages/*", "tools/*"]}"#;
        let files = [
            (
                "package.json",
                r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
            ),
            ("lerna.json", lerna),
            ("packages/a/package.json", MEMBER),
            ("tools/cli/package.json", r#"{"name":"cli"}"#),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(
            diagnostics[0].message,
            "`lerna.json` lists packages [`packages/*`, `tools/*`], but the `workspaces` of `package.json` are \
             [`packages/*`]: only Lerna acts on `tools/cli`"
        );
        assert_eq!(diagnostics[0].location.path.to_str(), Some("lerna.json"));
        assert_eq!(
            located(&files, &diagnostics[0]),
            r#""packages": ["packages/*", "tools/*"]"#
        );
    }

    #[test]
    fn members_lerna_skips_are_reported() {
        let files = [
            (
                "package.json",
                r#"{"name":"root","private":true,"packageManager":"pnpm@9.0.0"}"#,
            ),
            ("pnpm-workspace.yaml", "packages:\n  - packages/*\n  - apps/*\n"),
            ("lerna.json", r#"{"version": "1.0.0", "packages": ["packages/*"]}"#),
            ("packages/a/package.json", MEMBER),
            ("apps/web/package.json", r#"{"name":"web"}"#),
        ];
        assert_eq!(
            messages(&files),
            [
                "`lerna.json` lists packages [`packages/*`], but the `packages` of `pnpm-workspace.yaml` are \
              [`packages/*`, `apps/*`]: Lerna skips `apps/web`"
            ]
        );
    }

    #[test]
    fn both_directions_are_named() {
        let files = [
            (
                "package.json",
                r#"{"name":"root","private":true,"workspaces":["apps/*"]}"#,
            ),
            ("lerna.json", r#"{"version": "1.0.0", "packages": ["packages/*"]}"#),
            ("packages/a/package.json", MEMBER),
            ("apps/web/package.json", r#"{"name":"web"}"#),
        ];
        assert_eq!(
            messages(&files),
            [
                "`lerna.json` lists packages [`packages/*`], but the `workspaces` of `package.json` are [`apps/*`]: \
              only Lerna acts on `packages/a`; Lerna skips `apps/web`"
            ]
        );
    }

    #[test]
    fn lists_that_select_the_same_packages_are_clean() {
        let files = [
            (
                "package.json",
                r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#,
            ),
            (
                "lerna.json",
                r#"{"version": "1.0.0", "packages": ["packages/a", "packages/b"]}"#,
            ),
            ("packages/a/package.json", MEMBER),
            ("packages/b/package.json", r#"{"name":"b"}"#),
        ];
        assert_eq!(messages(&files), [] as [String; 0]);
    }

    #[test]
    fn use_workspaces_or_no_packages_or_lerna_alone_are_clean() {
        let root = r#"{"name":"root","private":true,"workspaces":["packages/*"]}"#;
        let tools = ("tools/cli/package.json", r#"{"name":"cli"}"#);
        let use_workspaces = [
            ("package.json", root),
            (
                "lerna.json",
                r#"{"version": "1.0.0", "useWorkspaces": true, "packages": ["tools/*"]}"#,
            ),
            ("packages/a/package.json", MEMBER),
            tools,
        ];
        assert_eq!(messages(&use_workspaces), [] as [String; 0]);
        let no_packages = [
            ("package.json", root),
            ("lerna.json", r#"{"version": "1.0.0"}"#),
            ("packages/a/package.json", MEMBER),
            tools,
        ];
        assert_eq!(messages(&no_packages), [] as [String; 0]);
        let lerna_alone = [
            ("package.json", r#"{"name":"root","private":true}"#),
            ("lerna.json", r#"{"version": "1.0.0", "packages": ["tools/*"]}"#),
            ("packages/a/package.json", MEMBER),
            tools,
        ];
        assert_eq!(messages(&lerna_alone), [] as [String; 0]);
    }
}
