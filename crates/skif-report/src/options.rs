//! Caller-supplied rendering configuration.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use skif_core::RuleMeta;

use crate::codeowners::CodeOwners;
use crate::path::{collapse_dots, is_absolute_text, relativize};
use crate::text::{Source, utf16_column};

/// Configuration for every renderer; the caller decides colour, root and sources. Built with the `with_*` methods.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct RenderOptions {
    /// Emit ANSI colour codes (human format only).
    pub(crate) color: bool,
    /// Root stripped from absolute paths so that output is relative.
    root: Option<PathBuf>,
    /// Version of the tool, reported as `skif` in JSON output; the version of this crate when unset.
    pub(crate) tool_version: Option<String>,
    /// The repository top-level, when the workspace root is below it; GitHub annotations are relative to it.
    repo_root: Option<PathBuf>,
    rules: Vec<&'static RuleMeta>,
    sources: BTreeMap<String, Source>,
    codeowners: Option<CodeOwners>,
}

impl RenderOptions {
    /// Enables or disables ANSI colour.
    #[must_use]
    pub const fn with_color(mut self, color: bool) -> Self {
        self.color = color;
        self
    }

    /// Sets the root used to relativise absolute paths.
    #[must_use]
    pub fn with_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.root = Some(root.into());
        self
    }

    /// Sets the repository top-level that GitHub annotation paths are relative to (`file=` must be repo-relative).
    #[must_use]
    pub fn with_repo_root(mut self, repo_root: impl Into<PathBuf>) -> Self {
        self.repo_root = Some(repo_root.into());
        self
    }

    /// Sets the tool version reported in JSON output.
    #[must_use]
    pub fn with_tool_version(mut self, version: impl Into<String>) -> Self {
        self.tool_version = Some(version.into());
        self
    }

    /// Sets the rule metadata listed by the SARIF renderer.
    #[must_use]
    pub fn with_rules(mut self, rules: impl IntoIterator<Item = &'static RuleMeta>) -> Self {
        self.rules = rules.into_iter().collect();
        self
    }

    /// Registers the text of a file so that the human format can show a code frame for it.
    ///
    /// The report does not carry source text; without this, frames are skipped.
    #[must_use]
    pub fn with_source(mut self, path: impl AsRef<Path>, text: impl Into<String>) -> Self {
        let key = relativize(path.as_ref(), self.root.as_deref());
        self.sources.insert(key, Source::new(text.into()));
        self
    }

    /// Sets the CODEOWNERS entries the codeowners format groups diagnostics by.
    #[must_use]
    pub fn with_codeowners(mut self, codeowners: CodeOwners) -> Self {
        self.codeowners = Some(codeowners);
        self
    }

    /// The CODEOWNERS entries supplied through [`Self::with_codeowners`].
    pub(crate) const fn codeowners(&self) -> Option<&CodeOwners> {
        self.codeowners.as_ref()
    }

    /// Path as displayed: relative to the root, with `/` separators.
    pub(crate) fn display_path(&self, path: &Path) -> String {
        relativize(path, self.root.as_deref())
    }

    /// Path as displayed in GitHub annotations: relative to the repository top-level when one is set and contains
    /// the root, otherwise as [`Self::display_path`].
    pub(crate) fn repo_path(&self, path: &Path) -> String {
        let shown = self.display_path(path);
        match (&self.root, &self.repo_root) {
            (Some(root), Some(repo)) if !is_absolute_text(&shown) => {
                let prefix = relativize(root, Some(repo));
                if is_absolute_text(&prefix) || prefix.is_empty() || prefix == "." {
                    shown
                } else {
                    collapse_dots(&format!("{prefix}/{shown}"))
                }
            }
            _ => shown,
        }
    }

    /// The column in UTF-16 code units for a column counted in scalar values, when the text of the file is known.
    pub(crate) fn utf16_column(&self, display_path: &str, line: u32, column: u32) -> Option<u32> {
        let line = self.source_line(display_path, line)?;
        Some(utf16_column(line, column))
    }

    /// Rule metadata supplied through [`Self::with_rules`].
    pub(crate) fn rules(&self) -> &[&'static RuleMeta] {
        &self.rules
    }

    /// The 1-based `line` of the source registered for an already displayed path, without its line terminator.
    pub(crate) fn source_line(&self, display_path: &str, line: u32) -> Option<&str> {
        self.sources.get(display_path)?.line(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotation_paths_of_files_above_the_root_are_normalised() {
        let options = RenderOptions::default().with_root("/repo/web").with_repo_root("/repo");
        assert_eq!(options.repo_path(Path::new("../skif.toml")), "skif.toml");
        assert_eq!(options.repo_path(Path::new("src/a.ts")), "web/src/a.ts");
    }
}
