use skif_config::Format;
use skif_core::{Category, Fix, FixOp, FixSafety, Fixability, RuleMeta, diagnostic};
use skif_edit::{FormatOptions, format_package_json, minimal_edit};
use skif_lint::{Emitter, LintContext, Rule};

use super::support::{active_packages, document_location};

/// Metadata of `package-json-format`.
pub static META: RuleMeta = RuleMeta::new(
    "package-json-format",
    Category::Manifest,
    None,
    Fixability::Sometimes,
    "A `package.json` is not formatted as `[format]` describes: key order, sorted fields, `exports` conditions, shorthand `bugs` and `repository`, and the indent. The fix is unsafe when `sort-exports` reorders conditions other than `types` and `default`.",
);

/// Reports manifests that `skif format` would change (ADR 0050).
#[derive(Debug, Clone, Copy, Default)]
pub struct PackageJsonFormat;

/// The formatter options of a `[format]` table.
pub(crate) fn options(format: &Format) -> FormatOptions {
    FormatOptions::default()
        .with_sort_first(format.sort_first.clone())
        .with_sort_az(format.sort_az.clone())
        .with_sort_exports(format.sort_exports.clone())
        .with_sort_packages(format.sort_packages)
        .with_format_bugs(format.format_bugs)
        .with_format_repository(format.format_repository)
        .with_indent(format.indent_text())
}

impl Rule for PackageJsonFormat {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        let options = options(&cx.config.format);
        for package in active_packages(cx.workspace) {
            let source = package.manifest.source();
            // A manifest the workspace could read but the formatter cannot is left to the rules that report it.
            let Ok(formatted) = format_package_json(source, &options) else {
                continue;
            };
            let Some(edit) = minimal_edit(source, &formatted.output) else {
                continue;
            };
            let found: Vec<String> = formatted.issues.iter().map(skif_edit::FormatIssue::describe).collect();
            // Reordering conditions other than `types` and `default` changes which one matches first.
            let safety = if formatted.issues.iter().all(skif_edit::FormatIssue::is_safe) {
                FixSafety::Safe
            } else {
                FixSafety::Unsafe
            };
            out.emit(
                diagnostic(&META, "`package.json` is not formatted", document_location(package))
                    .package(package.display_name())
                    .package_id(package.id)
                    .help(format!("{}; run `skif format`", found.join("; ")))
                    .fix(Fix::new(
                        "Format `package.json`".to_owned(),
                        safety,
                        vec![FixOp::text(package.manifest.path.clone(), edit)],
                    ))
                    .build(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, assert_fixes_clear, config, lint};

    const ON: &str = "[rules]\npackage-json-format = \"error\"\n";

    fn files(member: &str) -> Vec<(&'static str, String)> {
        vec![
            (
                "package.json",
                r#"{"name":"root","packageManager":"pnpm@9.0.0","private":true,"workspaces":["packages/*"]}"#
                    .to_owned(),
            ),
            ("packages/a/package.json", member.to_owned()),
        ]
    }

    fn borrowed<'a>(files: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
        files.iter().map(|(path, text)| (*path, text.as_str())).collect()
    }

    #[test]
    fn the_defaults_match_the_formatter() {
        assert_eq!(options(&Format::default()), FormatOptions::default());
    }

    #[test]
    fn off_by_default() {
        let files = files(r#"{"version":"1.0.0","name":"a"}"#);
        assert_eq!(lint(META.id, &borrowed(&files), &config("")).len(), 0);
    }

    #[test]
    fn reports_and_fixes_an_unformatted_manifest() {
        let member =
            "{\n  \"version\": \"1.0.0\",\n  \"name\": \"a\",\n  \"scripts\": {\"b\": \"1\", \"a\": \"2\"}\n}\n";
        let files = files(member);
        let files = borrowed(&files);
        let diagnostics = lint(META.id, &files, &config(ON));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].package.as_deref(), Some("a"));
        assert_eq!(
            diagnostics[0].help.as_deref(),
            Some(
                "the top-level keys are not in the configured order; `scripts` is not sorted alphabetically; run `skif format`"
            )
        );
        assert_fixes_clear(META.id, &files, &config(ON));
        let fixed = testing::apply_fixes(&files, &diagnostics);
        let (_, text) = fixed
            .iter()
            .find(|(path, _)| path == "packages/a/package.json")
            .unwrap();
        assert_eq!(
            text,
            "{\n  \"name\": \"a\",\n  \"version\": \"1.0.0\",\n  \"scripts\": {\"a\": \"2\", \"b\": \"1\"}\n}\n"
        );
    }

    #[test]
    fn format_table_options_apply() {
        let toml = format!("{ON}[format]\nsort-first = []\nsort-packages = false\nsort-az = []\nindent = \"\\\\t\"\n");
        let files = files("{\n  \"version\": \"1.0.0\",\n  \"name\": \"a\"\n}\n");
        let files = borrowed(&files);
        let diagnostics = lint(META.id, &files, &config(&toml));
        assert_eq!(
            diagnostics[0].help.as_deref(),
            Some("the layout differs from the configured indent; run `skif format`")
        );
        assert_fixes_clear(META.id, &files, &config(&toml));
    }

    #[test]
    fn a_longer_sort_exports_makes_the_fix_unsafe() {
        let member = r#"{"name":"a","exports":{"import":"./a.mjs","module":"./a.esm.js","types":"./a.d.ts"}}"#;
        let files = files(member);
        let files = borrowed(&files);
        let safe = lint(META.id, &files, &config(ON));
        assert_eq!(safe.len(), 1);
        assert_eq!(safe[0].fixes[0].safety, FixSafety::Safe);
        let toml = format!("{ON}[format]\nsort-exports = [\"types\", \"module\", \"import\", \"default\"]\n");
        let reordered = lint(META.id, &files, &config(&toml));
        assert_eq!(reordered.len(), 1);
        assert_eq!(reordered[0].fixes[0].safety, FixSafety::Unsafe);
    }

    #[test]
    fn ignored_packages_are_skipped() {
        let toml = format!("{ON}[workspace]\nignore-packages = [\"a\"]\n");
        let files = files(r#"{"version":"1.0.0","name":"a"}"#);
        assert_eq!(lint(META.id, &borrowed(&files), &config(&toml)).len(), 0);
    }
}
