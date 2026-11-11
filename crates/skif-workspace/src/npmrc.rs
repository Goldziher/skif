//! `.npmrc` text: the `key=value` entries of its top level, as npm's `ini` parser reads them.

/// The `key=value` entries of the top level of the `.npmrc` text `text`, in file order. Each key is trimmed; each
/// value is trimmed and loses one pair of surrounding quotes, or, unquoted, an inline comment that starts at a `#` or
/// `;` after whitespace (`node-linker=hoisted # flat` is `hoisted`). A leading byte order mark, blank lines, comment
/// lines (`#` or `;`) and lines without `=` give no entry, and a `[section]` header ends the top level: `ini` has no
/// way back to it, so nothing after the header is an entry.
pub fn npmrc_entries(text: &str) -> impl Iterator<Item = (&str, &str)> {
    text.strip_prefix('\u{feff}')
        .unwrap_or(text)
        .lines()
        .map(str::trim)
        .take_while(|line| !(line.starts_with('[') && line.ends_with(']')))
        .filter(|line| !line.starts_with(['#', ';']))
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim(), value_of(value)))
}

/// The value of an entry: `raw` trimmed and unquoted, else without its inline comment, then unquoted.
fn value_of(raw: &str) -> &str {
    let raw = raw.trim();
    if let Some(inner) = unquoted(raw) {
        return inner;
    }
    let end = raw
        .char_indices()
        .find(|&(at, c)| matches!(c, '#' | ';') && raw[..at].ends_with(char::is_whitespace))
        .map_or(raw.len(), |(at, _)| at);
    let value = raw[..end].trim_end();
    unquoted(value).unwrap_or(value)
}

/// `value` without one pair of surrounding `"` or `'` quotes, when it has them.
fn unquoted(value: &str) -> Option<&str> {
    ['"', '\'']
        .into_iter()
        .find_map(|quote| value.strip_prefix(quote).and_then(|inner| inner.strip_suffix(quote)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(text: &str) -> Vec<(&str, &str)> {
        npmrc_entries(text).collect()
    }

    #[test]
    fn keys_and_values_are_trimmed_and_unquoted() {
        assert_eq!(
            entries("registry = https://r.dev/\nnode-linker=\"hoisted\"\nname='a b'\nempty=\n"),
            [
                ("registry", "https://r.dev/"),
                ("node-linker", "hoisted"),
                ("name", "a b"),
                ("empty", ""),
            ]
        );
    }

    #[test]
    fn comments_blank_lines_and_lines_without_a_value_give_no_entry() {
        assert_eq!(entries("# a=1\n; b=2\n\n   \nflag\n  # c=3\nd=4\n"), [("d", "4")]);
    }

    #[test]
    fn a_byte_order_mark_is_not_part_of_the_first_key() {
        assert_eq!(entries("\u{feff}node-linker=hoisted\n"), [("node-linker", "hoisted")]);
    }

    #[test]
    fn a_section_header_ends_the_top_level() {
        assert_eq!(entries("a=1\n[section]\nb=2\n[other]\nc=3\n"), [("a", "1")]);
        assert_eq!(entries("[section]\nb=2\n"), Vec::<(&str, &str)>::new());
    }

    #[test]
    fn an_inline_comment_after_whitespace_is_dropped_from_an_unquoted_value() {
        assert_eq!(
            entries("a=hoisted # flat\nb=x ; why\nc=\"q\" # quoted\nd=\"in # side\"\ne=no#comment\nf=x\t#tab\n"),
            [
                ("a", "hoisted"),
                ("b", "x"),
                ("c", "q"),
                ("d", "in # side"),
                ("e", "no#comment"),
                ("f", "x"),
            ]
        );
    }

    #[test]
    fn a_lone_quote_is_kept() {
        assert_eq!(entries("a=\"\nb='x\n"), [("a", "\""), ("b", "'x")]);
    }
}
