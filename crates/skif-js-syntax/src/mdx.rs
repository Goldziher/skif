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

/// What the scan of an ESM paragraph is inside of.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Frame {
    /// A `(`, `[` or `{` of code, and the `${` of a template literal or the `{` of a JSX attribute or child.
    Bracket,
    /// The text of a template literal.
    Template,
    /// A JSX tag between its `<` and `>`; `closing` for `</a>`.
    Tag { closing: bool },
    /// The children of a JSX element: text, in which quotes, slashes and backticks mean nothing.
    Children,
}

/// The words after which an expression starts, so that `return <p/>` opens JSX.
const EXPRESSION_KEYWORDS: [&str; 15] = [
    "await",
    "case",
    "default",
    "delete",
    "do",
    "else",
    "in",
    "instanceof",
    "new",
    "of",
    "return",
    "throw",
    "typeof",
    "void",
    "yield",
];

/// The lexical state of an ESM paragraph between lines: the brackets, template literals and JSX elements and the block
/// comment it is inside of. A blank line ends the paragraph only outside all of them, so `export const meta = {`, a
/// multi-line template literal or JSX element may span blank lines. JSX text is told from code by where a `<` stands,
/// so `<a>http://x</a>` and `<p>don't</p>` hold no comment and no string.
struct Scan<'a> {
    /// What the scan is inside of, innermost last; empty at the top level of the paragraph.
    frames: Vec<Frame>,
    /// Whether an expression may start here in the code, so a `<` opens JSX and a `/` a regular expression, rather
    /// than comparing or dividing.
    expression_start: bool,
    in_block_comment: bool,
    /// The source offset of the `/*` that opened the block comment the scan is in.
    comment_start: usize,
    /// Source offsets of `/*` that do not open a comment (see [`esm_paragraph_end`]).
    not_comments: &'a [usize],
}

impl<'a> Scan<'a> {
    const fn new(not_comments: &'a [usize]) -> Self {
        Self {
            frames: Vec::new(),
            expression_start: true,
            in_block_comment: false,
            comment_start: 0,
            not_comments,
        }
    }

    fn is_open(&self) -> bool {
        !self.frames.is_empty() || self.in_block_comment
    }

    /// Scans one line, which starts at `offset` in the source.
    fn line(&mut self, text: &str, offset: usize) {
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            i = if self.in_block_comment {
                match bytes[i..].windows(2).position(|pair| pair == b"*/") {
                    Some(at) => {
                        self.in_block_comment = false;
                        i + at + 2
                    }
                    None => return,
                }
            } else {
                match self.frames.last().copied() {
                    Some(Frame::Template) => self.template(bytes, i),
                    Some(Frame::Tag { closing }) => self.tag(bytes, i, offset, closing),
                    Some(Frame::Children) => self.children(bytes, i),
                    Some(Frame::Bracket) | None => match self.code(bytes, i, offset) {
                        Some(next) => next,
                        None => return,
                    },
                }
            };
        }
    }

    /// Opens a block comment at `i` unless the paragraph scan ruled that `/*` out; returns whether it did.
    fn block_comment(&mut self, bytes: &[u8], i: usize, offset: usize) -> bool {
        let opens = bytes.get(i + 1) == Some(&b'*') && !self.not_comments.contains(&(offset + i));
        if opens {
            self.in_block_comment = true;
            self.comment_start = offset + i;
        }
        opens
    }

    /// Scans the code at `i` and returns where the scan goes on, or `None` at a `//` comment, which ends the line. A
    /// `'` or `"` that does not close on its line is skipped alone, as JSX text the scan did not recognise.
    fn code(&mut self, bytes: &[u8], i: usize, offset: usize) -> Option<usize> {
        let byte = bytes[i];
        if is_ident(byte) {
            let end = i + bytes[i..].iter().take_while(|&&b| is_ident(b)).count();
            let member = i > 0 && bytes[i - 1] == b'.';
            self.expression_start = !member && EXPRESSION_KEYWORDS.iter().any(|word| word.as_bytes() == &bytes[i..end]);
            return Some(end);
        }
        if byte.is_ascii_whitespace() {
            return Some(i + 1);
        }
        let mut next = i + 1;
        let mut expression_start = true;
        match byte {
            b'"' | b'\'' => {
                next = string_end(bytes, i).unwrap_or(next);
                expression_start = false;
            }
            b'`' => self.frames.push(Frame::Template),
            b'/' if bytes.get(i + 1) == Some(&b'/') => return None,
            b'/' if self.block_comment(bytes, i, offset) => return Some(i + 2),
            b'/' if self.expression_start && bytes.get(i + 1) != Some(&b'*') => {
                if let Some(end) = regex_end(bytes, i) {
                    next = end;
                    expression_start = false;
                }
            }
            b'(' | b'[' | b'{' => self.frames.push(Frame::Bracket),
            b')' | b']' | b'}' => {
                if self.frames.last() == Some(&Frame::Bracket) {
                    self.frames.pop();
                }
                expression_start = false;
            }
            b'<' if self.expression_start && bytes.get(i + 1).is_some_and(|&b| b == b'>' || is_ident(b)) => {
                self.frames.push(Frame::Tag { closing: false });
            }
            _ if !byte.is_ascii() => expression_start = false,
            _ => {}
        }
        self.expression_start = expression_start;
        Some(next)
    }

    fn template(&mut self, bytes: &[u8], i: usize) -> usize {
        match bytes[i] {
            b'\\' => return i + 2,
            b'`' => {
                self.frames.pop();
                self.expression_start = false;
            }
            b'$' if bytes.get(i + 1) == Some(&b'{') => {
                self.frames.push(Frame::Bracket);
                self.expression_start = true;
                return i + 2;
            }
            _ => {}
        }
        i + 1
    }

    fn tag(&mut self, bytes: &[u8], i: usize, offset: usize, closing: bool) -> usize {
        match bytes[i] {
            b'"' | b'\'' => return string_end(bytes, i).unwrap_or(i + 1),
            b'{' => {
                self.frames.push(Frame::Bracket);
                self.expression_start = true;
            }
            b'/' if bytes.get(i + 1) == Some(&b'>') => {
                self.frames.pop();
                self.expression_start = false;
                return i + 2;
            }
            b'/' if self.block_comment(bytes, i, offset) => return i + 2,
            b'>' => {
                self.frames.pop();
                if !closing {
                    self.frames.push(Frame::Children);
                } else if self.frames.last() == Some(&Frame::Children) {
                    self.frames.pop();
                }
                self.expression_start = false;
            }
            _ => {}
        }
        i + 1
    }

    fn children(&mut self, bytes: &[u8], i: usize) -> usize {
        match bytes[i] {
            b'{' => {
                self.frames.push(Frame::Bracket);
                self.expression_start = true;
            }
            b'<' if bytes.get(i + 1) == Some(&b'/') => {
                self.frames.push(Frame::Tag { closing: true });
                return i + 2;
            }
            b'<' => self.frames.push(Frame::Tag { closing: false }),
            _ => {}
        }
        i + 1
    }
}

const fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

/// The end of the `'` or `"` string that starts at `i`, past its closing quote, or `None` when it does not close on
/// its line.
fn string_end(bytes: &[u8], i: usize) -> Option<usize> {
    let quote = bytes[i];
    let mut j = i + 1;
    while j < bytes.len() && bytes[j] != quote {
        j += if bytes[j] == b'\\' { 2 } else { 1 };
    }
    (j < bytes.len()).then_some(j + 1)
}

/// The end of the regular expression literal that starts at `i`, past its closing `/`, or `None` when it does not
/// close on its line: then the `/` divides.
fn regex_end(bytes: &[u8], i: usize) -> Option<usize> {
    let mut class = false;
    let mut j = i + 1;
    while let Some(&b) = bytes.get(j) {
        match b {
            b'\\' => j += 1,
            b'[' => class = true,
            b']' => class = false,
            b'/' if !class => return Some(j + 1),
            _ => {}
        }
        j += 1;
    }
    None
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

/// The index of the line after the ESM paragraph that starts at line `first`: the first blank line outside brackets,
/// template literals and block comments, or the end.
///
/// MDX parses the paragraph with acorn at each blank line and goes on past it when the code is incomplete, which
/// includes an unterminated comment: a block comment may span blank lines. A `/*` with no `*/` anywhere after it
/// cannot be one, since MDX rejects the document then; it is JSX text (`<code>dist/*</code>`) or a regular
/// expression (`/\/*/`) the scan does not tell apart. Such a `/*` is not a comment, and the paragraph is scanned again
/// without it, so a blank line ends it as MDX does.
fn esm_paragraph_end(source: &str, all: &[Line], first: usize) -> usize {
    let mut not_comments = Vec::new();
    'scan: loop {
        let mut scan = Scan::new(&not_comments);
        let mut index = first;
        while index < all.len() {
            let line = &source[all[index].start..all[index].end];
            if line.trim().is_empty() {
                if !scan.is_open() {
                    return index;
                }
                if scan.in_block_comment && !source[all[index].start..].contains("*/") {
                    let start = scan.comment_start;
                    not_comments.push(start);
                    continue 'scan;
                }
            }
            scan.line(line, all[index].start);
            index += 1;
        }
        return index;
    }
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
            let end = esm_paragraph_end(source, &all, index);
            regions[index..end].fill(Region::Esm);
            index = end;
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

/// `markup` with its inline code spans (`` `<Card/>` ``, ``` `` a`b `` ```) blanked, so that code quoted in prose is
/// not taken for a component or expression it uses. A span opens with a run of backticks and closes at the next run of
/// the same length; a run that never closes is literal text. Line breaks and the byte length are kept.
pub(crate) fn blank_inline_code(markup: &str) -> String {
    let bytes = markup.as_bytes();
    let mut out = bytes.to_vec();
    let run_at = |at: usize| bytes[at..].iter().take_while(|&&b| b == b'`').count();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let open = run_at(i);
        let mut j = i + open;
        let mut close = None;
        while j < bytes.len() {
            if bytes[j] == b'`' {
                let run = run_at(j);
                if run == open {
                    close = Some(j + run);
                    break;
                }
                j += run;
            } else {
                j += 1;
            }
        }
        match close {
            Some(end) => {
                for byte in &mut out[i..end] {
                    if !matches!(*byte, b'\n' | b'\r') {
                        *byte = b' ';
                    }
                }
                i = end;
            }
            None => i += open,
        }
    }
    // Whole spans between ASCII backticks are blanked, so the buffer is valid UTF-8.
    String::from_utf8(out).unwrap_or_default()
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
    fn an_apostrophe_in_jsx_text_does_not_keep_the_paragraph_open() {
        let source = "export const Note = () => (<p>don't</p>)\n\n# Title\n\n<Note/>\n";
        assert_eq!(kept(source), ["export const Note = () => (<p>don't</p>)"]);
    }

    #[test]
    fn a_multi_line_template_literal_spans_blank_lines_and_ends() {
        let source =
            "export const css = `\n  a :( {\n\n  b ${x({ y: '`' })} c\n`\n\n# Title :(\n\nimport A from './a'\n";
        assert_eq!(
            kept(source),
            [
                "export const css = `",
                "  a :( {",
                "  b ${x({ y: '`' })} c",
                "`",
                "import A from './a'"
            ]
        );
    }

    #[test]
    fn a_comment_opener_that_never_closes_does_not_swallow_the_document() {
        let source = "export const X = () => <code>dist/*</code>\n\n# Title\n\n<Card/>\n";
        assert_eq!(kept(source), ["export const X = () => <code>dist/*</code>"]);
        let source = "export const re = (/\\/*/)\n\n# Title\n\n<Card/>\n";
        assert_eq!(kept(source), ["export const re = (/\\/*/)"]);
        let source = "export const n = 1 /* not closed\n\n# Title\n";
        assert_eq!(kept(source), ["export const n = 1 /* not closed"]);
    }

    #[test]
    fn a_block_comment_that_closes_later_spans_blank_lines() {
        let source = "export const a = 1 /* x\n\ny */\nexport const b = 2\n\n<B/>\n";
        assert_eq!(kept(source), ["export const a = 1 /* x", "y */", "export const b = 2"]);
    }

    #[test]
    fn jsx_text_and_regular_expressions_hold_no_comment() {
        let source = "export const L = () => (<a>http://x</a>)\n\n# Title\n\n<Card/>\n";
        assert_eq!(kept(source), ["export const L = () => (<a>http://x</a>)"]);
        let source = "export const G = () => (<code>dist/*</code>)\n\n# Paths\n\nUse `src/*/index.js`.\n";
        assert_eq!(kept(source), ["export const G = () => (<code>dist/*</code>)"]);
        let source = "export const isUrl = (s) => (/:\\/\\//).test(s)\n\n# Title\n";
        assert_eq!(kept(source), ["export const isUrl = (s) => (/:\\/\\//).test(s)"]);
    }

    #[test]
    fn a_jsx_element_spans_blank_lines() {
        let source = "export function Note() {\n  return <p a=\"{\" b={1 < 2}>\n\n    don't <b>stop</b> {`${x}`}\n  </p>\n}\n\n<Note/>\n";
        assert_eq!(
            kept(source),
            [
                "export function Note() {",
                "  return <p a=\"{\" b={1 < 2}>",
                "    don't <b>stop</b> {`${x}`}",
                "  </p>",
                "}"
            ]
        );
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
    fn inline_code_is_blanked() {
        let markup = "Use `<Card/>` or ``a ` <B/>`` but <C/>, and a lone ` <D/>\n";
        let blanked = blank_inline_code(markup);
        assert_eq!(blanked.len(), markup.len());
        assert!(!blanked.contains("Card") && !blanked.contains("<B/>"), "{blanked}");
        assert!(blanked.contains("<C/>") && blanked.contains("<D/>"), "{blanked}");
    }

    #[test]
    fn non_ascii_prose_keeps_the_byte_length() {
        let source = "Grüße — ünïcode\nimport A from './a'\n";
        let extracted = extract(source);
        assert_eq!(extracted.esm.len(), source.len());
        assert!(extracted.esm.contains("import A from './a'"));
    }
}
