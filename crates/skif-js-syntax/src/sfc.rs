//! Block extraction for single-file components (`.vue`, `.svelte`, `.astro`).
//!
//! This is a textual scan, not an HTML parser: it finds `<script …>…</script>` and `<style …>…</style>` pairs (and the
//! Astro `---` frontmatter) and returns their byte ranges so spans map straight back into the original file. What is
//! left is the markup, which [`crate::template`] scans for the identifiers it uses.

/// A script region of a component file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ScriptBlock {
    pub start: usize,
    pub end: usize,
    pub typescript: bool,
    pub jsx: bool,
    /// The block is JavaScript or TypeScript. Other blocks (`type="application/json"`, `type="text/template"`) are
    /// data: they are neither parsed nor part of the template.
    pub parse: bool,
    pub context: ScriptContext,
}

/// Which script of a Svelte component a block is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptContext {
    /// The instance script (and every script of other component kinds): in Svelte its exports are props.
    Instance,
    /// The module script (`context="module"` in Svelte 4, `module` in Svelte 5), whose exports are exports of the
    /// module.
    Module,
}

/// A `<style>` region of a component file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StyleBlock {
    pub start: usize,
    pub end: usize,
    /// Extension of the stylesheet language (`css`, `scss`, `sass`, `less`, `styl`).
    pub extension: &'static str,
}

/// A `src="…"` reference of a `<script>` or `<style>` tag: the block's code lives in another file.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SrcReference {
    pub specifier: String,
    pub start: usize,
    pub end: usize,
}

/// Everything a component file holds besides its markup.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Blocks {
    pub scripts: Vec<ScriptBlock>,
    pub styles: Vec<StyleBlock>,
    pub sources: Vec<SrcReference>,
}

impl Blocks {
    /// Byte ranges of the markup: the file without its script, style and frontmatter blocks, in source order.
    pub(crate) fn markup(&self, source: &str) -> Vec<(usize, usize)> {
        let mut taken: Vec<(usize, usize)> = self
            .scripts
            .iter()
            .map(|block| (block.start, block.end))
            .chain(self.styles.iter().map(|block| (block.start, block.end)))
            .collect();
        taken.sort_unstable();
        let mut markup = Vec::new();
        let mut cursor = 0;
        for (start, end) in taken {
            if start > cursor {
                markup.push((cursor, start));
            }
            cursor = cursor.max(end);
        }
        if cursor < source.len() {
            markup.push((cursor, source.len()));
        }
        markup
    }
}

pub(crate) fn blocks(source: &str, astro: bool) -> Blocks {
    let mut found = Blocks::default();
    let mut cursor = 0;
    if astro && let Some(frontmatter) = astro_frontmatter(source) {
        cursor = frontmatter.end;
        found.scripts.push(frontmatter);
    }
    while let Some((tag_start, name)) = next_block_tag(source, cursor) {
        let after_name = tag_start + 1 + name.len();
        let boundary_ok = source[after_name..]
            .chars()
            .next()
            .is_some_and(|c| c.is_whitespace() || c == '>' || c == '/');
        if !boundary_ok {
            cursor = after_name;
            continue;
        }
        let Some(tag_end) = tag_close(source, after_name) else {
            break;
        };
        let attrs = &source[after_name..tag_end];
        let self_closing = attrs.trim_end().ends_with('/');
        let body_start = tag_end + 1;
        if let Some(specifier) = attribute(attrs, "src").filter(|src| !src.is_empty()) {
            found.sources.push(SrcReference {
                specifier,
                start: tag_start,
                end: body_start,
            });
        }
        if self_closing {
            cursor = body_start;
            continue;
        }
        let closing: &[u8] = if name == "script" { b"</script" } else { b"</style" };
        let Some(close) = memchr::memmem::find(&source.as_bytes()[body_start..], closing) else {
            break;
        };
        let body_end = body_start + close;
        let lang = attribute(attrs, "lang").unwrap_or_default();
        if name == "script" {
            let kind = attribute(attrs, "type").unwrap_or_default();
            found.scripts.push(ScriptBlock {
                start: body_start,
                end: body_end,
                typescript: astro || matches!(lang.as_str(), "ts" | "tsx" | "typescript"),
                jsx: lang == "tsx" || lang == "jsx",
                parse: kind.is_empty() || SCRIPT_TYPES.contains(&kind.as_str()),
                context: if has_attribute(attrs, "module") || attribute(attrs, "context").is_some_and(|c| c == "module")
                {
                    ScriptContext::Module
                } else {
                    ScriptContext::Instance
                },
            });
        } else {
            found.styles.push(StyleBlock {
                start: body_start,
                end: body_end,
                extension: match lang.as_str() {
                    "scss" => "scss",
                    "sass" => "sass",
                    "less" => "less",
                    "stylus" | "styl" => "styl",
                    _ => "css",
                },
            });
        }
        cursor = body_end;
    }
    found
}

/// `type` values of a `<script>` whose body is code.
const SCRIPT_TYPES: [&str; 6] = [
    "module",
    "text/javascript",
    "application/javascript",
    "text/typescript",
    "application/typescript",
    "ts",
];

/// The next `<script` or `<style` at or after `from`: its start and tag name. One pass over the rest of the file finds
/// whichever comes first.
fn next_block_tag(source: &str, from: usize) -> Option<(usize, &'static str)> {
    let rest = &source.as_bytes()[from..];
    memchr::memmem::find_iter(rest, b"<s").find_map(|at| {
        let tag = &rest[at..];
        let name = if tag.starts_with(b"<script") {
            "script"
        } else if tag.starts_with(b"<style") {
            "style"
        } else {
            return None;
        };
        Some((from + at, name))
    })
}

