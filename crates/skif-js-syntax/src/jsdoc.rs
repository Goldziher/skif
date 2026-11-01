//! `JSDoc` tag extraction by text, so it does not depend on the parser's comment attachment.

/// Tag names (without `@`) of the `JSDoc` blocks that end directly before `start`, nearest block first.
pub(crate) fn leading_tags(source: &str, start: usize) -> Vec<String> {
    let mut tags = Vec::new();
    let mut end = start.min(source.len());
    while source.is_char_boundary(end) {
        let before = source[..end].trim_end();
        if !before.ends_with("*/") {
            break;
        }
        let Some(open) = before.rfind("/*") else { break };
        let block = &before[open..];
        if block.starts_with("/**") {
            parse_tags(block, &mut tags);
        }
        end = open;
    }
    tags
}

fn parse_tags(block: &str, tags: &mut Vec<String>) {
    let inner = block.trim_start_matches("/**").trim_end_matches("*/");
    for line in inner.lines() {
        let line = line.trim().trim_start_matches('*').trim_start();
        let Some(rest) = line.strip_prefix('@') else { continue };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            .collect();
        if !name.is_empty() && !tags.contains(&name) {
            tags.push(name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::leading_tags;

    #[test]
    fn extracts_tags_from_blocks_above() {
        let cases: &[(&str, &[&str])] = &[
            ("/** @public */\nexport const a = 1;", &["public"]),
            (
                "/**\n * Docs.\n * @internal\n * @alias foo\n */\nexport const a = 1;",
                &["internal", "alias"],
            ),
            ("/* @public */\nexport const a = 1;", &[]),
            ("const x = 1;\nexport const a = 1;", &[]),
            ("/** see {@link x} */\nexport const a = 1;", &[]),
            ("/** @public */\n/** @beta */\nexport const a = 1;", &["beta", "public"]),
        ];
        for (src, expected) in cases {
            let start = src.find("export").unwrap_or(0);
            assert_eq!(leading_tags(src, start), *expected, "{src}");
        }
    }
}
