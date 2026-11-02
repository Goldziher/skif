//! Glob helpers shared by the plugin table and the module graph.

/// Expands every brace group that has an empty alternative, such as `{,src/}app/**`, into separate patterns.
///
/// Plugin patterns use knip's glob dialect, where `{,src/}` means "nothing or `src/`"; `globset` rejects an empty
/// alternative, so such a pattern would silently match nothing. Groups without an empty alternative are left for
/// `globset` to handle.
#[must_use]
pub fn expand_empty_alternatives(pattern: &str) -> Vec<String> {
    let Some((open, close)) = empty_alternative_group(pattern) else {
        return vec![pattern.to_owned()];
    };
    let (head, body, tail) = (&pattern[..open], &pattern[open + 1..close], &pattern[close + 1..]);
    split_alternatives(body)
        .into_iter()
        .flat_map(|alternative| expand_empty_alternatives(&format!("{head}{alternative}{tail}")))
        .collect()
}

/// The byte range of the first top-level `{…}` group that has an empty alternative.
fn empty_alternative_group(pattern: &str) -> Option<(usize, usize)> {
    let bytes = pattern.as_bytes();
    let mut search = 0;
    while let Some(offset) = pattern[search..].find('{') {
        let open = search + offset;
        let mut depth = 0_usize;
        let mut close = None;
        for (index, &byte) in bytes.iter().enumerate().skip(open) {
            match byte {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(index);
                        break;
                    }
                }
                _ => {}
            }
        }
        let close = close?;
        if split_alternatives(&pattern[open + 1..close])
            .iter()
            .any(String::is_empty)
        {
            return Some((open, close));
        }
        search = close + 1;
    }
    None
}

/// Splits a brace body on its top-level commas.
fn split_alternatives(body: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0_usize;
    let mut current = String::new();
    for character in body.chars() {
        match character {
            '{' => {
                depth += 1;
                current.push(character);
            }
            '}' => {
                depth = depth.saturating_sub(1);
                current.push(character);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut current)),
            _ => current.push(character),
        }
    }
    parts.push(current);
    parts
}

#[cfg(test)]
mod tests {
    use super::expand_empty_alternatives;

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
}
