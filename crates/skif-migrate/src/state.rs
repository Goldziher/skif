//! What the translators share: the document being built, the notes, and the rules the old tools ran.

use std::collections::BTreeSet;

use skif_config::Config;
use toml::Table;

use crate::doc::{Doc, Val};
use crate::keys;
use crate::notes::{Note, NoteClass, Tool};
use crate::{MigrateError, Migration, Options, Profile};

/// Where a translator is reading from, for the notes.
#[derive(Debug, Clone)]
pub(crate) struct Src {
    pub tool: Tool,
    pub file: String,
}

impl Src {
    pub(crate) fn new(tool: Tool, file: impl Into<String>) -> Self {
        Self {
            tool,
            file: file.into(),
        }
    }
}

/// The level of a rule in `[rules]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Level {
    Off,
    Warn,
    Error,
}

impl Level {
    const fn text(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Default)]
struct RuleSetting {
    level: Option<Level>,
    ignore_tags: Vec<String>,
}

pub(crate) struct State<'a> {
    pub options: &'a Options,
    pub doc: Doc,
    pub notes: Vec<Note>,
    pub tools: Vec<Tool>,
    kept: BTreeSet<String>,
    settings: Vec<(String, RuleSetting)>,
}

impl<'a> State<'a> {
    pub(crate) fn new(options: &'a Options) -> Self {
        Self {
            options,
            doc: Doc::default(),
            notes: Vec::new(),
            tools: Vec::new(),
            kept: BTreeSet::new(),
            settings: Vec::new(),
        }
    }

    pub(crate) fn note(&mut self, src: &Src, class: NoteClass, path: impl Into<String>, message: impl Into<String>) {
        self.push_note(src, class, path.into(), message.into(), None);
    }

    pub(crate) fn note_with(
        &mut self,
        src: &Src,
        class: NoteClass,
        path: impl Into<String>,
        message: impl Into<String>,
        suggestion: impl Into<String>,
    ) {
        self.push_note(src, class, path.into(), message.into(), Some(suggestion.into()));
    }

    fn push_note(&mut self, src: &Src, class: NoteClass, path: String, message: String, suggestion: Option<String>) {
        self.notes.push(Note {
            class,
            tool: src.tool,
            file: src.file.clone(),
            path,
            message,
            suggestion,
        });
    }

    /// Records that the old tool ran these rules, so the `exact` profile leaves them as they are.
    pub(crate) fn keep<'r>(&mut self, ids: impl IntoIterator<Item = &'r str>) {
        self.kept.extend(ids.into_iter().map(str::to_owned));
    }

    fn setting(&mut self, id: &str) -> &mut RuleSetting {
        let index = self
            .settings
            .iter()
            .position(|(known, _)| known == id)
            .unwrap_or_else(|| {
                self.settings.push((id.to_owned(), RuleSetting::default()));
                self.settings.len() - 1
            });
        &mut self.settings[index].1
    }

    /// Sets the level of `id` in `[rules]`. The first level written for a rule stays, except that `off` always wins.
    pub(crate) fn set_level(&mut self, id: &str, level: Level) {
        let setting = self.setting(id);
        if setting.level.is_none() || level == Level::Off {
            setting.level = Some(level);
        }
    }

    /// Adds `tags` to the `ignore-tags` option of `id`.
    pub(crate) fn add_ignore_tags(&mut self, id: &str, tags: impl IntoIterator<Item = String>) {
        let setting = self.setting(id);
        for tag in tags {
            if !setting.ignore_tags.contains(&tag) {
                setting.ignore_tags.push(tag);
            }
        }
    }

    fn is_off(&self, id: &str) -> bool {
        self.settings
            .iter()
            .any(|(known, setting)| known == id && setting.level == Some(Level::Off))
    }

    fn write_rules(&mut self) {
        let rules = self.doc.table(&["rules"]);
        for (id, setting) in &self.settings {
            let value = if setting.ignore_tags.is_empty() {
                Val::str(setting.level.map_or("error", Level::text))
            } else {
                let mut options = Vec::new();
                if let Some(level) = setting.level {
                    options.push(("level".to_owned(), Val::str(level.text())));
                }
                options.push(("ignore-tags".to_owned(), Val::strs(setting.ignore_tags.iter().cloned())));
                Val::Inline(options)
            };
            rules.set(id, value);
        }
    }

    pub(crate) fn finish(mut self) -> Result<Migration, MigrateError> {
        let exact = self.options.profile == Profile::Exact;
        let mut extra_checks = Vec::new();
        let mut turned_off = Vec::new();
        for rule in &self.options.known_rules {
            let configured = self.settings.iter().any(|(known, _)| *known == rule.id);
            if !rule.default_on || self.kept.contains(&rule.id) || self.is_off(&rule.id) {
                continue;
            }
            extra_checks.push(rule.id.clone());
            if exact && !configured {
                turned_off.push(rule.id.clone());
            }
        }
        for id in turned_off {
            self.set_level(&id, Level::Off);
        }
        if exact && !self.tools.contains(&Tool::Knip) {
            self.doc.table(&["js"]).set("enabled", Val::Bool(false));
        }
        self.write_rules();
        let banner = banner(&self.tools, self.options.profile);
        let toml = self.doc.render(&banner);
        let config = validate(&toml)?;
        Ok(Migration {
            toml,
            config,
            notes: self.notes,
            extra_checks,
            profile: self.options.profile,
            tools: self.tools,
            doc: self.doc,
        })
    }
}

fn banner(tools: &[Tool], profile: Profile) -> Vec<String> {
    let names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
    let profile = match profile {
        Profile::Exact => "exact",
        Profile::Full => "full",
    };
    vec![
        "#:schema https://raw.githubusercontent.com/Goldziher/skif/main/schema/skif.schema.json".to_owned(),
        String::new(),
        format!(
            "# Migrated from {} by `skif migrate` (profile: {profile}).",
            names.join(", ")
        ),
        "# Review the report the command printed: notes marked H or X were not translated.".to_owned(),
    ]
}

/// Parses `text` as a `skif.toml`. Keys the configuration model does not have yet are removed first when the full
/// text is rejected, so the rest is still checked.
pub(crate) fn validate(text: &str) -> Result<Config, MigrateError> {
    let table: Table = text
        .parse()
        .map_err(|error: toml::de::Error| MigrateError::Invalid(error.to_string()))?;
    match Config::from_toml_str(text) {
        Ok(config) => Ok(config),
        Err(_) => {
            let view = keys::validation_view(&table);
            let stripped = toml::to_string(&view).map_err(|error| MigrateError::Invalid(error.to_string()))?;
            Config::from_toml_str(&stripped).map_err(|error| MigrateError::Invalid(error.to_string()))
        }
    }
}
