use std::path::{Path, PathBuf};

use skif_core::{Category, Fixability, LineIndex, Location, RuleMeta, Severity, Span, diagnostic, slash};
use skif_lint::{Emitter, LintContext, Rule};
use skif_workspace::WorkspaceError;

/// Metadata of `invalid-workspace-file`.
pub static META: RuleMeta = RuleMeta::new(
    "invalid-workspace-file",
    Category::Workspace,
    Some(Severity::Error),
    Fixability::None,
    "A member `package.json`, a workspace pattern or `.yarnrc.yml` cannot be read, so part of the workspace is not linted.",
);

/// Reports what discovery could not load: a member `package.json` that is not valid JSON or cannot be read (the
/// package is left out of the run), a workspace pattern that cannot be used (it selects nothing) and a `.yarnrc.yml`
/// that is not valid YAML (its catalogs are not read).
///
/// Discovery collects these in [`Workspace::manifest_errors`](skif_workspace::Workspace::manifest_errors) instead of
/// failing, so the rest of the workspace is still linted; without this rule the run would pass on a workspace it
/// only partly read.
#[derive(Debug, Clone, Copy, Default)]
pub struct InvalidWorkspaceFile;

impl Rule for InvalidWorkspaceFile {
    fn meta(&self) -> &'static RuleMeta {
        &META
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        let workspace = cx.workspace;
        let mut patterns_seen: Vec<&str> = Vec::new();
        for error in &workspace.manifest_errors {
            let (message, help, location) = match error {
                WorkspaceError::InvalidManifest {
                    path,
                    message,
                    line,
                    column,
                } => (
                    format!("`{}` is not valid JSON: {message}", slash(path)),
                    "fix the syntax error; until then the package is left out of the run".to_owned(),
                    Location::new(path.clone(), point(cx, path, *line, *column)),
                ),
                WorkspaceError::InvalidPattern { pattern, message } => {
                    // The same pattern written twice fails twice: pair each error with the next fact of that pattern.
                    let nth = patterns_seen.iter().filter(|seen| **seen == pattern.as_str()).count();
                    patterns_seen.push(pattern.as_str());
                    let fact = workspace
                        .facts
                        .patterns
                        .iter()
                        .filter(|fact| fact.pattern == *pattern)
                        .nth(nth);
                    let location = fact.map_or_else(
                        || Location::new(workspace.root_package().manifest.path.clone(), None),
                        |fact| Location::new(fact.file.clone(), fact.span),
                    );
                    (
                        format!("workspace pattern `{pattern}` cannot be used: {message}"),
                        "fix or remove the pattern; until then it selects no package".to_owned(),
                        location,
                    )
                }
                WorkspaceError::InvalidYarnrc { path, message } => (
                    format!("`{}` is not valid YAML: {message}", slash(path)),
                    "fix the syntax error; until then its catalogs are not read".to_owned(),
                    Location::new(path.clone(), None),
                ),
                WorkspaceError::Io { path, source } => {
                    let path = relative(&workspace.root, path);
                    (
                        format!("`{}` cannot be read: {source}", slash(&path)),
                        "make the file readable; until then the package is left out of the run".to_owned(),
                        Location::new(path, None),
                    )
                }
                other => (
                    other.to_string(),
                    "fix the file; until then part of the workspace is not linted".to_owned(),
                    Location::new(workspace.root_package().manifest.path.clone(), None),
                ),
            };
            out.emit(diagnostic(&META, message, location).help(help).build());
        }
    }
}

