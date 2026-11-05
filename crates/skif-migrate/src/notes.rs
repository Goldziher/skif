//! The report of a migration: one note per construct that was not copied one to one.

use std::fmt::{self, Write as _};

use serde::Serialize;

use crate::Migration;

/// A tool whose configuration can be migrated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tool {
    /// sherif, configured by the `sherif` key of `package.json`.
    Sherif,
    /// syncpack.
    Syncpack,
    /// knip.
    Knip,
}

impl Tool {
    /// Every tool, in the order they are migrated.
    pub const ALL: [Self; 3] = [Self::Sherif, Self::Syncpack, Self::Knip];

    /// The name the tool is invoked by and `--from` accepts.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sherif => "sherif",
            Self::Syncpack => "syncpack",
            Self::Knip => "knip",
        }
    }

    /// The tool called `name`.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tool| tool.name() == name)
    }
}

impl fmt::Display for Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How faithfully a construct was translated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NoteClass {
    /// Translated one to one.
    Exact,
    /// Translated, but skif's behaviour differs in a way the note explains.
    Approximate,
    /// Nothing to translate: skif does this by default or by a flag.
    Default,
    /// Not translated because the value is only known at run time or its meaning is not clear: a human decides.
    Human,
    /// Not translated because skif has no equivalent.
    Unsupported,
}

impl NoteClass {
    /// Whether the note needs a person to look at it.
    #[must_use]
    pub const fn needs_human(self) -> bool {
        matches!(self, Self::Human | Self::Unsupported)
    }

    /// The one-letter class of the migration design: M, A, D, H or X.
    #[must_use]
    pub const fn letter(self) -> char {
        match self {
            Self::Exact => 'M',
            Self::Approximate => 'A',
            Self::Default => 'D',
            Self::Human => 'H',
            Self::Unsupported => 'X',
        }
    }
}

/// One finding of the migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Note {
    /// How the construct was handled.
    pub class: NoteClass,
    /// The tool it came from.
    pub tool: Tool,
    /// The file it was read from.
    pub file: String,
    /// The key path in that file (`versionGroups[2].pinVersion`), or the script and flag (`scripts.knip: --strict`).
    pub path: String,
    /// What was done or why nothing was.
    pub message: String,
    /// What to do about it, when there is something to do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

type Section = (&'static str, fn(NoteClass) -> bool);

pub(crate) fn render_summary(migration: &Migration) -> String {
    let mut out = String::new();
    let sections: [Section; 3] = [
        ("Translated", |class| {
            matches!(class, NoteClass::Exact | NoteClass::Approximate)
        }),
        ("Covered by skif by default", |class| class == NoteClass::Default),
        ("Needs a human", NoteClass::needs_human),
    ];
    for (title, wanted) in sections {
        let notes: Vec<&Note> = migration.notes.iter().filter(|note| wanted(note.class)).collect();
        let _ = writeln!(out, "{title} ({}):", notes.len());
        if notes.is_empty() {
            out.push_str("  none\n");
        }
        for note in notes {
            let _ = writeln!(
                out,
                "  [{}] {} {}: {}",
                note.class.letter(),
                note.tool,
                note.path,
                note.message
            );
            if let Some(suggestion) = &note.suggestion {
                let _ = writeln!(out, "      -> {suggestion}");
            }
        }
    }
    let _ = write!(
        out,
        "Checks skif runs that the old tools did not ({}):",
        migration.extra_checks.len()
    );
    if migration.extra_checks.is_empty() {
        out.push_str(" none\n");
    } else {
        let action = match migration.profile {
            crate::Profile::Exact => "turned off in [rules] by --profile exact",
            crate::Profile::Full => "left on by --profile full",
        };
        let _ = write!(out, " {action}\n  {}\n", migration.extra_checks.join(", "));
    }
    out
}
