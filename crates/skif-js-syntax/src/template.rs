//! Identifier scan of the template of a single-file component (`.vue`, `.svelte`, `.astro`).
//!
//! Imports used only from a template (`<Card/>`, `{{ format(x) }}`, `{#if ready}`, `:prop="value"`, `@click="go"`,
//! `{items.length}`) must not look unused. This is a scan, not a template compiler: it reads tag names and the
//! expressions inside `{ }`, `{{ }}` and directive attributes, and records which identifiers appear and which members
//! are read through them (`ns.member`). Matching is by name; a template local that shadows an import can only make
//! the import look more used.

use crate::members::{Slot, Uses};

/// The template language, which decides what an expression looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flavor {
    Vue,
    Svelte,
    Astro,
}

/// Records the identifiers used in `text`, the markup of a component outside its script and style blocks.
pub(crate) fn scan(text: &str, flavor: Flavor, uses: &mut Uses) {
    let mut scanner = Scanner { text, flavor, uses };
    scanner.run();
}

struct Scanner<'t, 'u> {
    text: &'t str,
    flavor: Flavor,
    uses: &'u mut Uses,
}

/// Directives whose name after the colon is a binding of the component: `use:action`, `transition:fade`.
const SVELTE_REFERENCING_DIRECTIVES: [&str; 5] = ["use", "transition", "in", "out", "animate"];
/// Directives whose bare form is shorthand for a variable of the same name: `bind:value`, `class:active`.
const SVELTE_SHORTHAND_DIRECTIVES: [&str; 3] = ["bind", "class", "style"];
const VUE_BUILTIN_DIRECTIVES: [&str; 15] = [
    "if", "else", "else-if", "for", "model", "show", "bind", "on", "slot", "text", "html", "once", "pre", "cloak",
    "memo",
];

const fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$')
}

const fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

