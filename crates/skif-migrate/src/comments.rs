//! The comments a tool's configuration file writes next to the elements of its lists, so that the translation can
//! carry the comment on an ignore entry over as its `reason`.
//!
//! JavaScript, TypeScript and JSON with comments are read through the parser (`skif_js_syntax`); YAML is read line
//! by line, for the block style a configuration file is written in. JSON proper has no comments, so a `package.json`
//! key yields none.

use std::path::Path;

use skif_js_syntax::{ElementComment, PathSegment, extract_array_comments};

use crate::Input;

/// The element comments of one configuration file.
#[derive(Debug, Clone, Default)]
pub(crate) struct Comments {
    elements: Vec<ElementComment>,
}

impl Comments {
    /// The comments of `input`, read in the format its extension selects as `skif_js_plugins::load_config` does; a
    /// file that does not parse has none.
    pub(crate) fn read(input: &Input) -> Self {
        let path = Path::new(&input.path);
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase);
        let elements = match extension.as_deref() {
            Some("js" | "cjs" | "mjs" | "ts" | "cts" | "mts" | "jsx" | "tsx") => {
                extract_array_comments(&input.text, path)
            }
            Some("json" | "jsonc" | "json5") => json_like(&input.text),
            Some("yml" | "yaml") => Some(yaml(&input.text)),
            Some("toml") => None,
            _ => json_like(&input.text).or_else(|| Some(yaml(&input.text))),
        };
        Self {
            elements: elements.unwrap_or_default(),
        }
    }

    /// The comment on the string or regular expression `value` of an array whose path `array` accepts.
    pub(crate) fn on_value(&self, value: &str, array: impl Fn(&[PathSegment]) -> bool) -> Option<&str> {
        self.elements
            .iter()
            .find(|comment| comment.value.as_deref() == Some(value) && array(&comment.array))
            .map(|comment| comment.text.as_str())
    }

    /// The comment on the element `index` of the array under the key `key`, wherever that array is.
    pub(crate) fn on_element(&self, key: &str, index: usize) -> Option<&str> {
        self.elements
            .iter()
            .find(|comment| {
                comment.index == index && matches!(comment.array.last(), Some(PathSegment::Key(last)) if last == key)
            })
            .map(|comment| comment.text.as_str())
    }
}

/// JSON with comments, trailing commas and single quotes is a JavaScript expression.
fn json_like(text: &str) -> Option<Vec<ElementComment>> {
    extract_array_comments(&format!("module.exports = {text}"), Path::new("config.js"))
}

/// The comments of the sequence items of a block-style YAML document: the comment lines directly above an item, and
/// for a scalar item the comment after it on its line. Flow collections (`[a, b]`) are not looked into.
fn yaml(text: &str) -> Vec<ElementComment> {
    let mut out = Vec::new();
    // The open mapping keys and sequence items, each with the column it starts at.
    let mut open: Vec<(usize, PathSegment)> = Vec::new();
    let mut above: Vec<String> = Vec::new();
    for line in text.lines() {
        let content = line.trim_start();
        let column = line.len() - content.len();
        if content.is_empty() {
            above.clear();
            continue;
        }
        if let Some(comment) = content.strip_prefix('#') {
            let comment = comment.trim();
            if !comment.is_empty() {
                above.push(comment.to_owned());
            }
            continue;
        }
        let leading = std::mem::take(&mut above);
        if column == 0 && (content.starts_with("---") || content.starts_with("...")) {
            open.clear();
            continue;
        }
        if content == "-" || content.starts_with("- ") {
            let index = close_for_item(&mut open, column);
            let array: Vec<PathSegment> = open.iter().map(|(_, segment)| segment.clone()).collect();
            open.push((column, PathSegment::Index(index)));
            let item = content[1..].trim_start();
            let item_column = column + (content.len() - item.len());
            let (value, trailing) = if item.is_empty() || mapping_key(item).is_some() {
                (None, None)
            } else {
                scalar(item)
            };
            let texts: Vec<String> = leading.into_iter().chain(trailing).collect();
            if !texts.is_empty() {
                out.push(ElementComment::new(array, index, value, texts.join(" ")));
            }
            if let Some((key, _)) = mapping_key(item) {
                open_key(&mut open, item_column, key);
            }
        } else if let Some((key, _)) = mapping_key(content) {
            open_key(&mut open, column, key);
        }
    }
    out
}

/// Closes what a sequence item at `column` ends and returns its index: one past the previous item at that column,
/// or 0 for the first. A key at the same column stays open, since a sequence may be written at its key's column.
fn close_for_item(open: &mut Vec<(usize, PathSegment)>, column: usize) -> usize {
    while open.last().is_some_and(|(start, _)| *start > column) {
        open.pop();
    }
    match open.last() {
        Some((start, PathSegment::Index(previous))) if *start == column => {
            let next = previous + 1;
            open.pop();
            next
        }
        _ => 0,
    }
}

