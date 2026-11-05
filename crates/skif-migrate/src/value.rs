//! Reading statically evaluated configuration values.

use std::path::Path;

use skif_js_plugins::load_config;
use skif_js_syntax::ConfigValue;

use crate::notes::{NoteClass, Tool};
use crate::state::{Src, State};
use crate::{Input, MigrateError};

/// Reads a file of any supported format statically.
pub(crate) fn read(input: &Input, tool: Tool) -> Result<ConfigValue, MigrateError> {
    load_config(Path::new(&input.path), &input.text).ok_or_else(|| MigrateError::Unreadable {
        path: input.path.clone(),
        tool,
        expected: "JSON, YAML, or a JavaScript or TypeScript module with a static export",
    })
}

/// The `key: value` pairs of an object value, with a note when the object spreads something only known at run time.
/// A value that is not an object yields nothing and a note.
pub(crate) fn entries<'v>(
    state: &mut State<'_>,
    src: &Src,
    path: &str,
    value: &'v ConfigValue,
) -> Vec<(&'v str, &'v ConfigValue)> {
    let Some(pairs) = value.as_object() else {
        let message = if matches!(value, ConfigValue::Unknown) {
            "the value is only known at run time"
        } else {
            "expected an object"
        };
        state.note(src, NoteClass::Human, path, message);
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, inner) in pairs {
        if key == "..." {
            state.note(
                src,
                NoteClass::Human,
                join(path, "..."),
                "the object spreads a value that is only known at run time; keys may be missing from the migration",
            );
        } else {
            out.push((key.as_str(), inner));
        }
    }
    out
}

/// `parent.child`, or `child` at the top.
pub(crate) fn join(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_owned()
    } else {
        format!("{parent}.{child}")
    }
}

/// `parent[index]`.
pub(crate) fn at(parent: &str, index: usize) -> String {
    format!("{parent}[{index}]")
}

/// A string, or an array of strings. Elements that are not static or not strings are noted and skipped. A regular
/// expression literal is written as `/pattern/`.
pub(crate) fn string_list(state: &mut State<'_>, src: &Src, path: &str, value: &ConfigValue) -> Vec<String> {
    let mut out = Vec::new();
    match value {
        ConfigValue::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                if let Some(text) = string_item(state, src, &at(path, index), item) {
                    out.push(text);
                }
            }
        }
        other => {
            if let Some(text) = string_item(state, src, path, other) {
                out.push(text);
            }
        }
    }
    out
}

fn string_item(state: &mut State<'_>, src: &Src, path: &str, value: &ConfigValue) -> Option<String> {
    match value {
        ConfigValue::String(text) => Some(text.clone()),
        ConfigValue::Regex(pattern) => Some(slash_regex(pattern)),
        ConfigValue::Unknown => {
            state.note(src, NoteClass::Human, path, "the value is only known at run time");
            None
        }
        _ => {
            state.note(src, NoteClass::Human, path, "expected a string");
            None
        }
    }
}

/// A boolean; anything else is noted.
pub(crate) fn boolean(state: &mut State<'_>, src: &Src, path: &str, value: &ConfigValue) -> Option<bool> {
    let parsed = value.as_bool();
    if parsed.is_none() {
        let message = if matches!(value, ConfigValue::Unknown) {
            "the value is only known at run time"
        } else {
            "expected true or false"
        };
        state.note(src, NoteClass::Human, path, message);
    }
    parsed
}

/// A string; anything else is noted.
pub(crate) fn string(state: &mut State<'_>, src: &Src, path: &str, value: &ConfigValue) -> Option<String> {
    let parsed = value.as_str().map(str::to_owned);
    if parsed.is_none() {
        let message = if matches!(value, ConfigValue::Unknown) {
            "the value is only known at run time"
        } else {
            "expected a string"
        };
        state.note(src, NoteClass::Human, path, message);
    }
    parsed
}

/// The slash-delimited spelling of a regular expression: `/^@org\/.+$/`.
pub(crate) fn slash_regex(pattern: &str) -> String {
    format!("/{pattern}/")
}

/// Whether knip compiles a string from a configuration file as a regular expression: it contains one of
/// `* + \ ( | { ^ $`.
fn is_regex_like(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c, '*' | '+' | '\\' | '(' | '|' | '{' | '^' | '$'))
}