/// Index just past the string literal that starts at `start`.
fn skip_string(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            c if c == quote => return i + 1,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// Index of the `}` that closes the `{` at `open`, or the end of `bytes`.
fn matching_brace(bytes: &[u8], open: usize) -> usize {
    let mut depth = 0_u32;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            b'"' | b'\'' | b'`' => {
                i = skip_string(bytes, i);
                continue;
            }
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

fn capitalised(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect::<String>())
        .unwrap_or_default()
}

/// `foo-bar` as `FooBar`.
fn pascal_case(kebab: &str) -> String {
    kebab.split('-').map(capitalised).collect()
}

impl Scanner<'_, '_> {
    fn bare(&mut self, name: &str) {
        self.uses.entry(name.to_owned()).or_default().uses += 1;
    }

    fn member(&mut self, name: &str, member: &str) {
        let slot: &mut Slot = self.uses.entry(name.to_owned()).or_default();
        slot.uses += 1;
        slot.member_uses += 1;
        slot.members.insert(member.to_owned());
    }

    fn run(&mut self) {
        let text = self.text;
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'<' {
                if text[i..].starts_with("<!--") {
                    i = text[i + 4..].find("-->").map_or(bytes.len(), |end| i + 4 + end + 3);
                } else if bytes.get(i + 1).is_some_and(u8::is_ascii_alphabetic) {
                    i = self.tag(i);
                } else {
                    i += 1;
                }
            } else if self.flavor == Flavor::Vue && bytes[i..].starts_with(b"{{") {
                let end = text[i + 2..].find("}}").map_or(bytes.len(), |end| i + 2 + end);
                self.expression(&text[i + 2..end]);
                i = (end + 2).min(bytes.len());
            } else if self.flavor != Flavor::Vue && bytes[i] == b'{' {
                i = self.braced(i);
            } else {
                i += 1;
            }
        }
    }

    /// Scans the `{ … }` that starts at `open` and returns the index after it.
    fn braced(&mut self, open: usize) -> usize {
        let text = self.text;
        let close = matching_brace(text.as_bytes(), open);
        self.expression(&text[open + 1..close]);
        (close + 1).min(text.len())
    }

    /// Scans the tag that starts at `start` (a `<`) and returns the index after its `>`.
    fn tag(&mut self, start: usize) -> usize {
        let text = self.text;
        let bytes = text.as_bytes();
        let mut i = start + 1;
        while i < bytes.len() && (is_ident_continue(bytes[i]) || matches!(bytes[i], b'-' | b'.' | b':')) {
            i += 1;
        }
        self.tag_name(&text[start + 1..i]);
        while i < bytes.len() {
            match bytes[i] {
                b'>' => return i + 1,
                c if c.is_ascii_whitespace() || c == b'/' => i += 1,
                b'{' => i = self.braced(i),
                _ => i = self.attribute(i),
            }
        }
        bytes.len()
    }

    fn tag_name(&mut self, name: &str) {
        if name.contains(':') {
            return;
        }
        if let Some((object, member)) = name.split_once('.') {
            let member = member.split('.').next().unwrap_or(member);
            self.member(object, member);
            return;
        }
        self.bare(name);
        if name.contains('-') {
            let pascal = pascal_case(name);
            let mut camel = pascal.clone();
            if let Some(first) = camel.get_mut(0..1) {
                first.make_ascii_lowercase();
            }
            self.bare(&pascal);
            self.bare(&camel);
        } else if self.flavor == Flavor::Vue {
            self.bare(&capitalised(name));
        }
    }

    /// Scans the attribute that starts at `start` and returns the index after it.
    fn attribute(&mut self, start: usize) -> usize {
        let text = self.text;
        let bytes = text.as_bytes();
        let mut i = start;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !matches!(bytes[i], b'=' | b'>' | b'/') {
            i += 1;
        }
        let name = &text[start..i];
        let mut after_name = i;
        while after_name < bytes.len() && bytes[after_name].is_ascii_whitespace() {
            after_name += 1;
        }
        let has_value = bytes.get(after_name) == Some(&b'=');
        self.attribute_name(name, has_value);
        if !has_value {
            return i.max(start + 1);
        }
        let mut value = after_name + 1;
        while value < bytes.len() && bytes[value].is_ascii_whitespace() {
            value += 1;
        }
        match bytes.get(value) {
            Some(b'{') => self.braced(value),
            Some(b'"' | b'\'') => {
                let end = skip_string(bytes, value);
                let closed = end > value + 1 && bytes[end - 1] == bytes[value];
                let inner = text
                    .get(value + 1..if closed { end - 1 } else { end })
                    .unwrap_or_default();
                if self.is_expression_attribute(name) {
                    self.expression(inner);
                } else if self.flavor != Flavor::Vue {
                    self.interpolations(inner);
                }
                end
            }
            Some(_) => {
                let mut end = value;
                while end < bytes.len() && !bytes[end].is_ascii_whitespace() && bytes[end] != b'>' {
                    end += 1;
                }
                if self.is_expression_attribute(name) {
                    self.expression(&text[value..end]);
                }
                end
            }
            None => bytes.len(),
        }
    }

    fn is_expression_attribute(&self, name: &str) -> bool {
        self.flavor == Flavor::Vue && (name.starts_with([':', '@', '#', '.']) || name.starts_with("v-"))
    }

    /// Credits what an attribute name itself refers to: a custom Vue directive or a Svelte action.
    fn attribute_name(&mut self, name: &str, has_value: bool) {
        match self.flavor {
            Flavor::Vue => {
                let Some(directive) = name.strip_prefix("v-") else {
                    return;
                };
                let directive = directive.split([':', '.']).next().unwrap_or(directive);
                if !directive.is_empty() && !VUE_BUILTIN_DIRECTIVES.contains(&directive) {
                    self.bare(&format!("v{}", pascal_case(directive)));
                }
            }
            Flavor::Svelte => {
                let Some((prefix, rest)) = name.split_once(':') else {
                    return;
                };
                let target = rest.split('|').next().unwrap_or(rest);
                if SVELTE_REFERENCING_DIRECTIVES.contains(&prefix)
                    || (!has_value && SVELTE_SHORTHAND_DIRECTIVES.contains(&prefix))
                {
                    self.bare(target);
                }
            }
            Flavor::Astro => {}
        }
    }

    /// Scans the `{ … }` parts of a quoted attribute value.
    fn interpolations(&mut self, value: &str) {
        let bytes = value.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'{' {
                let close = matching_brace(bytes, i);
                self.expression(&value[i + 1..close]);
                i = close + 1;
            } else {
                i += 1;
            }
        }
    }

    /// Records every identifier of the JavaScript expression `code`, and the member read right after it.
    fn expression(&mut self, code: &str) {
        let bytes = code.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let current = bytes[i];
            if current == b'\'' || current == b'"' {
                i = skip_string(bytes, i);
            } else if current == b'`' {
                i = self.template_literal(code, i);
            } else if current.is_ascii_digit() {
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'.' | b'_')) {
                    i += 1;
                }
            } else if is_ident_start(current) {
                let start = i;
                while i < bytes.len() && is_ident_continue(bytes[i]) {
                    i += 1;
                }
                let property = start > 0 && bytes[start - 1] == b'.' && !code[..start].ends_with("...");
                if property {
                    continue;
                }
                let name = &code[start..i];
                match member_after(code, i) {
                    Some(member) => self.member(name, member),
                    None => self.bare(name),
                }
            } else {
                i += 1;
            }
        }
    }

    /// Scans the `${ … }` parts of the template literal that starts at `start` and returns the index after it.
    fn template_literal(&mut self, code: &str, start: usize) -> usize {
        let bytes = code.as_bytes();
        let mut i = start + 1;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b'`' => return i + 1,
                b'$' if bytes.get(i + 1) == Some(&b'{') => {
                    let close = matching_brace(bytes, i + 1);
                    self.expression(&code[i + 2..close]);
                    i = close + 1;
                }
                _ => i += 1,
            }
        }
        bytes.len()
    }
}

