//! Script extraction for single-file components (`.vue`, `.svelte`, `.astro`).
//!
//! This is a textual scan, not an HTML parser: it finds `<script …>…</script>` pairs (and the Astro `---` frontmatter)
//! and returns their byte ranges so spans map straight back into the original file. Template expressions are not
//! analysed, so imports used only from a template look unused to the caller.

/// A script region of a component file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ScriptBlock {
    pub start: usize,
    pub end: usize,
    pub typescript: bool,
    pub jsx: bool,
}

pub(crate) fn script_blocks(source: &str, astro: bool) -> Vec<ScriptBlock> {
    let mut blocks = Vec::new();
    let mut cursor = 0;
    if astro && let Some(frontmatter) = astro_frontmatter(source) {
        cursor = frontmatter.end;
        blocks.push(frontmatter);
    }
    while let Some(found) = source[cursor..].find("<script") {
        let tag_start = cursor + found;
        let after_name = tag_start + "<script".len();
        let boundary_ok = source[after_name..]
            .chars()
            .next()
            .is_some_and(|c| c.is_whitespace() || c == '>' || c == '/');
        if !boundary_ok {
            cursor = after_name;
            continue;
        }
        let Some(tag_end) = tag_close(source, after_name) else { break };
        let attrs = &source[after_name..tag_end];
        let self_closing = attrs.trim_end().ends_with('/');
        let body_start = tag_end + 1;
        if self_closing {
            cursor = body_start;
            continue;
        }
        let Some(close) = source[body_start..].find("</script") else { break };
        let body_end = body_start + close;
        let lang = attribute(attrs, "lang").unwrap_or_default();
        blocks.push(ScriptBlock {
            start: body_start,
            end: body_end,
            typescript: astro || matches!(lang.as_str(), "ts" | "tsx"),
            jsx: lang == "tsx" || lang == "jsx",
        });
        cursor = body_end;
    }
    blocks
}

/// Index of the `>` closing a start tag, skipping quoted attribute values.
fn tag_close(source: &str, from: usize) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (i, c) in source[from..].char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '>') => return Some(from + i),
            (None, _) => {}
        }
    }
    None
}

fn attribute(attrs: &str, wanted: &str) -> Option<String> {
    let mut rest = attrs;
    while let Some(i) = rest.find(wanted) {
        let preceded_ok = rest[..i].chars().next_back().is_none_or(char::is_whitespace);
        let after = rest[i + wanted.len()..].trim_start();
        if preceded_ok && let Some(value) = after.strip_prefix('=') {
            let value = value.trim_start();
            let (quote, body) = match value.chars().next() {
                Some(q @ ('"' | '\'')) => (Some(q), &value[1..]),
                _ => (None, value),
            };
            let end = match quote {
                Some(q) => body.find(q).unwrap_or(body.len()),
                None => body.find(|c: char| c.is_whitespace() || c == '>').unwrap_or(body.len()),
            };
            return Some(body[..end].to_string());
        }
        rest = &rest[i + wanted.len()..];
    }
    None
}

fn astro_frontmatter(source: &str) -> Option<ScriptBlock> {
    let lead = source.len() - source.trim_start().len();
    let rest = &source[lead..];
    let after_fence = rest.strip_prefix("---")?;
    let body_start = lead + 3;
    let close = after_fence.find("\n---")?;
    Some(ScriptBlock {
        start: body_start,
        end: body_start + close + 1,
        typescript: true,
        jsx: false,
    })
}

#[cfg(test)]
mod tests {
    use super::script_blocks;

    #[test]
    fn finds_script_bodies() {
        let cases: &[(&str, bool, &[&str])] = &[
            ("<template/><script>let a</script>", false, &["let a"]),
            ("<script lang=\"ts\" setup>let b</script><script>let c</script>", false, &["let b", "let c"]),
            ("<script src=\"./x.js\"/>", false, &[]),
            ("<scripty>no</scripty>", false, &[]),
            ("---\nimport a from 'a'\n---\n<div/>", true, &["\nimport a from 'a'\n"]),
            ("<script attr=\"a>b\">x</script>", false, &["x"]),
        ];
        for (src, astro, expected) in cases {
            let got: Vec<&str> = script_blocks(src, *astro).iter().map(|b| &src[b.start..b.end]).collect();
            assert_eq!(&got, expected, "{src}");
        }
    }

    #[test]
    fn detects_typescript() {
        let blocks = script_blocks("<script lang='ts'>x</script><script>y</script>", false);
        assert_eq!(blocks.iter().map(|b| b.typescript).collect::<Vec<_>>(), [true, false]);
    }
}
