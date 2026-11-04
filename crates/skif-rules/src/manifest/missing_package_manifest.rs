use serde_json::json;
use skif_core::{Category, Fix, FixOp, FixSafety, Fixability, Location, RuleMeta, Severity, diagnostic, slash};
use skif_lint::{Emitter, LintContext, Rule};

/// Metadata of `missing-package-manifest`.
pub static META: RuleMeta = RuleMeta::new(
    "missing-package-manifest",
    Category::Workspace,
    Some(Severity::Warning),
    Fixability::Unsafe,
    "A directory matched by the workspace patterns has no `package.json`.",
);

/// Reports non-empty directories matched by the workspace patterns that lack a `package.json`.
///
/// Empty directories (left behind by switching branches), hidden directories and `node_modules` are not reported.
#[derive(Debug, Clone, Copy, Default)]
pub struct MissingPackageManifest;

impl Rule for MissingPackageManifest {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        for dir in &cx.workspace.facts.dirs_without_manifest {
            let manifest = dir.join("package.json");
            let mut builder = diagnostic(
                &META,
                format!("`{}` matches a workspace pattern but has no package.json", slash(dir)),
                Location::new(
                    dir.clone(),
                    None,
                ),
            )
            .help("add a package.json, or narrow the workspace patterns so the directory is not matched");
            if let Some(name) = dir.file_name().and_then(|name| name.to_str()) {
                let contents = serde_json::to_string_pretty(&json!({
                    "name": name,
                    "version": "0.0.0",
                    "private": true,
                }));
                if let Ok(contents) = contents {
                    builder = builder.fix(Fix::new(
                        format!("Create `{}`", slash(&manifest)),
                        FixSafety::Unsafe,
                        vec![FixOp::create_file(
                            manifest,
                            format!("{contents}\n"),
                        )],
                    ));
                }
            }
            out.emit(builder.build());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, assert_fixes_clear, config, lint, root};

    const MEMBER: &str = r#"{"name":"a"}"#;

    #[test]
    fn directory_table() {
        let root_json = root("");
        let cases = [
            (
                "every directory has one",
                vec![("packages/a/package.json", MEMBER)],
                vec![],
            ),
            (
                "non-empty without manifest",
                vec![("packages/a/package.json", MEMBER), ("packages/b/index.js", "")],
                vec!["packages/b"],
            ),
            (
                "several, sorted",
                vec![
                    ("packages/z/x", ""),
                    ("packages/a/package.json", MEMBER),
                    ("packages/m/x", ""),
                ],
                vec!["packages/m", "packages/z"],
            ),
            (
                "hidden and node_modules are not matched",
                vec![
                    ("packages/a/package.json", MEMBER),
                    ("packages/.cache/x", ""),
                    ("packages/node_modules/x", ""),
                ],
                vec![],
            ),
            (
                "unicode directory",
                vec![("packages/a/package.json", MEMBER), ("packages/pä/x", "")],
                vec!["packages/pä"],
            ),
        ];
        for (name, members, expected) in cases {
            let mut files = vec![("package.json", root_json.as_str())];
            files.extend(members);
            let found: Vec<String> = lint(META.id, &files, &config(""))
                .iter()
                .map(|d| d.location.path.to_string_lossy().into_owned())
                .collect();
            assert_eq!(found, expected, "{name}");
        }
    }

    #[test]
    fn the_location_is_the_directory_without_a_span() {
        let root_json = root("");
        let files = [("package.json", root_json.as_str()), ("packages/b/index.js", "")];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics[0].location.path.to_str(), Some("packages/b"));
        assert!(diagnostics[0].location.span.is_none());
    }

    #[test]
    fn fix_creates_a_private_manifest_named_after_the_directory() {
        let root_json = root("");
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", MEMBER),
            ("packages/b/index.js", ""),
        ];
        assert_fixes_clear(META.id, &files, &config(""));
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics[0].fixes[0].safety, FixSafety::Unsafe);
        let fixed = testing::apply_fixes(&files, &diagnostics);
        let (_, text) = fixed
            .iter()
            .find(|(path, _)| path == "packages/b/package.json")
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"name": "b", "version": "0.0.0", "private": true})
        );
    }
}
