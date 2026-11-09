//! What the translators share: the document being built, the notes, and the rules the old tools ran.

use std::sync::Arc;

use skif_config::Config;

use crate::comments::Comments;
use crate::doc::{Doc, Val};
use crate::keys;
use crate::notes::{Note, NoteClass, Tool};
use crate::{Input, MigrateError, Migration, Options, Profile};

/// Where a translator is reading from, for the notes, with the comments of that file.
#[derive(Debug, Clone)]
pub(crate) struct Src {
    pub tool: Tool,
    pub file: String,
    pub comments: Arc<Comments>,
}

impl Src {
    pub(crate) fn new(tool: Tool, file: impl Into<String>) -> Self {
        Self {
            tool,
            file: file.into(),
            comments: Arc::default(),
        }
    }

    /// The source `input` of `tool`, with the comments it writes next to list elements.
    pub(crate) fn read(tool: Tool, input: &Input) -> Self {
        Self {
            comments: Arc::new(Comments::read(input)),
            ..Self::new(tool, input.path.clone())
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
    include_tags: Vec<String>,
    /// The `off` was added by the `exact` profile, not taken from a tool's configuration.
    profile: bool,
}

/// Rules one tool's configuration turns off, decided at the end because another tool may still check them.
struct DeferredOff {
    src: Src,
    origin: String,
    ids: Vec<String>,
}

pub(crate) struct State<'a> {
    pub options: &'a Options,
    pub doc: Doc,
    pub notes: Vec<Note>,
    pub tools: Vec<Tool>,
    kept: Vec<(String, Tool)>,
    deferred: Vec<DeferredOff>,
    settings: Vec<(String, RuleSetting)>,
}

impl<'a> State<'a> {
    pub(crate) fn new(options: &'a Options) -> Self {
        Self {
            options,
            doc: Doc::default(),
            notes: Vec::new(),
            tools: Vec::new(),
            kept: Vec::new(),
            deferred: Vec::new(),
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

    /// Records that `tool` ran these rules, so the `exact` profile leaves them as they are.
    pub(crate) fn keep<'r>(&mut self, tool: Tool, ids: impl IntoIterator<Item = &'r str>) {
        for id in ids {
            if !self.kept.iter().any(|(known, by)| known == id && *by == tool) {
                self.kept.push((id.to_owned(), tool));
            }
        }
    }

    /// The tools other than `tool` that ran the rule `id`.
    fn kept_by_others(&self, id: &str, tool: Tool) -> Vec<Tool> {
        let mut others: Vec<Tool> = Vec::new();
        for (known, by) in &self.kept {
            if known == id && *by != tool && !others.contains(by) {
                others.push(*by);
            }
        }
        others
    }

    /// Turns `ids` off for a tool whose configuration ignores them, unless another migrated tool still runs them; that
    /// is decided in [`State::finish`], when every tool has been read.
    pub(crate) fn ignore_rules(&mut self, src: &Src, origin: String, ids: Vec<String>) {
        self.deferred.push(DeferredOff {
            src: src.clone(),
            origin,
            ids,
        });
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

    /// Adds `tags` to the `include-tags` option of `id`.
    pub(crate) fn add_include_tags(&mut self, id: &str, tags: impl IntoIterator<Item = String>) {
        let setting = self.setting(id);
        for tag in tags {
            if !setting.include_tags.contains(&tag) {
                setting.include_tags.push(tag);
            }
        }
    }

    fn resolve_ignored_rules(&mut self) {
        for DeferredOff { src, origin, ids } in std::mem::take(&mut self.deferred) {
            let mut turned_off = Vec::new();
            for id in ids {
                let others = self.kept_by_others(&id, src.tool);
                if others.is_empty() {
                    self.set_level(&id, Level::Off);
                    turned_off.push(id);
                } else {
                    let names: Vec<&str> = others.iter().map(|tool| tool.name()).collect();
                    self.note_with(
                        &src,
                        NoteClass::Human,
                        origin.clone(),
                        format!(
                            "{} ignored `{id}`, but {} still checks it, so the rule stays on",
                            src.tool,
                            names.join(" and ")
                        ),
                        format!("set `{id}` to \"off\" in [rules] if it should not run at all"),
                    );
                }
            }
            if !turned_off.is_empty() {
                self.note(
                    &src,
                    NoteClass::Exact,
                    origin,
                    format!("turned off in [rules]: {}", turned_off.join(", ")),
                );
            }
        }
    }

    fn write_rules(&mut self) {
        let marker = profile_marker(&self.tools);
        let rules = self.doc.table(&["rules"]);
        for (id, setting) in &self.settings {
            if setting.profile {
                rules.set_commented(id, Val::str("off"), marker.clone());
                continue;
            }
            let value = if setting.ignore_tags.is_empty() && setting.include_tags.is_empty() {
                Val::str(setting.level.map_or("error", Level::text))
            } else {
                let mut options = Vec::new();
                if let Some(level) = setting.level {
                    options.push(("level".to_owned(), Val::str(level.text())));
                }
                if !setting.ignore_tags.is_empty() {
                    options.push(("ignore-tags".to_owned(), Val::strs(setting.ignore_tags.iter().cloned())));
                }
                if !setting.include_tags.is_empty() {
                    options.push((
                        "include-tags".to_owned(),
                        Val::strs(setting.include_tags.iter().cloned()),
                    ));
                }
                Val::Inline(options)
            };
            rules.set(id, value);
        }
    }

    pub(crate) fn finish(mut self) -> Result<Migration, MigrateError> {
        self.resolve_ignored_rules();
        let exact = self.options.profile == Profile::Exact;
        let mut extra_checks = Vec::new();
        for rule in &self.options.known_rules {
            let configured = self.settings.iter().any(|(known, _)| *known == rule.id);
            if !rule.default_on || configured || self.kept.iter().any(|(known, _)| *known == rule.id) {
                continue;
            }
            extra_checks.push(rule.id.clone());
        }
        if exact {
            for id in &extra_checks {
                let setting = self.setting(id);
                setting.level = Some(Level::Off);
                setting.profile = true;
            }
            if !self.tools.contains(&Tool::Knip) {
                let marker = profile_marker(&self.tools);
                self.doc
                    .table(&["js"])
                    .set_commented("enabled", Val::Bool(false), marker);
            }
        }
        self.write_rules();
        let body = self.doc.render(&[]);
        let toml = format!("{}{body}", banner(&self.tools, self.options.profile));
        let config = validate(&toml)?;
        Ok(Migration {
            toml,
            config,
            notes: self.notes,
            extra_checks,
            profile: self.options.profile,
            tools: self.tools,
            body,
        })
    }
}

/// The comment after a rule the `exact` profile turned off: a later `--merge` replaces such a value when the
/// tool it migrates does check the rule.
fn profile_marker(tools: &[Tool]) -> String {
    let names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
    format!("{}{}", keys::PROFILE_MARKER, names.join(", "))
}

/// The banner lines of a new `skif.toml`, each ending in a newline.
fn banner(tools: &[Tool], profile: Profile) -> String {
    let names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
    let profile = match profile {
        Profile::Exact => "exact",
        Profile::Full => "full",
    };
    format!(
        "#:schema https://raw.githubusercontent.com/Goldziher/skif/main/schema/skif.schema.json\n\n\
         # Migrated from {} by `skif migrate` (profile: {profile}).\n\
         # Review the report the command printed: notes marked H or X were not translated.\n\
         {}\n",
        names.join(", "),
        crate::merge::sources_line(tools),
    )
}

/// Parses `text` as a `skif.toml`.
pub(crate) fn validate(text: &str) -> Result<Config, MigrateError> {
    Config::from_toml_str(text).map_err(|error| MigrateError::Invalid(error.to_string()))
}
