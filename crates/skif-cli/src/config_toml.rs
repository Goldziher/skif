//! A configuration rendered as TOML with a comment on every value that some file set.

use std::fmt::Write as _;

use skif::join_key;
use toml::{Table, Value};

/// Asked for the comment text of a dotted origin key and item; `None` writes no comment.
type Origin<'a> = Option<&'a dyn Fn(&str, Item) -> Option<String>>;

/// What a comment is asked about.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Item {
    /// A `key = value` line.
    Value,
    /// A `[table]` or `[[entry]]` header.
    Header,
}

/// Renders `table` as TOML: the plain values of a table first, then its sub-tables, then its arrays of tables.
///
/// `origin` is asked for the comment text of each value and header, given the dotted origin key (`lint.max-level`,
/// `version-groups[0]`, `js.plugins."a.b"`); `None` writes no comment. The inside of an array-of-tables entry has no
/// origins of its own, so it is written without asking.
pub fn render(table: &Table, origin: &dyn Fn(&str, Item) -> Option<String>) -> String {
    let mut out = String::new();
    emit(&mut out, "", "", table, Some(origin));
    out
}

fn toml_key(key: &str) -> String {
    let bare = !key.is_empty() && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if bare {
        key.to_owned()
    } else {
        Value::String(key.to_owned()).to_string()
    }
}

fn is_entries(value: &Value) -> bool {
    matches!(value, Value::Array(items) if !items.is_empty() && items.iter().all(Value::is_table))
}

fn comment(out: &mut String, origin: Origin<'_>, key: &str, item: Item) {
    if let Some(text) = origin.and_then(|origin| origin(key, item)) {
        out.push_str("  # from ");
        out.push_str(&text);
    }
}

fn separate(out: &mut String) {
    if !out.is_empty() && !out.ends_with("\n\n") {
        out.push('\n');
    }
}

fn emit(out: &mut String, key: &str, header: &str, table: &Table, origin: Origin<'_>) {
    for (name, value) in table {
        if !value.is_table() && !is_entries(value) {
            let _ = write!(out, "{} = {value}", toml_key(name));
            comment(out, origin, &join_key(key, name), Item::Value);
            out.push('\n');
        }
    }
    for (name, value) in table {
        let child_key = join_key(key, name);
        let child_header = if header.is_empty() {
            toml_key(name)
        } else {
            format!("{header}.{}", toml_key(name))
        };
        match value {
            Value::Table(child) => {
                let mut line = String::new();
                comment(&mut line, origin, &child_key, Item::Header);
                let plain = child.values().any(|v| !v.is_table() && !is_entries(v));
                if plain || child.is_empty() || !line.is_empty() {
                    separate(out);
                    let _ = writeln!(out, "[{child_header}]{line}");
                }
                emit(out, &child_key, &child_header, child, origin);
            }
            Value::Array(items) if is_entries(value) => {
                for (index, item) in items.iter().enumerate() {
                    separate(out);
                    let _ = write!(out, "[[{child_header}]]");
                    comment(out, origin, &format!("{child_key}[{index}]"), Item::Header);
                    out.push('\n');
                    if let Value::Table(entry) = item {
                        emit(out, &child_key, &child_header, entry, None);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Table {
        text.parse().expect("test toml")
    }

    fn at(key: &str, item: Item) -> Option<String> {
        match (key, item) {
            ("lint.max-level", Item::Value) => Some("a.toml:3".to_owned()),
            ("js.plugins.\"a.b\"", Item::Header) => Some("b.toml:7".to_owned()),
            ("ignore[1]", Item::Header) => Some("c.toml:9".to_owned()),
            _ => None,
        }
    }

    #[test]
    fn values_headers_and_entries_carry_their_origin() {
        let table = parse(
            "top = 1\n[lint]\nmax-level = \"warn\"\nfail-on-warnings = true\n[js.plugins.\"a.b\"]\nenabled = true\n[[ignore]]\nrule = \"x\"\n[[ignore]]\nrule = \"y\"\n",
        );
        let text = render(&table, &at);
        assert_eq!(
            text,
            "top = 1\n\n[lint]\nmax-level = \"warn\"  # from a.toml:3\nfail-on-warnings = true\n\n[js.plugins.\"a.b\"]  # from b.toml:7\nenabled = true\n\n[[ignore]]\nrule = \"x\"\n\n[[ignore]]  # from c.toml:9\nrule = \"y\"\n"
        );
        assert_eq!(parse(&text), table, "the output stays valid TOML with the same content");
    }

    #[test]
    fn a_table_of_only_sub_tables_has_no_header_of_its_own() {
        let table = parse("[a.b]\nx = 1\n");
        assert_eq!(render(&table, &|_, _| None), "[a.b]\nx = 1\n");
    }

    #[test]
    fn keys_that_are_not_bare_are_quoted() {
        let table = parse("\"a b\" = 1\n");
        assert_eq!(render(&table, &|_, _| None), "\"a b\" = 1\n");
    }
}
