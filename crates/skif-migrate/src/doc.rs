//! A small ordered TOML document builder. It exists because the output carries comments (provenance, labels) and a
//! fixed section order, neither of which `toml::Table` keeps.

use std::fmt::Write as _;

/// The widest a list is written on one line.
const LINE_WIDTH: usize = 100;

/// Top-level tables in the order they are written.
const ORDER: &[&str] = &[
    "workspace",
    "lint",
    "policy",
    "catalog",
    "update",
    "js",
    "rules",
    "ignore",
    "version-groups",
    "semver-groups",
    "update-groups",
    "custom-types",
    "overrides",
];

/// A TOML value the migration writes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Val {
    Str(String),
    Bool(bool),
    Int(u64),
    List(Vec<Val>),
    Inline(Vec<(String, Val)>),
}

impl Val {
    pub(crate) fn str(text: impl Into<String>) -> Self {
        Self::Str(text.into())
    }

    pub(crate) fn strs<I, S>(items: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::List(items.into_iter().map(|item| Self::Str(item.into())).collect())
    }

    fn render(&self) -> String {
        match self {
            Self::Str(text) => quote(text),
            Self::Bool(value) => value.to_string(),
            Self::Int(value) => value.to_string(),
            Self::List(items) => {
                let items: Vec<String> = items.iter().map(Self::render).collect();
                format!("[{}]", items.join(", "))
            }
            Self::Inline(entries) => {
                if entries.is_empty() {
                    return "{}".to_owned();
                }
                let entries: Vec<String> = entries
                    .iter()
                    .map(|(key, value)| format!("{} = {}", key_text(key), value.render()))
                    .collect();
                format!("{{ {} }}", entries.join(", "))
            }
        }
    }
}

/// A TOML basic string.
fn quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if u32::from(c) < 0x20 || u32::from(c) == 0x7f => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A key, quoted unless it is a bare key.
fn key_text(key: &str) -> String {
    if !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        key.to_owned()
    } else {
        quote(key)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Entry {
    pub key: String,
    pub value: Val,
    /// A comment written after the value on the same line.
    pub comment: Option<String>,
}

/// One `[table]` or `[[array-of-tables]]` element.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Section {
    pub path: Vec<String>,
    pub array: bool,
    pub comments: Vec<String>,
    pub entries: Vec<Entry>,
}

impl Section {
    /// Sets `key`, replacing an earlier value.
    pub(crate) fn set(&mut self, key: &str, value: Val) {
        self.put(key, value, None);
    }

    /// Sets `key` and writes `comment` after the value, replacing an earlier value and comment.
    pub(crate) fn set_commented(&mut self, key: &str, value: Val, comment: String) {
        self.put(key, value, Some(comment));
    }

    fn put(&mut self, key: &str, value: Val, comment: Option<String>) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.key == key) {
            entry.value = value;
            entry.comment = comment;
        } else {
            self.entries.push(Entry {
                key: key.to_owned(),
                value,
                comment,
            });
        }
    }

    /// Appends the strings of `items` that `key` does not hold yet; the first call creates the list.
    pub(crate) fn extend_strs(&mut self, key: &str, items: impl IntoIterator<Item = String>) {
        self.extend_entries(key, items.into_iter().map(|item| (item, None)));
    }

    /// Appends the ignore entries of `items` whose name `key` does not hold yet, each a string or, with a reason, a
    /// `{ name, reason }` table; the first call creates the list. A reason given for a name the list holds as a
    /// plain string is added to it.
    pub(crate) fn extend_entries(&mut self, key: &str, items: impl IntoIterator<Item = (String, Option<String>)>) {
        let position = self.entries.iter().position(|entry| entry.key == key);
        let index = position.unwrap_or_else(|| {
            self.entries.push(Entry {
                key: key.to_owned(),
                value: Val::List(Vec::new()),
                comment: None,
            });
            self.entries.len() - 1
        });
        if let Val::List(list) = &mut self.entries[index].value {
            for (name, reason) in items {
                let held = list.iter().position(|held| match held {
                    Val::Str(text) => *text == name,
                    Val::Inline(fields) => fields.first() == Some(&("name".to_owned(), Val::Str(name.clone()))),
                    _ => false,
                });
                let item = match reason {
                    Some(reason) => Val::Inline(vec![
                        ("name".to_owned(), Val::Str(name)),
                        ("reason".to_owned(), Val::Str(reason)),
                    ]),
                    None => Val::Str(name),
                };
                match held {
                    None => list.push(item),
                    Some(position) if matches!(list[position], Val::Str(_)) && matches!(item, Val::Inline(_)) => {
                        list[position] = item;
                    }
                    Some(_) => {}
                }
            }
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<&Val> {
        self.entries
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| &entry.value)
    }

    fn header(&self) -> String {
        let path: Vec<String> = self.path.iter().map(|part| key_text(part)).collect();
        if self.array {
            format!("[[{}]]", path.join("."))
        } else {
            format!("[{}]", path.join("."))
        }
    }

    pub(crate) fn render(&self) -> String {
        let mut out = String::new();
        for comment in &self.comments {
            let _ = writeln!(out, "# {comment}");
        }
        let _ = writeln!(out, "{}", self.header());
        for entry in &self.entries {
            out.push_str(&render_entry(&entry.key, &entry.value, entry.comment.as_deref()));
        }
        out
    }

    fn is_blank(&self) -> bool {
        self.entries.is_empty() && self.comments.is_empty()
    }
}

