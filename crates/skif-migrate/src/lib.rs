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
pub enum Profile {
    /// Only the checks the old tools ran: every other rule that skif enables by default is turned off in `[rules]`.
    #[default]
    Exact,
    /// The checks of the old tools plus skif's defaults for everything else.
    Full,
}

/// A rule skif knows, as the caller lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleInfo {
    /// The rule id, for example `version-mismatch`.
    pub id: String,
    /// Whether the rule runs when the configuration does not mention it.
    pub default_on: bool,
}

/// What [`migrate`] needs besides the files.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Which checks to keep.
    pub profile: Profile,
    /// Every rule of the build the output is for. The `exact` profile turns off the ones the old tools did not run,
    /// and the report lists them. Empty means no rule is added or listed.
    pub known_rules: Vec<RuleInfo>,
}

/// A file the caller has read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    /// The path as the caller wants it shown and as the format is chosen from: `knip.ts`, `.syncpackrc`.
    pub path: String,
    /// The file content.
    pub text: String,
}

/// The files of one tool.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Whether the root `package.json` text configures or runs `tool`: it has the tool's key, or a script runs it.
/// Text that is not JSON counts as no.
#[must_use]
pub fn package_json_mentions(tool: Tool, package_json: &str) -> bool {
    let Some(value) = load_config(Path::new("package.json"), package_json) else {
        return false;
    };
    scripts::has_key(tool, &value) || !scripts::invocations(&value, tool).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

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
