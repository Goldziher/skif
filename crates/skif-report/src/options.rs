//! Caller-supplied rendering configuration.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use skif_core::RuleMeta;

use crate::path::relativize;

/// Configuration for every renderer; the caller decides colour, root and sources.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct RenderOptions {
    /// Emit ANSI colour codes (human format only).
    pub color: bool,
    /// Root stripped from absolute paths so that output is relative.
    pub root: Option<PathBuf>,
    /// Version of the tool, reported as `skif` in JSON output.
    pub tool_version: Option<String>,
    rules: Vec<&'static RuleMeta>,
    sources: BTreeMap<String, String>,
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
        self.sources.insert(key, text.into());
        self
    }

    /// Path as displayed: relative to the root, with `/` separators.
    pub(crate) fn display_path(&self, path: &Path) -> String {
        relativize(path, self.root.as_deref())
    }

    /// Rule metadata supplied through [`Self::with_rules`].
    pub(crate) fn rules(&self) -> &[&'static RuleMeta] {
        &self.rules
    }

    /// Source text registered for an already displayed path.
    pub(crate) fn source(&self, display_path: &str) -> Option<&str> {
        self.sources.get(display_path).map(String::as_str)
    }
}
