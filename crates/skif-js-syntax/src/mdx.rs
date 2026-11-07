//! Static ESM extraction for `.mdx` files.
//!
//! This is not an MDX compiler (ADR 0001 rules out running one): it finds the top-level `import` and `export`
//! paragraphs the way MDX itself delimits them, outside YAML frontmatter and fenced code blocks, and blanks
//! everything else. The result has the byte length and line breaks of the original, so the JavaScript parser's spans
//! point straight into the `.mdx` file. The remaining prose is the markup, scanned for the components and
//! expressions it uses (`<Card/>`, `<ui.Button/>`, `{format(x)}`).

/// The ESM of an MDX document and where its markup lies.
pub(crate) struct Extracted {
    /// The source with every byte that is not ESM replaced by a space; line breaks are kept.
    pub esm: String,
    /// Byte ranges of the markup: prose and JSX outside ESM, frontmatter and fenced code.
    pub markup: Vec<(usize, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Region {
    Markup,
    Esm,
    Skipped,
}

/// One line of `source`: its byte range without the line break, and the range including it.
struct Line {
    start: usize,
    end: usize,
    next: usize,
}

fn lines(source: &str) -> Vec<Line> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < source.len() {
        let next = source[start..].find('\n').map_or(source.len(), |at| start + at + 1);
        let mut end = next;
        if source[start..end].ends_with('\n') {
            end -= 1;
        }
        if source[start..end].ends_with('\r') {
            end -= 1;
        }
        out.push(Line { start, end, next });
        start = next;
    }
    out
}

/// The fence marker that opens a fenced code block on `line` (up to three spaces of indentation, then three or more
/// backticks or tildes): its character and length.
fn fence_open(line: &str) -> Option<(u8, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let marker = *trimmed.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let run = trimmed.bytes().take_while(|&b| b == marker).count();
    (run >= 3).then_some((marker, run))
}

fn fence_closes(line: &str, marker: u8, length: usize) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return false;
    }
    let run = trimmed.bytes().take_while(|&b| b == marker).count();
    run >= length && trimmed[run..].trim().is_empty()
}

/// Whether `line` starts an ESM paragraph: `import` or `export` at the first column, followed by a space, `{` or `*`.
fn starts_esm(line: &str) -> bool {
    ["import", "export"].iter().any(|keyword| {
        line.strip_prefix(keyword)
            .and_then(|rest| rest.bytes().next())
            .is_some_and(|next| next.is_ascii_whitespace() || next == b'{' || next == b'*')
    })
}

/// Open bracket depth after scanning `text`, skipping strings, template literals and comments. A blank line ends an
/// ESM paragraph only at depth zero, so `export const meta = {` may span blank lines.
fn depth_after(text: &str, mut depth: i32) -> i32 {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'"' | b'\'' | b'`') => {
                i += 1;
                while i < bytes.len() && bytes[i] != quote {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i += 1;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    depth
}

/// The end of a YAML frontmatter block at the very start of `source`, if there is one.
fn frontmatter_end(all: &[Line], source: &str) -> Option<usize> {
    let first = all.first()?;
    if &source[first.start..first.end] != "---" {
        return None;
    }
    all.iter()
        .skip(1)
        .position(|line| &source[line.start..line.end] == "---")
        .map(|index| index + 2)
}

/// Splits `source` into ESM, skipped and markup lines.
pub(crate) fn extract(source: &str) -> Extracted {
    let all = lines(source);
    let mut regions = vec![Region::Markup; all.len()];
    let mut index = 0;
    if let Some(end) = frontmatter_end(&all, source) {
        regions[..end].fill(Region::Skipped);
        index = end;
    }
    while index < all.len() {
        let text = &source[all[index].start..all[index].end];
        if let Some((marker, length)) = fence_open(text) {
            regions[index] = Region::Skipped;
            index += 1;
            while index < all.len() {
                regions[index] = Region::Skipped;
                let closes = fence_closes(&source[all[index].start..all[index].end], marker, length);
                index += 1;
                if closes {
                    break;
                }
            }
        } else if starts_esm(text) {
            let mut depth = 0;
            while index < all.len() {
                let line = &source[all[index].start..all[index].end];
                if line.trim().is_empty() && depth <= 0 {
                    break;
                }
                regions[index] = Region::Esm;
                depth = depth_after(line, depth);
                index += 1;
            }
        } else {
            index += 1;
        }
    }

    let mut esm = vec![b' '; source.len()];
    let mut markup: Vec<(usize, usize)> = Vec::new();
    for (line, region) in all.iter().zip(&regions) {
        let bytes = source.as_bytes();
        match region {
            Region::Esm => esm[line.start..line.next].copy_from_slice(&bytes[line.start..line.next]),
            Region::Markup => match markup.last_mut() {
                Some(last) if last.1 == line.start => last.1 = line.next,
                _ => markup.push((line.start, line.next)),
            },
            Region::Skipped => {}
        }
        for (offset, byte) in bytes[line.end..line.next].iter().enumerate() {
            esm[line.end + offset] = *byte;
        }
    }
    // Whole lines are copied, and every other byte is an ASCII space, so the buffer is valid UTF-8.
    let esm = String::from_utf8(esm).unwrap_or_default();
    Extracted { esm, markup }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(source: &str) -> Vec<String> {
        extract(source)
            .esm
            .lines()
            .map(str::trim_end)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn top_level_import_and_export_paragraphs_are_kept() {
        let source = "---\ntitle: import x from 'y'\n---\nimport A from './a'\nimport {\n  B,\n} from './b'\n\n# Title\n\nexport const meta = {\n\n  a: 1,\n}\n\n<A/> import later\n";
        assert_eq!(
            kept(source),
            [
                "import A from './a'",
                "import {",
                "  B,",
                "} from './b'",
                "export const meta = {",
                "  a: 1,",
                "}"
            ]
        );
        assert_eq!(extract(source).esm.len(), source.len());
    }

    #[test]
    fn code_fences_and_indented_lines_are_not_esm() {
        let source = "```js\nimport x from 'fenced'\n```\n~~~~\nimport y from 'tilde'\n~~~~\n  import z from 'indented'\nimportant prose\nexport default Layout\n";
        assert_eq!(kept(source), ["export default Layout"]);
    }

    #[test]
    fn markup_excludes_esm_and_code() {
        let source = "import A from './a'\n\n<A/>\n```\n<B/>\n```\n{c}\n";
        let markup: Vec<&str> = extract(source)
            .markup
            .iter()
            .map(|&(start, end)| &source[start..end])
            .collect();
        assert_eq!(markup, ["\n<A/>\n", "{c}\n"]);
    }

    #[test]
    fn non_ascii_prose_keeps_the_byte_length() {
        let source = "Grüße — ünïcode\nimport A from './a'\n";
        let extracted = extract(source);
        assert_eq!(extracted.esm.len(), source.len());
        assert!(extracted.esm.contains("import A from './a'"));
    }
}