/// The identifier read through `.` or `?.` right after the name that ends at `end`.
fn member_after(code: &str, end: usize) -> Option<&str> {
    let rest = &code[end..];
    let rest = rest.strip_prefix("?.").or_else(|| rest.strip_prefix('.'))?;
    let length = rest
        .bytes()
        .position(|byte| !is_ident_continue(byte))
        .unwrap_or(rest.len());
    (length > 0 && is_ident_start(rest.as_bytes()[0])).then(|| &rest[..length])
}

#[cfg(test)]
mod tests {
    use super::{Flavor, scan};
    use crate::members::Uses;

    fn used(text: &str, flavor: Flavor) -> Uses {
        let mut uses = Uses::new();
        scan(text, flavor, &mut uses);
        uses
    }

    fn bare(uses: &Uses, name: &str) -> bool {
        uses.get(name).is_some_and(super::Slot::escapes)
    }

    fn members(uses: &Uses, name: &str) -> Vec<String> {
        uses.get(name)
            .map(|slot| slot.members.iter().cloned().collect())
            .unwrap_or_default()
    }

    #[test]
    fn vue_components_in_every_spelling() {
        let uses = used(
            "<template><MyCard/><my-button :x=\"1\"></my-button><lower /><ui.Item/></template>",
            Flavor::Vue,
        );
        assert!(bare(&uses, "MyCard"));
        assert!(bare(&uses, "MyButton"));
        assert!(bare(&uses, "myButton"));
        assert!(bare(&uses, "Lower"));
        assert_eq!(members(&uses, "ui"), ["Item"]);
        assert!(!bare(&uses, "ui"));
    }

    #[test]
    fn vue_expressions_and_directives() {
        let uses = used(
            "<div :class=\"cls(a)\" @click=\"go\" v-focus v-model=\"form.name\" v-for=\"row in rows\" id=\"plain\">{{ fmt(price) + ns.max }}</div>",
            Flavor::Vue,
        );
        for name in ["cls", "a", "go", "vFocus", "rows", "fmt", "price"] {
            assert!(bare(&uses, name), "{name}");
        }
        assert_eq!(members(&uses, "form"), ["name"]);
        assert_eq!(members(&uses, "ns"), ["max"]);
        assert!(
            !uses.contains_key("plain"),
            "static attribute values are not expressions"
        );
        assert!(!uses.contains_key("id"));
        assert!(!bare(&uses, "model"), "a property is not a use of an import");
    }

    #[test]
    fn svelte_blocks_expressions_and_directives() {
        let uses = used(
            "{#if ready}<Widget on:click={handle} use:tooltip bind:value class:active {...rest} />{:else if other}{/if}{@html raw}{#each list as item}<p>{item}</p>{/each}",
            Flavor::Svelte,
        );
        for name in [
            "ready", "Widget", "handle", "tooltip", "value", "active", "rest", "other", "raw", "list",
        ] {
            assert!(bare(&uses, name), "{name}");
        }
    }

    #[test]
    fn astro_expressions_and_nested_markup() {
        let uses = used(
            "<Layout title={site.title}>{items.map((x) => <Item data={x} />)}<p class=\"a {cls}\">text</p></Layout>",
            Flavor::Astro,
        );
        assert!(bare(&uses, "Layout"));
        assert!(bare(&uses, "Item"));
        assert!(bare(&uses, "cls"));
        assert_eq!(members(&uses, "site"), ["title"]);
        assert_eq!(members(&uses, "items"), ["map"]);
    }

    #[test]
    fn strings_comments_and_prose_are_not_expressions() {
        let uses = used(
            "<!-- <Hidden/> {{ gone }} --><p>prose with ns and a < b</p><span :title=\"'quoted' + `t${inner}`\"/>",
            Flavor::Vue,
        );
        assert!(!uses.contains_key("Hidden"));
        assert!(!uses.contains_key("gone"));
        assert!(!uses.contains_key("ns"));
        assert!(!uses.contains_key("quoted"));
        assert!(bare(&uses, "inner"));
    }
}
