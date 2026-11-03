//! Glob helpers shared by the plugin table and the module graph.

/// The most variants one pattern expands to; past it the remaining groups are left as they are.
const MAX_VARIANTS: usize = 64;

/// Expands every brace group that has an empty alternative, such as `{,src/}app/**`, into separate patterns.
///
/// Plugin patterns use knip's glob dialect, where `{,src/}` means "nothing or `src/`"; `globset` rejects an empty
/// alternative, so such a pattern would silently match nothing. Groups without an empty alternative are left for
/// `globset` to handle. Groups nest, a brace inside a character class (`[{]`) or after a backslash is a literal, an
/// unbalanced brace is left as it is, and a pattern expands to at most about 64 variants.
#[must_use]
pub fn expand_empty_alternatives(pattern: &str) -> Vec<String> {
    expand(pattern, |alternatives| alternatives.iter().any(String::is_empty))
}

/// Expands every brace group that has an empty alternative or an alternative containing `/`, so that each result
/// can be matched one path segment at a time (`{src/app,lib}/*.ts` becomes `src/app/*.ts` and `lib/*.ts`).
#[must_use]
pub fn expand_path_alternatives(pattern: &str) -> Vec<String> {
    expand(pattern, |alternatives| {
        alternatives
            .iter()
            .any(|alternative| alternative.is_empty() || alternative.contains('/'))
    })
}

fn expand(pattern: &str, wanted: fn(&[String]) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    expand_into(pattern, wanted, &mut out);
    out
}

fn expand_into(pattern: &str, wanted: fn(&[String]) -> bool, out: &mut Vec<String>) {
    if out.len() >= MAX_VARIANTS {
        out.push(pattern.to_owned());
        return;
    }
    let found = brace_groups(pattern).into_iter().find_map(|(open, close)| {
        let alternatives = split_alternatives(&pattern[open + 1..close]);
        wanted(&alternatives).then_some((open, close, alternatives))
    });
    let Some((open, close, alternatives)) = found else {
        out.push(pattern.to_owned());
        return;
    };
    let (head, tail) = (&pattern[..open], &pattern[close + 1..]);
    for alternative in alternatives {
        expand_into(&format!("{head}{alternative}{tail}"), wanted, out);
    }
}

/// For each byte of `pattern`, whether it is a literal that cannot open or close a group: a backslash and the byte
/// it escapes, and everything in a character class (`[{]`).
fn literal_mask(pattern: &str) -> Vec<bool> {
    let bytes = pattern.as_bytes();
    let mut literal = vec![false; bytes.len()];
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'\\' => {
                for slot in literal.iter_mut().skip(index).take(2) {
                    *slot = true;
                }
                index += 2;
            }
            b'[' => match class_end(bytes, index) {
                Some(end) => {
                    for slot in &mut literal[index..=end] {
                        *slot = true;
                    }
                    index = end + 1;
                }
                None => index += 1,
            },
            _ => index += 1,
        }
    }
    literal
}

/// The index of the `]` that closes the character class opened at `open`.
fn class_end(bytes: &[u8], open: usize) -> Option<usize> {
    let mut index = open + 1;
    if matches!(bytes.get(index), Some(b'!' | b'^')) {
        index += 1;
    }
    if bytes.get(index) == Some(&b']') {
        index += 1;
    }
    while index < bytes.len() {
        match bytes[index] {
            b']' => return Some(index),
            b'\\' => index += 2,
            _ => index += 1,
        }
    }
    None
}

/// The byte ranges of every balanced `{…}` group, outermost and leftmost first.
fn brace_groups(pattern: &str) -> Vec<(usize, usize)> {
    let literal = literal_mask(pattern);
    let mut open = Vec::new();
    let mut groups = Vec::new();
    for (index, &byte) in pattern.as_bytes().iter().enumerate() {
        if literal[index] {
            continue;
        }
        match byte {
            b'{' => open.push(index),
            b'}' => {
                if let Some(start) = open.pop() {
                    groups.push((start, index));
                }
            }
            _ => {}
        }
    }
    groups.sort_unstable();
    groups
}

/// Splits a brace body on its top-level commas.
fn split_alternatives(body: &str) -> Vec<String> {
    let literal = literal_mask(body);
    let mut parts = Vec::new();
    let mut depth = 0_usize;
    let mut start = 0;
    for (index, &byte) in body.as_bytes().iter().enumerate() {
        if literal[index] {
            continue;
        }
        match byte {
            b'{' => depth += 1,
            b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                parts.push(body[start..index].to_owned());
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(body[start..].to_owned());
    parts
}

#[cfg(test)]
mod tests {
    use super::{expand_empty_alternatives, expand_path_alternatives};

    #[test]
    fn empty_alternatives_expand_and_others_stay() {
        let cases: [(&str, &[&str]); 4] = [
            ("app/**/page.tsx", &["app/**/page.tsx"]),
            ("{,src/}app/*.{js,ts}", &["app/*.{js,ts}", "src/app/*.{js,ts}"]),
            (
                "{,src/}app/{,[(]*[)]/}x.ts",
                &["app/x.ts", "app/[(]*[)]/x.ts", "src/app/x.ts", "src/app/[(]*[)]/x.ts"],
            ),
            ("a/{b,c}/d", &["a/{b,c}/d"]),
        ];
        for (pattern, expected) in cases {
            assert_eq!(expand_empty_alternatives(pattern), expected, "{pattern}");
        }
    }

    #[test]
    fn nested_groups_are_searched_for_empty_alternatives() {
        assert_eq!(
            expand_empty_alternatives("x/{a,{b,}}.ts"),
            ["x/{a,b}.ts", "x/a.ts", "x/.ts"]
        );
        assert_eq!(
            expand_empty_alternatives("{src,{,lib/}pkg}/i.ts"),
            ["{src,pkg}/i.ts", "{src,lib/pkg}/i.ts"]
        );
    }

    #[test]
    fn unbalanced_braces_are_left_alone_and_do_not_hide_later_groups() {
        assert_eq!(expand_empty_alternatives("{a,b"), ["{a,b"]);
        assert_eq!(expand_empty_alternatives("a}b/{,c/}d"), ["a}b/d", "a}b/c/d"]);
        assert_eq!(expand_empty_alternatives("{x/{,c/}d"), ["{x/d", "{x/c/d"]);
    }

    #[test]
    fn escaped_and_class_braces_are_literals() {
        assert_eq!(expand_empty_alternatives("a[{]/{,b/}c"), ["a[{]/c", "a[{]/b/c"]);
        assert_eq!(expand_empty_alternatives(r"a\{/{,b/}c"), [r"a\{/c", r"a\{/b/c"]);
        assert_eq!(expand_empty_alternatives("[{,]/x"), ["[{,]/x"]);
        assert_eq!(expand_empty_alternatives(r"{a\,,b}"), [r"{a\,,b}"]);
    }

    #[test]
    fn expansion_is_capped() {
        let pattern = "{,a}".repeat(12);
        let variants = expand_empty_alternatives(&pattern);
        assert!(variants.len() < 100, "{}", variants.len());
        assert!(variants.len() >= 64);
    }

    #[test]
    fn slash_alternatives_expand_for_segment_matching() {
        assert_eq!(
            expand_path_alternatives("{src/app,lib}/*.ts"),
            ["src/app/*.ts", "lib/*.ts"]
        );
        assert_eq!(expand_path_alternatives("a/*.{ts,tsx}"), ["a/*.{ts,tsx}"]);
        assert_eq!(expand_path_alternatives("{a,{b/c,d}}/x"), ["a/x", "b/c/x", "d/x"]);
    }
}