/// `path` relative to the workspace root, when it is below it.
fn relative(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

/// The span of the character at the 1-based `line` and `column` (counted in Unicode scalar values after a leading
/// byte order mark, which takes no column, as [`LineIndex::line_col`] counts them) of the file at the
/// workspace-relative `path`; `None` when the file cannot be read again.
fn point(cx: &LintContext<'_>, path: &Path, line: u32, column: u32) -> Option<Span> {
    let source = cx.fs().read_to_string(&cx.workspace.root.join(path)).ok()?;
    let start = offset_at(&source, line, column)?;
    let end = source[start..].chars().next().map_or(start, |c| start + c.len_utf8());
    let index = LineIndex::new(&source);
    Some(index.span(u32::try_from(start).ok()?, u32::try_from(end).ok()?))
}

/// The byte offset of the 1-based `line` and `column` in `source`, where a leading byte order mark takes no column;
/// the end of the line when the column is past it.
fn offset_at(source: &str, line: u32, column: u32) -> Option<usize> {
    let line_start = match line.checked_sub(2) {
        None => source.strip_prefix('\u{feff}').map_or(0, |rest| source.len() - rest.len()),
        Some(newlines) => source.match_indices('\n').nth(usize::try_from(newlines).ok()?)?.0 + 1,
    };
    let rest = &source[line_start..];
    let line_text = rest.split('\n').next().unwrap_or_default();
    let skip = usize::try_from(column.saturating_sub(1)).ok()?;
    Some(
        line_start
            + line_text
                .char_indices()
                .nth(skip)
                .map_or(line_text.len(), |(index, _)| index),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{config, lint, located, root};

    #[test]
    fn an_invalid_member_manifest_is_reported_at_the_syntax_error() {
        let root_json = root("");
        let broken = r#"{"name":"e", "version": "1.0.0",, }"#;
        let files = [
            ("package.json", root_json.as_str()),
            (
                "packages/a/package.json",
                r#"{"name":"a","dependencies":{"e":"workspace:*"}}"#,
            ),
            ("packages/e/package.json", broken),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        let diagnostic = &diagnostics[0];
        assert_eq!(diagnostic.severity, Severity::Error);
        assert_eq!(diagnostic.location.path, Path::new("packages/e/package.json"));
        assert!(
            diagnostic
                .message
                .starts_with("`packages/e/package.json` is not valid JSON: "),
            "{}",
            diagnostic.message
        );
        // The error is on one of the two commas after the version; either way it is located, on line 1.
        assert_eq!(located(&files, diagnostic), ",");
        assert_eq!(diagnostic.location.span.unwrap().start_line, 1);
    }

    #[test]
    fn a_syntax_error_after_a_byte_order_mark_is_located_on_the_same_character() {
        let root_json = root("");
        let broken = r#"{"name":"e"x}"#;
        let with_bom = format!("\u{feff}{broken}");
        let locate = |manifest: &str| {
            let files = [
                ("package.json", root_json.as_str()),
                ("packages/e/package.json", manifest),
            ];
            let diagnostics = lint(META.id, &files, &config(""));
            assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
            let span = diagnostics[0].location.span.unwrap();
            (located(&files, &diagnostics[0]).to_owned(), span.start_line, span.start_col)
        };
        let plain = locate(broken);
        assert!(!plain.0.is_empty(), "{plain:?}");
        assert_eq!(locate(&with_bom), plain);
    }

    #[test]
    fn an_unusable_workspace_pattern_is_reported_where_it_is_written() {
        let manifest =
            r#"{"name":"root","private":true,"packageManager":"pnpm@9.0.0","workspaces":["packages/*","../x"]}"#;
        let files = [
            ("package.json", manifest),
            ("packages/a/package.json", r#"{"name":"a"}"#),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(diagnostics[0].location.path, Path::new("package.json"));
        assert!(
            diagnostics[0]
                .message
                .starts_with("workspace pattern `../x` cannot be used: "),
            "{}",
            diagnostics[0].message
        );
        assert_eq!(located(&files, &diagnostics[0]), r#""../x""#);
    }

    #[test]
    fn an_invalid_yarnrc_is_reported() {
        let manifest = r#"{"name":"r","private":true,"packageManager":"yarn@4.10.0","workspaces":["a"]}"#;
        let files = [
            ("package.json", manifest),
            (".yarnrc.yml", "catalog: [unclosed\n"),
            ("a/package.json", r#"{"name":"a"}"#),
        ];
        let diagnostics = lint(META.id, &files, &config(""));
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert_eq!(diagnostics[0].location.path, Path::new(".yarnrc.yml"));
        assert!(
            diagnostics[0].message.starts_with("`.yarnrc.yml` is not valid YAML: "),
            "{}",
            diagnostics[0].message
        );
    }

    #[test]
    fn a_workspace_discovery_read_in_full_reports_nothing() {
        let root_json = root("");
        let files = [
            ("package.json", root_json.as_str()),
            ("packages/a/package.json", r#"{"name":"a"}"#),
        ];
        assert_eq!(lint(META.id, &files, &config("")), []);
    }

    #[test]
    fn offsets_count_characters_from_the_start_of_the_line() {
        let source = "{\n  \"ä\": 1,,\n}";
        assert_eq!(offset_at(source, 1, 1), Some(0));
        assert_eq!(offset_at(source, 2, 3), Some(4));
        assert_eq!(&source[offset_at(source, 2, 9).unwrap()..][..1], ",");
        assert_eq!(offset_at(source, 2, 99), Some(source.find(",,").unwrap() + 2));
        assert_eq!(offset_at(source, 9, 1), None);
    }
}
