//! The options both toolchain rules take: which kinds of file they read, more files by glob, and files they skip.

use serde::Deserialize;
use skif_config::{IgnoreItem, Selector};
use skif_lint::{LintContext, OptionsError, parse_options};

/// A kind of toolchain file, as named in the `kinds` option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolchainKind {
    /// `.nvmrc`, `.node-version`, `.bun-version`, `.tool-versions`, `mise.toml` and `.mise.toml`.
    VersionFiles,
    /// `Dockerfile`, `Dockerfile.*`, `*.Dockerfile`, `*.dockerfile` and `Containerfile`.
    Dockerfiles,
    /// `.github/workflows/*.yml` and `*.yaml`.
    Workflows,
    /// `*.sh`.
    ShellScripts,
}

impl ToolchainKind {
    /// The name the `kinds` option uses.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::VersionFiles => "version-files",
            Self::Dockerfiles => "dockerfiles",
            Self::Workflows => "workflows",
            Self::ShellScripts => "shell-scripts",
        }
    }
}

/// Options of `toolchain-pin-drift` and `foreign-package-manager-command`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct ToolchainOptions {
    /// The kinds of file the rule reads at their usual places; `None` keeps the rule's default.
    pub kinds: Option<Vec<ToolchainKind>>,
    /// More files to read, as globs relative to the workspace root (`docker/**`, `ci/*.bash`), whatever their kind.
    pub files: Vec<String>,
    /// Files, as globs relative to the workspace root, the rule reports nothing in; each entry may say why.
    pub ignore_files: Vec<IgnoreItem>,
}

/// [`ToolchainOptions`] with the globs compiled, as a rule applies them.
#[derive(Debug, Clone)]
pub(crate) struct Resolved {
    pub kinds: Vec<ToolchainKind>,
    pub files: Selector,
    pub ignore_files: Selector,
}

impl Resolved {
    /// Whether the rule reports in the file at `path` (workspace-relative, `/` separators).
    pub fn reads(&self, path: &str, kind: ToolchainKind, at_default_place: bool) -> bool {
        let read = (at_default_place && self.kinds.contains(&kind)) || self.files.matches(path, false);
        read && !self.ignore_files.matches(path, false)
    }
}

fn selector(key: &str, patterns: Vec<String>) -> Result<Selector, OptionsError> {
    Selector::new(patterns).map_err(|error| OptionsError::for_key(key, error.to_string()))
}

impl ToolchainOptions {
    /// Parses and checks the raw options of a rule.
    pub(crate) fn parse(options: &toml::Table) -> Result<Self, OptionsError> {
        let parsed = parse_options::<Self>(options)?;
        parsed.resolve(&[])?;
        Ok(parsed)
    }

    /// The options of rule `id` in `cx`, or the defaults when they are missing or invalid (the registry rejects
    /// invalid options before a run starts).
    pub(crate) fn of(cx: &LintContext<'_>, id: &str) -> Self {
        cx.config
            .rule(id)
            .and_then(|rule| Self::parse(&rule.options).ok())
            .unwrap_or_default()
    }

    /// Compiles the globs; `defaults` are the kinds read when `kinds` is not set.
    pub(crate) fn resolve(&self, defaults: &[ToolchainKind]) -> Result<Resolved, OptionsError> {
        Ok(Resolved {
            kinds: self.kinds.clone().unwrap_or_else(|| defaults.to_vec()),
            files: selector("files", self.files.clone())?,
            ignore_files: selector(
                "ignore-files",
                self.ignore_files.iter().map(|item| item.name.clone()).collect(),
            )?,
        })
    }
}

/// JSON Schema of [`ToolchainOptions`], with `defaults` as the default of `kinds`.
pub(crate) fn schema(defaults: &[ToolchainKind]) -> serde_json::Value {
    let kinds = ["version-files", "dockerfiles", "workflows", "shell-scripts"];
    let defaults: Vec<&str> = defaults.iter().map(|kind| kind.name()).collect();
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "kinds": {
                "type": "array",
                "items": { "enum": kinds },
                "default": defaults,
            },
            "files": { "type": "array", "items": { "type": "string" }, "default": [] },
            "ignore-files": {
                "type": "array",
                "default": [],
                "items": {
                    "anyOf": [
                        { "type": "string" },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["name"],
                            "properties": { "name": { "type": "string" }, "reason": { "type": "string" } },
                        },
                    ],
                },
            },
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(toml: &str) -> toml::Table {
        toml.parse().unwrap()
    }

    #[test]
    fn options_parse_with_reasons_and_reject_unknown_keys_and_bad_globs() {
        let parsed = ToolchainOptions::parse(&table(
            "kinds = [\"dockerfiles\"]\nfiles = [\"ci/**\"]\nignore-files = [\"a.sh\", { name = \"b/**\", reason = \"kept on npm\" }]\n",
        ))
        .unwrap();
        assert_eq!(parsed.kinds, Some(vec![ToolchainKind::Dockerfiles]));
        assert_eq!(parsed.files, ["ci/**"]);
        assert_eq!(parsed.ignore_files[1].reason.as_deref(), Some("kept on npm"));

        let unknown = ToolchainOptions::parse(&table("kind = []\n")).unwrap_err();
        assert_eq!(unknown.key(), Some("kind"));
        let bad_kind = ToolchainOptions::parse(&table("kinds = [\"makefiles\"]\n")).unwrap_err();
        assert_eq!(bad_kind.key(), Some("kinds"));
        let bad_glob = ToolchainOptions::parse(&table("files = [\"a/[\"]\n")).unwrap_err();
        assert_eq!(bad_glob.key(), Some("files"));
    }

    #[test]
    fn a_file_is_read_at_its_default_place_by_kind_or_by_glob_and_skipped_by_ignore() {
        let options = ToolchainOptions::parse(&table(
            "files = [\"ci/**\"]\nignore-files = [{ name = \"ci/old.sh\", reason = \"release branch fallback\" }]\n",
        ))
        .unwrap()
        .resolve(&[ToolchainKind::Dockerfiles])
        .unwrap();
        assert!(options.reads("Dockerfile", ToolchainKind::Dockerfiles, true));
        assert!(!options.reads("deploy.sh", ToolchainKind::ShellScripts, true));
        assert!(options.reads("ci/deploy.sh", ToolchainKind::ShellScripts, false));
        assert!(!options.reads("ci/old.sh", ToolchainKind::ShellScripts, false));
        assert!(!options.reads("docker/Dockerfile", ToolchainKind::Dockerfiles, false));
    }
}