/// Opens the mapping key `key` at `column`, closing what it ends.
fn open_key(open: &mut Vec<(usize, PathSegment)>, column: usize, key: String) {
    while open.last().is_some_and(|(start, _)| *start >= column) {
        open.pop();
    }
    open.push((column, PathSegment::Key(key)));
}

/// The key of a `key: value` line, and the rest after the colon. `None` for anything else.
fn mapping_key(content: &str) -> Option<(String, &str)> {
    let (key, rest) = match content.chars().next()? {
        quote @ ('"' | '\'') => {
            let close = content[1..].find(quote)? + 1;
            (content[1..close].to_owned(), &content[close + 1..])
        }
        '[' | '{' | '#' | '&' | '*' | '!' | '|' | '>' => return None,
        _ => {
            let colon = content
                .match_indices(':')
                .map(|(index, _)| index)
                .find(|index| content[index + 1..].is_empty() || content[index + 1..].starts_with([' ', '\t']))?;
            if content[..colon].contains(" #") {
                return None;
            }
            (content[..colon].trim_end().to_owned(), &content[colon..])
        }
    };
    let rest = rest.strip_prefix(':')?;
    (rest.is_empty() || rest.starts_with([' ', '\t'])).then(|| (key, rest.trim_start()))
}

/// The value of a scalar sequence item (`None` for a flow collection) and the comment after it.
fn scalar(item: &str) -> (Option<String>, Option<String>) {
    let comment = |rest: &str| {
        rest.trim_start()
            .strip_prefix('#')
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    match item.chars().next() {
        Some(quote @ ('"' | '\'')) => match item[1..].find(quote) {
            Some(close) => {
                let inner = &item[1..=close];
                let value = if quote == '"' {
                    inner.replace("\\\"", "\"")
                } else {
                    inner.to_owned()
                };
                (Some(value), comment(&item[close + 2..]))
            }
            None => (None, None),
        },
        Some('[' | '{') => (None, None),
        _ => {
            let hash = item
                .match_indices('#')
                .map(|(index, _)| index)
                .find(|index| item[..*index].ends_with([' ', '\t']));
            match hash {
                Some(hash) => (Some(item[..hash].trim_end().to_owned()), comment(&item[hash..])),
                None => (Some(item.trim_end().to_owned()), None),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &str) -> PathSegment {
        PathSegment::Key(name.to_owned())
    }

    #[test]
    fn yaml_comments_above_and_after_sequence_items() {
        let found = yaml(
            "versionGroups:\n  # react stays on 17 until the design system moves\n  - label: react\n    dependencies:\n      - react # pinned by the host page\n      - \"react-dom\"  # with react\n    isIgnored: true\n\n  # unrelated\n\n  - dependencies: ['lodash']\nsemverGroups:\n- range: ''\n  # the second group\n- range: '^'\n",
        );
        assert_eq!(
            found,
            [
                ElementComment::new(
                    vec![key("versionGroups")],
                    0,
                    None,
                    "react stays on 17 until the design system moves"
                ),
                ElementComment::new(
                    vec![key("versionGroups"), PathSegment::Index(0), key("dependencies")],
                    0,
                    Some("react".to_owned()),
                    "pinned by the host page"
                ),
                ElementComment::new(
                    vec![key("versionGroups"), PathSegment::Index(0), key("dependencies")],
                    1,
                    Some("react-dom".to_owned()),
                    "with react"
                ),
                ElementComment::new(vec![key("semverGroups")], 1, None, "the second group"),
            ]
        );
    }

    #[test]
    fn keys_and_scalars_are_split_where_yaml_splits_them() {
        assert_eq!(mapping_key("label: a: b"), Some(("label".to_owned(), "a: b")));
        assert_eq!(mapping_key("\"a: b\": c"), Some(("a: b".to_owned(), "c")));
        assert_eq!(mapping_key("http://example.com"), None);
        assert_eq!(mapping_key("[a, b]"), None);
        assert_eq!(scalar("a#b"), (Some("a#b".to_owned()), None));
        assert_eq!(scalar("'a # b' # c"), (Some("a # b".to_owned()), Some("c".to_owned())));
        assert_eq!(scalar("[a, b] # c"), (None, None));
    }

    #[test]
    fn the_format_follows_the_extension() {
        let jsonc = Input::new(
            ".syncpackrc.jsonc",
            "{\n  \"versionGroups\": [\n    // kept on the old major\n    { \"dependencies\": [\"react\"], \"isIgnored\": true }\n  ]\n}\n",
        );
        assert_eq!(
            Comments::read(&jsonc).on_element("versionGroups", 0),
            Some("kept on the old major")
        );
        let yaml = Input::new(".syncpackrc", "versionGroups:\n  # kept on the old major\n  - isIgnored: true\n");
        assert_eq!(
            Comments::read(&yaml).on_element("versionGroups", 0),
            Some("kept on the old major")
        );
        let json = Input::new("package.json", "{ \"knip\": { \"ignore\": [\"a\"] } }");
        assert!(Comments::read(&json).elements.is_empty());
    }
}
