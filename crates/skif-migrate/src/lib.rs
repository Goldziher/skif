//! One-shot translation of sherif, syncpack and knip configuration into a `skif.toml`.
//!
//! This is a translator, not a compatibility reader: skif keeps reading `skif.toml` only (ADR 0039). The library is
//! pure. The caller reads the files and passes their text in a [`ToolInput`] per tool; [`migrate`] returns the
//! generated TOML, the typed [`Config`] it was re-parsed into, and a [`Note`] for every construct that was not
//! translated one to one. JavaScript and TypeScript configuration files are evaluated statically by
//! `skif-js-syntax`: nothing is executed, and a value that is only known at run time becomes a note that needs a
//! human.
//!
//! The output is built as text so it can carry comments and a fixed section order, then parsed again with
//! [`Config::from_toml_str`] in full; a failure there is a bug in this crate and is reported as
//! [`MigrateError::Invalid`]. Configuration content never reaches that error: a construct that cannot be written
//! becomes a [`Note`].

#![warn(missing_docs)]

mod doc;
mod keys;
mod knip;
mod merge;
mod notes;
mod scripts;
mod sherif;
mod state;
mod syncpack;
mod tables;
mod value;

use std::path::Path;

pub use merge::Merged;
pub use notes::{Note, NoteClass, Tool};
pub use scripts::script_config_path;
use skif_config::Config;
use skif_js_plugins::load_config;
use state::State;

/// Which checks the generated configuration keeps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Profile {
    /// Only the checks the old tools ran: every other rule that skif enables by default is turned off in `[rules]`.
    #[default]
    Exact,
    /// The checks of the old tools plus skif's defaults for everything else.
    Full,
}

/// A rule skif knows, as the caller lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RuleInfo {
    /// The rule id, for example `version-mismatch`.
    pub id: String,
    /// Whether the rule runs when the configuration does not mention it.
    pub default_on: bool,
}

impl RuleInfo {
    /// A rule with the given id, running by default or not.
    #[must_use]
    pub fn new(id: impl Into<String>, default_on: bool) -> Self {
        Self {
            id: id.into(),
            default_on,
        }
    }
}

/// What [`migrate`] needs besides the files.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Options {
    /// Which checks to keep.
    pub profile: Profile,
    /// Every rule of the build the output is for. The `exact` profile turns off the ones the old tools did not run,
    /// and the report lists them. Empty means no rule is added or listed.
    pub known_rules: Vec<RuleInfo>,
    /// Write `[rules] stale-js-config = "error"` for knip's `treatConfigHintsAsErrors`. Without it the rule is
    /// written as `warn`, because skif's hints differ from knip's and `error` could fail a project knip passed.
    pub strict_hints: bool,
}

impl Options {
    /// Options for `profile`, with no known rules and the default handling of configuration hints.
    #[must_use]
    pub fn new(profile: Profile) -> Self {
        Self {
            profile,
            ..Self::default()
        }
    }

    /// Sets the rules of the build the output is for.
    #[must_use]
    pub fn with_known_rules(mut self, known_rules: Vec<RuleInfo>) -> Self {
        self.known_rules = known_rules;
        self
    }

    /// Writes `stale-js-config` as `error` for knip's `treatConfigHintsAsErrors`.
    #[must_use]
    pub const fn with_strict_hints(mut self, strict_hints: bool) -> Self {
        self.strict_hints = strict_hints;
        self
    }
}

/// A file the caller has read.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Input {
    /// The path as the caller wants it shown and as the format is chosen from: `knip.ts`, `.syncpackrc`.
    pub path: String,
    /// The file content.
    pub text: String,
}

impl Input {
    /// A file shown as `path` with the given content.
    #[must_use]
    pub fn new(path: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            text: text.into(),
        }
    }
}

/// The files of one tool.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolInput {
    /// The tool the files configure.
    pub tool: Tool,
    /// The tool's own configuration file, if it has one (syncpack and knip). Without it the `package.json` key is
    /// read.
    pub config: Option<Input>,
    /// The root `package.json`: holds the tool's configuration key and the scripts that run the tool.
    pub package_json: Option<Input>,
    /// The workspace patterns of the repository, used to tell whether a syncpack `source` is the default.
    pub workspace_patterns: Vec<String>,
}