fn render_entry(key: &str, value: &Val, comment: Option<&str>) -> String {
    let one_line = format!(
        "{} = {}{}\n",
        key_text(key),
        value.render(),
        comment.map_or_else(String::new, |comment| format!(" # {comment}"))
    );
    let Val::List(items) = value else {
        return one_line;
    };
    if one_line.len() <= LINE_WIDTH || items.is_empty() {
        return one_line;
    }
    let mut out = format!("{} = [\n", key_text(key));
    for item in items {
        let _ = writeln!(out, "  {},", item.render());
    }
    out.push_str("]\n");
    out
}

/// The sections of the file being built, in creation order within each top-level table.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Doc {
    sections: Vec<Section>,
}

impl Doc {
    /// The `[path]` table, created on first use.
    pub(crate) fn table(&mut self, path: &[&str]) -> &mut Section {
        let path: Vec<String> = path.iter().map(|part| (*part).to_owned()).collect();
        let index = self
            .sections
            .iter()
            .position(|section| !section.array && section.path == path)
            .unwrap_or_else(|| {
                self.sections.push(Section {
                    path,
                    array: false,
                    comments: Vec::new(),
                    entries: Vec::new(),
                });
                self.sections.len() - 1
            });
        &mut self.sections[index]
    }

    /// A new `[[path]]` element.
    pub(crate) fn element(&mut self, path: &[&str], comments: Vec<String>) -> &mut Section {
        self.sections.push(Section {
            path: path.iter().map(|part| (*part).to_owned()).collect(),
            array: true,
            comments,
            entries: Vec::new(),
        });
        let last = self.sections.len() - 1;
        &mut self.sections[last]
    }

    /// Applies `edit` to every `[[path]]` element and drops the elements it returns `false` for.
    pub(crate) fn edit_elements(&mut self, path: &[&str], mut edit: impl FnMut(&mut Section) -> bool) {
        self.sections.retain_mut(|section| {
            !(section.array && section.path.iter().map(String::as_str).eq(path.iter().copied())) || edit(section)
        });
    }

    /// The sections that have content, in the order they are written.
    pub(crate) fn ordered(&self) -> Vec<&Section> {
        let mut sections: Vec<&Section> = self.sections.iter().filter(|section| !section.is_blank()).collect();
        sections.sort_by_key(|section| {
            let top = section.path.first().map_or("", String::as_str);
            ORDER.iter().position(|name| *name == top).unwrap_or(ORDER.len())
        });
        sections
    }

    /// The document text: `banner` comment lines, then every section separated by a blank line.
    pub(crate) fn render(&self, banner: &[String]) -> String {
        let mut out = String::new();
        for line in banner {
            if line.is_empty() {
                out.push('\n');
            } else {
                let _ = writeln!(out, "{line}");
            }
        }
        for section in self.ordered() {
            out.push('\n');
            out.push_str(&section.render());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_are_escaped() {
        assert_eq!(quote("a\"b\\c\n"), "\"a\\\"b\\\\c\\n\"");
        assert_eq!(quote("/^@org\\/.+$/"), "\"/^@org\\\\/.+$/\"");
    }

    #[test]
    fn keys_are_quoted_only_when_needed() {
        assert_eq!(key_text("ignore-unresolved"), "ignore-unresolved");
        assert_eq!(key_text("apps/web"), "\"apps/web\"");
        assert_eq!(key_text("."), "\".\"");
    }

    #[test]
    fn long_lists_wrap() {
        let long = Val::strs((0..12).map(|n| format!("package-number-{n}")));
        let text = render_entry("k", &long, None);
        assert!(text.starts_with("k = [\n  \"package-number-0\",\n"));
        assert!(text.ends_with("]\n"));
    }

    #[test]
    fn sections_are_ordered_and_blank_ones_skipped() {
        let mut doc = Doc::default();
        doc.table(&["rules"]).set("a", Val::str("off"));
        doc.table(&["js"]);
        doc.table(&["lint"]).set("fail-on-warnings", Val::Bool(true));
        let text = doc.render(&[]);
        assert_eq!(text, "\n[lint]\nfail-on-warnings = true\n\n[rules]\na = \"off\"\n");
    }

    #[test]
    fn a_comment_follows_its_value() {
        let mut doc = Doc::default();
        doc.table(&["rules"])
            .set_commented("a", Val::str("off"), "why".to_owned());
        doc.table(&["rules"]).set("b", Val::str("error"));
        assert_eq!(doc.render(&[]), "\n[rules]\na = \"off\" # why\nb = \"error\"\n");
    }

    #[test]
    fn entries_with_a_reason_are_tables_and_others_stay_strings() {
        let mut doc = Doc::default();
        let js = doc.table(&["js"]);
        js.extend_entries(
            "ignore-dependencies",
            [
                ("left-pad".to_owned(), None),
                ("some-loader".to_owned(), Some("loaded by the bundler".to_owned())),
            ],
        );
        js.extend_entries(
            "ignore-dependencies",
            [
                ("left-pad".to_owned(), Some("kept for the old build".to_owned())),
                ("some-loader".to_owned(), None),
            ],
        );
        assert_eq!(
            doc.render(&[]),
            "\n[js]\nignore-dependencies = [\n  { name = \"left-pad\", reason = \"kept for the old build\" },\n  { name = \"some-loader\", reason = \"loaded by the bundler\" },\n]\n"
        );
    }

    #[test]
    fn extend_keeps_unique_items() {
        let mut doc = Doc::default();
        let js = doc.table(&["js"]);
        js.extend_strs("ignore", ["a".to_owned(), "b".to_owned()]);
        js.extend_strs("ignore", ["b".to_owned(), "c".to_owned()]);
        assert_eq!(js.get("ignore"), Some(&Val::strs(["a", "b", "c"])));
    }
}