/// A trailing `*` after plain characters (`@org/*`): written as a glob, which is what it was meant as. knip would
/// compile it as a regular expression that matches a superset.
fn is_trailing_star(text: &str) -> bool {
    text.strip_suffix('*')
        .is_some_and(|rest| !rest.is_empty() && !rest.ends_with('.') && !is_regex_like(rest))
}

/// Turns `^literal` into the glob `literal*` and `^literal$` into `literal`, the two forms with an exact glob
/// equivalent. `None` for anything else.
fn simple_glob(pattern: &str) -> Option<String> {
    let body = pattern.strip_prefix('^')?;
    let (literal, anchored) = body.strip_suffix('$').map_or((body, false), |literal| (literal, true));
    let plain = !literal.is_empty()
        && literal
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '/' | '_' | '-'));
    plain.then(|| {
        if anchored {
            literal.to_owned()
        } else {
            format!("{literal}*")
        }
    })
}

/// One entry of an ignore list (`ignoreDependencies`, `ignoreBinaries`, `ignoreMembers`, `ignoreUnresolved`).
///
/// A regular expression literal, and a string knip would compile as one, become a slash-delimited regex entry. With
/// `glob` set the two simple forms that a glob states exactly are written as globs instead.
pub(crate) fn pattern_entry(value: &ConfigValue, glob: bool) -> Option<String> {
    match value {
        ConfigValue::Regex(pattern) => Some(
            glob.then(|| simple_glob(pattern))
                .flatten()
                .unwrap_or_else(|| slash_regex(pattern)),
        ),
        ConfigValue::String(text) if is_regex_like(text) && !is_trailing_star(text) => Some(
            glob.then(|| simple_glob(text))
                .flatten()
                .unwrap_or_else(|| slash_regex(text)),
        ),
        ConfigValue::String(text) => Some(text.clone()),
        _ => None,
    }
}

/// A list of ignore entries, with the notes of [`string_list`].
pub(crate) fn pattern_list(
    state: &mut State<'_>,
    src: &Src,
    path: &str,
    value: &ConfigValue,
    glob: bool,
) -> Vec<String> {
    let items: Vec<&ConfigValue> = match value {
        ConfigValue::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    let mut out = Vec::new();
    for (index, item) in items.into_iter().enumerate() {
        let item_path = if matches!(value, ConfigValue::Array(_)) {
            at(path, index)
        } else {
            path.to_owned()
        };
        if let Some(entry) = pattern_entry(item, glob) {
            out.push(entry);
        } else {
            let message = if matches!(item, ConfigValue::Unknown) {
                "the value is only known at run time"
            } else {
                "expected a string or a regular expression"
            };
            state.note(src, NoteClass::Human, item_path, message);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> ConfigValue {
        ConfigValue::String(value.to_owned())
    }

    #[test]
    fn plain_strings_stay_plain() {
        assert_eq!(pattern_entry(&text("left-pad"), true).as_deref(), Some("left-pad"));
        assert_eq!(pattern_entry(&text("@org/pkg"), true).as_deref(), Some("@org/pkg"));
    }

    #[test]
    fn a_trailing_star_stays_a_glob() {
        assert_eq!(pattern_entry(&text("@org/*"), true).as_deref(), Some("@org/*"));
        assert_eq!(pattern_entry(&text("@org/.*"), true).as_deref(), Some("/@org/.*/"));
    }

    #[test]
    fn regex_like_strings_become_slash_regexes() {
        assert_eq!(pattern_entry(&text("@org/.+"), false).as_deref(), Some("/@org/.+/"));
        assert_eq!(
            pattern_entry(&ConfigValue::Regex("^@org\\/.+$".to_owned()), true).as_deref(),
            Some("/^@org\\/.+$/")
        );
    }

    #[test]
    fn simple_anchors_become_globs() {
        assert_eq!(pattern_entry(&text("^eslint-"), true).as_deref(), Some("eslint-*"));
        assert_eq!(pattern_entry(&text("^@types/"), true).as_deref(), Some("@types/*"));
        assert_eq!(pattern_entry(&text("^left-pad$"), true).as_deref(), Some("left-pad"));
        assert_eq!(pattern_entry(&text("^eslint-"), false).as_deref(), Some("/^eslint-/"));
        assert_eq!(pattern_entry(&text("^a.b"), true).as_deref(), Some("/^a.b/"));
    }
}