impl ToolInput {
    /// A tool with no files, for a run that only applies the profile.
    #[must_use]
    pub const fn new(tool: Tool) -> Self {
        Self {
            tool,
            config: None,
            package_json: None,
            workspace_patterns: Vec::new(),
        }
    }
}

/// Why a migration could not be produced.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MigrateError {
    /// A file is not valid JSON, YAML, TOML or JavaScript/TypeScript that can be read statically.
    #[error("cannot read {path}: the {tool} configuration is not valid {expected}")]
    Unreadable {
        /// The file.
        path: String,
        /// The tool it configures.
        tool: Tool,
        /// What was expected.
        expected: &'static str,
    },
    /// The generated configuration is not valid `skif.toml`. This is a bug in `skif-migrate`: nothing a user's
    /// configuration says can cause it.
    #[error("the generated configuration is not valid: {0}")]
    Invalid(String),
    /// The existing configuration to merge into is not valid TOML, or the merge is not a valid `skif.toml`.
    #[error("cannot merge into the existing configuration: {0}")]
    Existing(String),
}

/// The result of a migration.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Migration {
    /// The generated `skif.toml`.
    pub toml: String,
    /// The text parsed back into the typed model.
    pub config: Config,
    /// Everything that was translated, approximated, left to skif's defaults or left to a human.
    pub notes: Vec<Note>,
    /// Rules skif runs by default that none of the migrated tools ran. The `exact` profile turns them off.
    pub extra_checks: Vec<String>,
    /// The profile the output was produced with.
    pub profile: Profile,
    /// The tools that were migrated.
    pub tools: Vec<Tool>,
    /// The sections of `toml` without the banner, which `--merge` combines with an existing file.
    body: String,
}

impl Migration {
    /// Whether any construct needs a human (a note of class [`NoteClass::Human`] or [`NoteClass::Unsupported`]).
    #[must_use]
    pub fn needs_human(&self) -> bool {
        self.notes.iter().any(|note| note.class.needs_human())
    }

    /// Combines the generated sections with the text of an existing `skif.toml`.
    ///
    /// The existing file is edited in place, so its comments and order stay. Tables it does not have are appended
    /// after its text under a banner; in a table both files define, the keys it lacks are added, lists are united and
    /// the existing value wins on a conflict. Values the `exact` profile added (marked with a comment) are not
    /// conflicts: a migration of another tool replaces them with the rule it does check, so migrating tools in
    /// several steps ends with the same checks as migrating them in one command. Merging the output of a migration
    /// into itself changes nothing.
    ///
    /// # Errors
    /// [`MigrateError::Existing`] when `existing` is not valid TOML or the merged text is not a valid `skif.toml`.
    pub fn merge_into(&self, existing: &str) -> Result<Merged, MigrateError> {
        let merged = merge::merge_into(self, existing)?;
        state::validate(&merged.toml)
            .map_err(|error| MigrateError::Existing(format!("the merged configuration is not valid: {error}")))?;
        Ok(merged)
    }

    /// The report as one JSON value: `version`, `profile`, `tools`, `translated`, `default`, `needs-human`,
    /// `extra-checks` and `notes`. The caller adds `skif`, `written` and the like.
    #[must_use]
    pub fn report(&self) -> serde_json::Value {
        let list = |wanted: fn(NoteClass) -> bool| pick(&self.notes, wanted);
        serde_json::json!({
            "version": 1,
            "profile": self.profile,
            "tools": self.tools,
            "translated": list(|class| matches!(class, NoteClass::Exact | NoteClass::Approximate)),
            "default": list(|class| class == NoteClass::Default),
            "needs-human": list(NoteClass::needs_human),
            "extra-checks": self.extra_checks,
            "notes": self.notes,
        })
    }

    /// The summary for a terminal: translated constructs, constructs skif covers by default, constructs that need a
    /// human, and the checks skif runs that the old tools did not.
    #[must_use]
    pub fn summary(&self) -> String {
        notes::render_summary(self)
    }
}

fn pick(notes: &[Note], wanted: fn(NoteClass) -> bool) -> Vec<&Note> {
    notes.iter().filter(|note| wanted(note.class)).collect()
}