/// Index of the `>` closing a start tag, skipping quoted attribute values.
fn tag_close(source: &str, from: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut at = from;
    loop {
        let found = at + memchr::memchr3(b'>', b'"', b'\'', &bytes[at..])?;
        match bytes[found] {
            b'>' => return Some(found),
            quote => at = found + 1 + memchr::memchr(quote, &bytes[found + 1..])? + 1,
        }
    }
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

/// Whether the attributes `attrs` of a start tag hold one named `wanted`, with a value or without one (`module`).
fn has_attribute(attrs: &str, wanted: &str) -> bool {
    let bytes = attrs.as_bytes();
    let skip_space = |mut at: usize| {
        while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        at
    };
    let mut at = 0;
    while at < bytes.len() {
        let start = at;
        while bytes
            .get(at)
            .is_some_and(|&byte| !byte.is_ascii_whitespace() && !matches!(byte, b'=' | b'/' | b'>'))
        {
            at += 1;
        }
        if at > start && attrs[start..at].eq_ignore_ascii_case(wanted) {
            return true;
        }
        let after_name = skip_space(at);
        if bytes.get(after_name) != Some(&b'=') {
            at = at.max(start + 1);
            continue;
        }
        let value = skip_space(after_name + 1);
        at = match bytes.get(value) {
            Some(&quote @ (b'"' | b'\'')) => {
                memchr::memchr(quote, &bytes[value + 1..]).map_or(bytes.len(), |end| value + end + 2)
            }
            _ => bytes[value..]
                .iter()
                .position(u8::is_ascii_whitespace)
                .map_or(bytes.len(), |end| value + end),
        };
    }
    false
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
        parse: true,
        context: ScriptContext::Instance,
    })
}

#[cfg(test)]
mod tests {
    use super::{ScriptContext, blocks};

    #[test]
    fn finds_script_bodies() {
        let cases: &[(&str, bool, &[&str])] = &[
            ("<template/><script>let a</script>", false, &["let a"]),
            (
                "<script lang=\"ts\" setup>let b</script><script>let c</script>",
                false,
                &["let b", "let c"],
            ),
            ("<script src=\"./x.js\"/>", false, &[]),
            ("<scripty>no</scripty>", false, &[]),
            ("---\nimport a from 'a'\n---\n<div/>", true, &["\nimport a from 'a'\n"]),
            ("<script attr=\"a>b\">x</script>", false, &["x"]),
            (
                "<span/><style>a</style><select/><script>let s</script>",
                false,
                &["let s"],
            ),
            ("<script data-x='>' data-y=\"'\">let q</script>", false, &["let q"]),
            ("<script data-x=\"unclosed>let u</script>", false, &[]),
        ];
        for (src, astro, expected) in cases {
            let got: Vec<&str> = blocks(src, *astro)
                .scripts
                .iter()
                .map(|b| &src[b.start..b.end])
                .collect();
            assert_eq!(&got, expected, "{src}");
        }
    }

    #[test]
    fn detects_typescript() {
        let found = blocks("<script lang='ts'>x</script><script>y</script>", false);
        assert_eq!(
            found.scripts.iter().map(|b| b.typescript).collect::<Vec<_>>(),
            [true, false]
        );
    }

    #[test]
    fn detects_svelte_module_scripts() {
        let cases = [
            ("<script context=\"module\">x</script>", true),
            ("<script lang='ts' context = 'module'>x</script>", true),
            ("<script module>x</script>", true),
            ("<script module lang=\"ts\">x</script>", true),
            ("<script>x</script>", false),
            ("<script context=\"instance\">x</script>", false),
            ("<script data-module title=\"module\" modules>x</script>", false),
        ];
        for (source, module) in cases {
            let found = blocks(source, false);
            let modules: Vec<bool> = found
                .scripts
                .iter()
                .map(|b| b.context == ScriptContext::Module)
                .collect();
            assert_eq!(modules, [module], "{source}");
        }
    }

    #[test]
    fn styles_sources_data_scripts_and_markup() {
        let source = "<template><p/></template>\n<script setup lang=\"ts\">let a</script>\n<script type=\"application/json\">{\"a\":1}</script>\n<script src=\"./ext.ts\"></script>\n<style lang=\"scss\">@use 'x';</style>\n<style src=\"./y.css\"/>\n";
        let found = blocks(source, false);
        let parsed: Vec<bool> = found.scripts.iter().map(|b| b.parse).collect();
        assert_eq!(parsed, [true, false, true]);
        assert_eq!(found.styles.len(), 1);
        assert_eq!(found.styles[0].extension, "scss");
        assert_eq!(&source[found.styles[0].start..found.styles[0].end], "@use 'x';");
        let sources: Vec<&str> = found.sources.iter().map(|s| s.specifier.as_str()).collect();
        assert_eq!(sources, ["./ext.ts", "./y.css"]);
        let markup: String = found
            .markup(source)
            .iter()
            .map(|(start, end)| &source[*start..*end])
            .collect();
        assert!(markup.contains("<template><p/></template>"));
        assert!(!markup.contains("let a") && !markup.contains("@use") && !markup.contains("\"a\":1"));
    }
}