/// Translates the configuration of the tools in `inputs` into one `skif.toml`.
///
/// # Errors
/// [`MigrateError::Unreadable`] when a configuration file cannot be read statically, and
/// [`MigrateError::Invalid`] when the generated text is not a valid `skif.toml`.
pub fn migrate(inputs: &[ToolInput], options: &Options) -> Result<Migration, MigrateError> {
    let mut state = State::new(options);
    for input in inputs {
        state.tools.push(input.tool);
        match input.tool {
            Tool::Sherif => sherif::translate(input, &mut state)?,
            Tool::Syncpack => syncpack::translate(input, &mut state)?,
            Tool::Knip => knip::translate(input, &mut state)?,
        }
    }
    state.finish()
}

/// The configuration files `tool` reads, in the order the tool looks for them. Sherif has none: its configuration
/// is the `sherif` key of `package.json`.
#[must_use]
pub const fn config_file_names(tool: Tool) -> &'static [&'static str] {
    match tool {
        Tool::Sherif => &[],
        Tool::Syncpack => keys::SYNCPACK_FILES,
        Tool::Knip => keys::KNIP_FILES,
    }
}

/// The skif rule id of the sherif rule `name` (`multiple-dependency-versions` is `version-mismatch`), or `None` when
/// sherif has no rule of that name.
#[must_use]
pub fn sherif_rule_id(name: &str) -> Option<&'static str> {
    tables::sherif_rule(name)
}

/// Whether the root `package.json` text configures or runs `tool`: it has the tool's key, or a script runs it.
/// Text that is not JSON counts as no.
#[must_use]
pub fn package_json_mentions(tool: Tool, package_json: &str) -> bool {
    let Some(value) = load_config(Path::new("package.json"), package_json) else {
        return false;
    };
    scripts::has_key(tool, &value) || !scripts::invocations(&value, tool).is_empty()
}

/// The skif rules whose fixes knip's `--fix-type <name>` stands for, or `None` when `name` is not a knip issue type.
///
/// Each knip issue type maps to the rules that report the same findings. `dependencies` also covers
/// `devDependencies` and `optionalPeerDependencies`, because knip's dependency fixer removes all three.
#[must_use]
pub fn knip_fix_type_rules(name: &str) -> Option<Vec<&'static str>> {
    let types = tables::knip_types();
    let rules_of = |wanted: &str| {
        types
            .iter()
            .find(|knip_type| knip_type.name == wanted)
            .map(|knip_type| knip_type.rules.clone())
    };
    let mut rules = rules_of(name)?;
    if name == "dependencies" {
        for extra in ["devDependencies", "optionalPeerDependencies"] {
            rules.extend(rules_of(extra).unwrap_or_default());
        }
    }
    Some(rules)
}

/// Every knip issue type name, in the order of skif's table.
#[must_use]
pub fn knip_issue_types() -> Vec<&'static str> {
    tables::knip_types()
        .into_iter()
        .map(|knip_type| knip_type.name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knip_fix_types_map_to_rules() {
        assert_eq!(knip_fix_type_rules("enumMembers"), Some(vec!["unused-enum-members"]));
        assert_eq!(
            knip_fix_type_rules("dependencies"),
            Some(vec![
                "unused-dependencies",
                "unused-dev-dependencies",
                "unused-optional-peer-dependencies"
            ])
        );
        assert_eq!(knip_fix_type_rules("unused-files"), None);
        assert!(knip_issue_types().contains(&"catalog"));
    }

    #[test]
    fn a_package_json_that_runs_the_tool_mentions_it() {
        let text = r#"{ "scripts": { "check": "pnpm sherif --fail-on-warnings && echo done" } }"#;
        assert!(package_json_mentions(Tool::Sherif, text));
        assert!(!package_json_mentions(Tool::Knip, text));
    }

    #[test]
    fn a_key_counts_as_a_mention() {
        assert!(package_json_mentions(
            Tool::Knip,
            r#"{ "knip": { "entry": ["a.ts"] } }"#
        ));
        assert!(package_json_mentions(
            Tool::Syncpack,
            r#"{ "config": { "syncpack": { "strict": true } } }"#
        ));
        assert!(!package_json_mentions(Tool::Knip, "not json"));
    }
}
