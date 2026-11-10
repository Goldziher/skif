//! Identifier scan of the template of a single-file component (`.vue`, `.svelte`, `.astro`) and of MDX markup.
//!
//! Imports used only from a template (`<Card/>`, `{{ format(x) }}`, `{#if ready}`, `:prop="value"`, `@click="go"`,
//! `{items.length}`) must not look unused. This is a scan, not a template compiler: it reads tag names and the
//! expressions inside `{ }`, `{{ }}` and directive attributes, and records which identifiers appear and which members
//! are read through them (`ns.member`).
//!
//! The expressions are JavaScript, so oxc parses them: a name the expression binds itself (`items.map((x) => x)`) is
//! no use. The template locals of the constructs around an expression are kept on a scope stack: Vue `v-for`,
//! `v-slot` (`#name`) and `slot-scope`, and Svelte `{#each}`, `{#await}`, `{:then}`, `{:catch}`, `{#snippet}`,
//! `{@const}` and `let:`. A name bound either way is not a use of an import of the same name (ADR 0082). An
//! expression oxc cannot parse is read token by token, which can only make an import look more used.
//!
//! The tag names are returned as written, with the static values of Vue `is` attributes, for linking the components
//! `unplugin-vue-components` registers (ADR 0082).

use std::collections::BTreeSet;

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    ComputedMemberExpression, Expression, IdentifierReference, JSXMemberExpression, JSXMemberExpressionObject,
    Statement, StaticMemberExpression,
};
use oxc_ast_visit::{Visit, walk};
use oxc_parser::Parser;
use oxc_semantic::{Scoping, SemanticBuilder};
use oxc_span::SourceType;

use crate::components::{camelize, capitalize, resolved_names};
use crate::members::{Slot, Uses};

/// The template language, which decides what an expression looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flavor {
    Vue,
    Svelte,
    Astro,
}

/// Records the identifiers used in `text`, the markup of a component outside its script and style blocks, and
/// returns the tag names of its elements as written (`AppButton`, `app-button`, `div`), with the components Vue `is`
/// attributes name statically.
pub(crate) fn scan(text: &str, flavor: Flavor, uses: &mut Uses) -> BTreeSet<String> {
    let mut scanner = Scanner {
        text,
        flavor,
        uses,
        scopes: Vec::new(),
        tags: BTreeSet::new(),
        allocator: Allocator::default(),
    };
    scanner.run();
    scanner.tags
}

/// The template locals of one construct.
struct Frame {
    /// The element whose end tag closes the frame; `None` for a Svelte block, which `{/…}` closes.
    element: Option<String>,
    names: Vec<String>,
    /// The elements of the same name open inside it that have no frame of their own: their end tags come first.
    depth: u32,
}

impl Frame {
    /// The frame of a Svelte block.
    const fn block(names: Vec<String>) -> Self {
        Self {
            element: None,
            names,
            depth: 0,
        }
    }
}

struct Scanner<'t, 'u> {
    text: &'t str,
    flavor: Flavor,
    uses: &'u mut Uses,
    /// Innermost last. A Vue element has a frame only when it binds names; a Svelte element always has one, which a
    /// `{@const}` inside it adds to.
    scopes: Vec<Frame>,
    tags: BTreeSet<String>,
    /// The arena every expression of the scan is parsed into, reset before each.
    allocator: Allocator,
}

/// One attribute of a start tag, or a bare `{…}` in it (Svelte and Astro spreads and shorthands), with no name.
struct Attribute<'t> {
    name: &'t str,
    value: Option<Value<'t>>,
}

enum Value<'t> {
    /// `{…}`, without the braces.
    Braced(&'t str),
    /// `"…"` or `'…'`, without the quotes.
    Quoted(&'t str),
    /// An unquoted value.
    Bare(&'t str),
}

impl<'t> Value<'t> {
    const fn code(&self) -> &'t str {
        match self {
            Self::Braced(code) | Self::Quoted(code) | Self::Bare(code) => *code,
        }
    }
}

/// Where the locals an attribute binds are visible.
#[derive(Clone, Copy)]
enum Reach {
    /// The other attributes of the element and its content: `v-for`.
    Element,
    /// The content of the element: `v-slot`, `#name`, `slot-scope`, `let:`.
    Content,
}

/// What a piece of template code declares besides the names it uses.
#[derive(Clone, Copy)]
enum Binds {
    /// Nothing: an expression or the statements of an event handler.
    Nothing,
    /// The parameters of the arrow function the code is: `(item, index) => 0`.
    Parameters,
    /// Its top-level declarations: `const total = …`.
    Declarations,
}

/// A use found in parsed code: a name, and the member read right through it.
type Found = (String, Option<String>);

/// Directives whose name after the colon is a binding of the component: `use:action`, `transition:fade`.
const SVELTE_REFERENCING_DIRECTIVES: [&str; 5] = ["use", "transition", "in", "out", "animate"];
/// Directives whose bare form is shorthand for a variable of the same name: `bind:value`, `class:active`.
const SVELTE_SHORTHAND_DIRECTIVES: [&str; 3] = ["bind", "class", "style"];
const VUE_BUILTIN_DIRECTIVES: [&str; 15] = [
    "if", "else", "else-if", "for", "model", "show", "bind", "on", "slot", "text", "html", "once", "pre", "cloak",
    "memo",
];
/// HTML elements without content: their start tag opens no scope.
const VOID_ELEMENTS: [&str; 14] = [
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source", "track", "wbr",
];

const fn is_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$')
}

const fn is_ident_continue(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

const fn is_tag_name_byte(byte: u8) -> bool {
    is_ident_continue(byte) || matches!(byte, b'-' | b'.' | b':')
}

fn is_void(name: &str) -> bool {
    VOID_ELEMENTS.iter().any(|void| void.eq_ignore_ascii_case(name))
}

/// `code` is a name or a chain of property reads (`user.name`): reading its tokens gives what parsing it would.
fn is_path(code: &str) -> bool {
    code.as_bytes().first().is_some_and(|first| is_ident_start(*first))
        && !code.contains("..")
        && !code.ends_with('.')
        && code.bytes().all(|byte| is_ident_continue(byte) || byte == b'.')
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

/// Byte index of `word` standing alone (after whitespace, before whitespace or the end) outside brackets and
/// strings: the first occurrence, or with `last` the last one.
fn find_word(code: &str, word: &str, last: bool) -> Option<usize> {
    let bytes = code.as_bytes();
    let mut depth = 0_i32;
    let mut found = None;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'"' | b'\'' | b'`' => {
                i = skip_string(bytes, i);
                continue;
            }
            byte if depth == 0 && byte.is_ascii_whitespace() && code[i + 1..].starts_with(word) => {
                if bytes.get(i + 1 + word.len()).is_none_or(u8::is_ascii_whitespace) {
                    found = Some(i + 1);
                    if !last {
                        return found;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    found
}

/// `code` before and after the standalone `word`, trimmed.
fn split_word<'c>(code: &'c str, word: &str, last: bool) -> Option<(&'c str, &'c str)> {
    find_word(code, word, last).map(|at| (code[..at].trim(), code[at + word.len()..].trim()))
}

/// The alias and the source of a Vue `v-for`: `(item, index) in items` is `item, index` and `items`.
fn split_v_for(value: &str) -> Option<(&str, &str)> {
    let at = [find_word(value, "in", false), find_word(value, "of", false)]
        .into_iter()
        .flatten()
        .min()?;
    let alias = value[..at].trim();
    let alias = alias
        .strip_prefix('(')
        .and_then(|inner| inner.strip_suffix(')'))
        .unwrap_or(alias);
    Some((alias, value[at + 2..].trim()))
}

/// The context of a Svelte `{#each}` and its key: `item, i (item.id)` is `item, i` keyed by `item.id`.
fn split_key(context: &str) -> (&str, Option<&str>) {
    let trimmed = context.trim_end();
    if !trimmed.ends_with(')') {
        return (context, None);
    }
    let bytes = trimmed.as_bytes();
    let mut depth = 0_u32;
    for i in (0..bytes.len()).rev() {
        match bytes[i] {
            b')' => depth += 1,
            b'(' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    let pattern = trimmed[..i].trim();
                    return if pattern.is_empty() {
                        (context, None)
                    } else {
                        (pattern, Some(&trimmed[i + 1..trimmed.len() - 1]))
                    };
                }
            }
            _ => {}
        }
    }
    (context, None)
}

/// The leading keyword of a Svelte tag and the rest: `each items as item` is `each` and ` items as item`.
fn keyword(tag: &str) -> (&str, &str) {
    let end = tag.find(|c: char| !c.is_ascii_alphabetic()).unwrap_or(tag.len());
    (&tag[..end], &tag[end..])
}

/// Index just past the quoted HTML attribute value that starts at `start`: the next quote of the same kind. A `\`
/// escapes nothing in HTML (`title="C:\"`).
fn skip_attribute_value(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    bytes[start + 1..]
        .iter()
        .position(|byte| *byte == quote)
        .map_or(bytes.len(), |at| start + 1 + at + 1)
}

/// The component a Vue `is` attribute names statically: `is="AppButton"`, `is="vue:AppButton"` on a native element,
/// or the string literal of `:is="'AppButton'"`.
fn static_is<'t>(attribute: &Attribute<'t>) -> Option<&'t str> {
    let value = match attribute.value {
        Some(Value::Quoted(value) | Value::Bare(value)) => value.trim(),
        _ => return None,
    };
    let name = match attribute.name {
        "is" => value.strip_prefix("vue:").unwrap_or(value),
        ":is" | "v-bind:is" => {
            let quote = value.chars().next().filter(|quote| matches!(*quote, '\'' | '"' | '`'))?;
            let inner = value.strip_prefix(quote)?.strip_suffix(quote)?;
            if inner.contains([quote, '\\']) || inner.contains("${") {
                return None;
            }
            inner
        }
        _ => return None,
    };
    (!name.is_empty()).then_some(name)
}

/// Whether `code` may declare a name: an arrow, a block or object (functions, classes, methods, `catch`) or a
/// declaration. Code without one binds nothing, so its references need no semantic analysis.
fn may_bind(code: &str) -> bool {
    code.contains("=>") || code.contains('{') || ["let", "const", "var", "using"].iter().any(|word| code.contains(word))
}

/// Whether the Vue event handler `code` is a function expression (`function (e) {…}`, `async () => …`), which Vue
/// calls, rather than statements it wraps in a function.
fn is_function_handler(code: &str) -> bool {
    let code = code.strip_prefix("async").map_or(code, str::trim_start);
    code.starts_with("function") || (code.starts_with('(') && code.contains("=>"))
}

/// Parses `code` into `allocator` and returns the uses it makes of names it does not bind, and the names `binds` asks
/// for; `None` when `code` is no valid program.
fn analyse(
    allocator: &Allocator,
    code: &str,
    source_type: SourceType,
    binds: Binds,
) -> Option<(Vec<Found>, Vec<String>)> {
    let parsed = Parser::new(allocator, code, source_type).parse();
    if parsed.fatal_error || parsed.diagnostics.has_errors() {
        return None;
    }
    let program = parsed.program;
    if matches!(binds, Binds::Nothing) && !may_bind(code) {
        let mut references = References {
            scoping: None,
            found: Vec::new(),
        };
        references.visit_program(&program);
        return Some((references.found, Vec::new()));
    }
    let semantic = SemanticBuilder::new().build(&program).semantic;
    let scoping = semantic.scoping();
    let scope = match binds {
        Binds::Nothing => None,
        Binds::Declarations => Some(scoping.root_scope_id()),
        Binds::Parameters => match program.body.first() {
            Some(Statement::ExpressionStatement(statement)) => match &statement.expression {
                Expression::ArrowFunctionExpression(arrow) => arrow.scope_id.get(),
                _ => None,
            },
            _ => None,
        },
    };
    let names = scope.map_or_else(Vec::new, |scope| {
        scoping
            .symbol_ids()
            .filter(|symbol| scoping.symbol_scope_id(*symbol) == scope)
            .map(|symbol| scoping.symbol_name(symbol).to_owned())
            .collect()
    });
    let mut references = References {
        scoping: Some(scoping),
        found: Vec::new(),
    };
    references.visit_program(&program);
    Some((references.found, names))
}

/// Collects the references of a program that resolve to no binding of it, with the member read through each.
struct References<'s> {
    /// The program's scopes; `None` for a program that binds nothing.
    scoping: Option<&'s Scoping>,
    found: Vec<Found>,
}

impl References<'_> {
    fn read(&mut self, id: &IdentifierReference<'_>, member: Option<&str>) {
        let bound = self.scoping.is_some_and(|scoping| {
            id.reference_id
                .get()
                .is_some_and(|reference| scoping.get_reference(reference).symbol_id().is_some())
        });
        if !bound {
            self.found
                .push((id.name.as_str().to_owned(), member.map(str::to_owned)));
        }
    }
}

impl<'a> Visit<'a> for References<'_> {
    fn visit_identifier_reference(&mut self, it: &IdentifierReference<'a>) {
        self.read(it, None);
    }

    fn visit_static_member_expression(&mut self, it: &StaticMemberExpression<'a>) {
        if let Expression::Identifier(id) = &it.object {
            self.read(id, Some(it.property.name.as_str()));
        } else {
            walk::walk_static_member_expression(self, it);
        }
    }

    fn visit_computed_member_expression(&mut self, it: &ComputedMemberExpression<'a>) {
        if let Expression::Identifier(id) = &it.object
            && let Expression::StringLiteral(literal) = &it.expression
            && let Some(member) = literal.value.as_str()
        {
            self.read(id, Some(member));
        } else {
            walk::walk_computed_member_expression(self, it);
        }
    }

    fn visit_jsx_member_expression(&mut self, it: &JSXMemberExpression<'a>) {
        if let JSXMemberExpressionObject::IdentifierReference(id) = &it.object {
            self.read(id, Some(it.property.name.as_str()));
        } else {
            walk::walk_jsx_member_expression(self, it);
        }
    }
}

impl<'t> Scanner<'t, '_> {
    /// Whether a template local of an enclosing construct is named `name`.
    fn bound(&self, name: &str) -> bool {
        self.scopes
            .iter()
            .any(|frame| frame.names.iter().any(|local| local == name))
    }

    fn bare(&mut self, name: &str) {
        if !self.bound(name) {
            self.uses.entry(name.to_owned()).or_default().uses += 1;
        }
    }

    fn member(&mut self, name: &str, member: &str) {
        if self.bound(name) {
            return;
        }
        let slot: &mut Slot = self.uses.entry(name.to_owned()).or_default();
        slot.uses += 1;
        slot.member_uses += 1;
        slot.members.insert(member.to_owned());
    }

    fn credit(&mut self, found: Vec<Found>) {
        for (name, member) in found {
            match member {
                Some(member) => self.member(&name, &member),
                None => self.bare(&name),
            }
        }
    }

    /// Astro and MDX expressions hold JSX; Vue and Svelte ones may hold TypeScript.
    const fn source_type(&self) -> SourceType {
        match self.flavor {
            Flavor::Astro => SourceType::tsx(),
            Flavor::Vue | Flavor::Svelte => SourceType::ts(),
        }
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
                } else if bytes.get(i + 1) == Some(&b'/') {
                    i = self.end_tag(i);
                } else {
                    i += 1;
                }
            } else if self.flavor == Flavor::Vue && bytes[i..].starts_with(b"{{") {
                let end = text[i + 2..].find("}}").map_or(bytes.len(), |end| i + 2 + end);
                self.expression(&text[i + 2..end]);
                i = (end + 2).min(bytes.len());
            } else if self.flavor != Flavor::Vue && bytes[i] == b'{' {
                let close = matching_brace(bytes, i);
                let content = &text[i + 1..close];
                if self.flavor == Flavor::Svelte {
                    self.svelte_tag(content);
                } else {
                    self.expression(content);
                }
                i = (close + 1).min(bytes.len());
            } else {
                i += 1;
            }
        }
    }

    /// Reads the end tag that starts at `start` (a `</`), closes the scopes of its element, and returns the index
    /// after its `>`.
    fn end_tag(&mut self, start: usize) -> usize {
        let text = self.text;
        let bytes = text.as_bytes();
        let mut i = start + 2;
        while i < bytes.len() && is_tag_name_byte(bytes[i]) {
            i += 1;
        }
        let name = &text[start + 2..i];
        if let Some(at) = self
            .scopes
            .iter()
            .rposition(|frame| frame.element.as_deref() == Some(name))
        {
            if self.scopes[at].depth > 0 {
                self.scopes[at].depth -= 1;
            } else {
                self.scopes.truncate(at);
            }
        }
        text[i..].find('>').map_or(bytes.len(), |end| i + end + 1)
    }

    /// Scans the start tag that starts at `start` (a `<`) and returns the index after its `>`.
    fn tag(&mut self, start: usize) -> usize {
        let text = self.text;
        let bytes = text.as_bytes();
        let mut i = start + 1;
        while i < bytes.len() && is_tag_name_byte(bytes[i]) {
            i += 1;
        }
        let name = &text[start + 1..i];
        self.tag_name(name);
        let (attributes, end, self_closing) = self.attributes(i);
        let opens = self.flavor != Flavor::Astro && !self_closing && !is_void(name);
        let mut element = Vec::new();
        let mut content = Vec::new();
        for attribute in &attributes {
            if let Some(reach) = self.reach(attribute.name) {
                let names = self.bind(attribute);
                match reach {
                    Reach::Element => element.extend(names),
                    Reach::Content => content.extend(names),
                }
            }
        }
        let framed = self.flavor == Flavor::Svelte || !element.is_empty() || !content.is_empty();
        if framed {
            self.scopes.push(Frame {
                element: opens.then(|| name.to_owned()),
                names: element,
                depth: 0,
            });
        }
        for attribute in &attributes {
            if self.reach(attribute.name).is_none() {
                self.attribute(attribute);
            }
            if self.flavor == Flavor::Vue
                && let Some(component) = static_is(attribute)
            {
                self.tags.insert(component.to_owned());
            }
        }
        if framed {
            if opens {
                if let Some(frame) = self.scopes.last_mut() {
                    frame.names.extend(content);
                }
            } else {
                self.scopes.pop();
            }
        } else if opens
            && let Some(frame) = self
                .scopes
                .iter_mut()
                .rev()
                .find(|frame| frame.element.as_deref() == Some(name))
        {
            frame.depth += 1;
        }
        end
    }

    fn tag_name(&mut self, name: &str) {
        if !name.is_empty() {
            self.tags.insert(name.to_owned());
        }
        if name.contains(':') {
            return;
        }
        if let Some((object, member)) = name.split_once('.') {
            let member = member.split('.').next().unwrap_or(member);
            self.member(object, member);
            return;
        }
        // Vue resolves a tag to a script binding the way it resolves a registered component; Svelte and Astro read a
        // custom element's name in both cases.
        if self.flavor == Flavor::Vue || name.contains('-') {
            for spelling in resolved_names(name) {
                self.bare(&spelling);
            }
        } else {
            self.bare(name);
        }
    }

    /// The attributes of the start tag whose name ends at `from`, the index after its `>`, and whether it closes
    /// itself (`<Card />`).
    fn attributes(&self, from: usize) -> (Vec<Attribute<'t>>, usize, bool) {
        let text = self.text;
        let bytes = text.as_bytes();
        let mut found = Vec::new();
        let mut slash = false;
        let mut i = from;
        while i < bytes.len() {
            match bytes[i] {
                b'>' => return (found, i + 1, slash),
                b'/' => {
                    slash = true;
                    i += 1;
                }
                byte if byte.is_ascii_whitespace() => i += 1,
                b'{' => {
                    let close = matching_brace(bytes, i);
                    found.push(Attribute {
                        name: "",
                        value: Some(Value::Braced(&text[i + 1..close])),
                    });
                    i = (close + 1).min(bytes.len());
                    slash = false;
                }
                _ => {
                    let (attribute, next) = self.attribute_at(i);
                    found.push(attribute);
                    i = next;
                    slash = false;
                }
            }
        }
        (found, bytes.len(), slash)
    }

    /// The attribute that starts at `start` and the index after it.
    fn attribute_at(&self, start: usize) -> (Attribute<'t>, usize) {
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
        if bytes.get(after_name) != Some(&b'=') {
            return (Attribute { name, value: None }, i.max(start + 1));
        }
        let mut value = after_name + 1;
        while value < bytes.len() && bytes[value].is_ascii_whitespace() {
            value += 1;
        }
        match bytes.get(value) {
            Some(b'{') => {
                let close = matching_brace(bytes, value);
                let attribute = Attribute {
                    name,
                    value: Some(Value::Braced(&text[value + 1..close])),
                };
                (attribute, (close + 1).min(bytes.len()))
            }
            Some(b'"' | b'\'') => {
                let end = skip_attribute_value(bytes, value);
                let closed = end > value + 1 && bytes[end - 1] == bytes[value];
                let inner = text
                    .get(value + 1..if closed { end - 1 } else { end })
                    .unwrap_or_default();
                let attribute = Attribute {
                    name,
                    value: Some(Value::Quoted(inner)),
                };
                (attribute, end)
            }
            Some(_) => {
                let mut end = value;
                while end < bytes.len() && !bytes[end].is_ascii_whitespace() && bytes[end] != b'>' {
                    end += 1;
                }
                let attribute = Attribute {
                    name,
                    value: Some(Value::Bare(&text[value..end])),
                };
                (attribute, end)
            }
            None => (Attribute { name, value: None }, bytes.len()),
        }
    }

    /// Where the template locals the attribute `name` binds are visible, if it binds any.
    fn reach(&self, name: &str) -> Option<Reach> {
        match self.flavor {
            Flavor::Vue if name == "v-for" => Some(Reach::Element),
            Flavor::Vue
                if name == "v-slot" || name.starts_with("v-slot:") || name.starts_with('#') || name == "slot-scope" =>
            {
                Some(Reach::Content)
            }
            Flavor::Svelte if name.starts_with("let:") => Some(Reach::Content),
            _ => None,
        }
    }

    /// The template locals a binding attribute declares, after recording the uses of its value (the source of a
    /// `v-for`, default values of slot props).
    fn bind(&mut self, attribute: &Attribute<'t>) -> Vec<String> {
        let value = attribute.value.as_ref().map(Value::code);
        if attribute.name == "v-for" {
            let Some(value) = value else {
                return Vec::new();
            };
            return match split_v_for(value) {
                Some((alias, source)) => {
                    self.expression(source);
                    self.parameters(alias)
                }
                None => {
                    self.expression(value);
                    Vec::new()
                }
            };
        }
        match (attribute.name.strip_prefix("let:"), value) {
            (Some(local), None) => vec![local.to_owned()],
            (_, Some(value)) => self.parameters(value),
            (None, None) => Vec::new(),
        }
    }

    /// Records the uses of an attribute that binds nothing.
    fn attribute(&mut self, attribute: &Attribute<'t>) {
        let name = attribute.name;
        self.attribute_name(name, attribute.value.is_some());
        let handler = self.flavor == Flavor::Vue && (name.starts_with('@') || name == "v-on" || name.starts_with("v-on:"));
        match &attribute.value {
            Some(Value::Braced(code)) => self.expression(code),
            Some(Value::Quoted(code) | Value::Bare(code)) if self.is_expression_attribute(name) => {
                if handler {
                    self.handler(code);
                } else {
                    self.expression(code);
                }
            }
            Some(Value::Quoted(inner)) if self.flavor != Flavor::Vue => self.interpolations(inner),
            _ => {}
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
                    self.bare(&format!("v{}", capitalize(&camelize(directive))));
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

    /// Reads the content of a Svelte `{…}`: a block tag (`#each`, `:else`, `/if`), a special tag (`@html`,
    /// `@const`) or an expression.
    fn svelte_tag(&mut self, content: &str) {
        let content = content.trim_start();
        if let Some(rest) = content.strip_prefix('#') {
            let (keyword, body) = keyword(rest);
            self.open_block(keyword, body);
        } else if let Some(rest) = content.strip_prefix(':') {
            let (keyword, body) = keyword(rest);
            self.branch(keyword, body);
        } else if content.starts_with('/') {
            if let Some(at) = self.scopes.iter().rposition(|frame| frame.element.is_none()) {
                self.scopes.truncate(at);
            }
        } else if let Some(rest) = content.strip_prefix('@') {
            let (keyword, body) = keyword(rest);
            if keyword == "const" {
                let names = self.bindings(rest, Binds::Declarations);
                if let Some(frame) = self.scopes.last_mut() {
                    frame.names.extend(names);
                }
            } else {
                self.expression(body);
            }
        } else {
            self.expression(content);
        }
    }

    /// Opens the scope of a Svelte block: `{#each}`, `{#await}`, `{#snippet}`, `{#if}`, `{#key}`.
    fn open_block(&mut self, keyword: &str, body: &str) {
        let names = match keyword {
            "each" => {
                let Some((source, context)) = split_word(body, "as", true) else {
                    self.expression(body);
                    self.scopes.push(Frame::block(Vec::new()));
                    return;
                };
                self.expression(source);
                let (pattern, key) = split_key(context);
                let names = self.parameters(pattern);
                self.scopes.push(Frame::block(names));
                if let Some(key) = key {
                    self.expression(key);
                }
                return;
            }
            "await" => {
                let (promise, value) = split_word(body, "then", false)
                    .or_else(|| split_word(body, "catch", false))
                    .map_or((body, None), |(promise, value)| (promise, Some(value)));
                self.expression(promise);
                value.map_or_else(Vec::new, |value| self.parameters(value))
            }
            "snippet" => body.split_once('(').map_or_else(Vec::new, |(_, parameters)| {
                let parameters = parameters.trim_end();
                self.parameters(parameters.strip_suffix(')').unwrap_or(parameters))
            }),
            _ => {
                self.expression(body);
                Vec::new()
            }
        };
        self.scopes.push(Frame::block(names));
    }

    /// A branch of the innermost Svelte block replaces its locals: `{:then value}`, `{:catch error}`, `{:else}`.
    fn branch(&mut self, keyword: &str, body: &str) {
        let names = match keyword {
            "then" | "catch" => self.parameters(body),
            _ => Vec::new(),
        };
        if let Some(frame) = self.scopes.iter_mut().rev().find(|frame| frame.element.is_none()) {
            frame.names = names;
        }
        if keyword == "else" {
            let (word, condition) = keyword_of(body);
            if word == "if" {
                self.expression(condition);
            }
        }
    }

    /// The names the parameter list `code` binds (`item, index`, `{ id, name }`), after recording the uses of its
    /// default values.
    fn parameters(&mut self, code: &str) -> Vec<String> {
        if code.trim().is_empty() {
            return Vec::new();
        }
        self.bindings(&format!("({code}) => 0"), Binds::Parameters)
    }

    /// [`analyse`] in the scan's arena, which the previous program no longer needs.
    fn analyse(&mut self, code: &str, binds: Binds) -> Option<(Vec<Found>, Vec<String>)> {
        let source_type = self.source_type();
        self.allocator.reset();
        analyse(&self.allocator, code, source_type, binds)
    }

    /// The names the program `code` binds, after recording its uses. Code oxc cannot parse binds nothing.
    fn bindings(&mut self, code: &str, binds: Binds) -> Vec<String> {
        if let Some((found, names)) = self.analyse(code, binds) {
            self.credit(found);
            names
        } else {
            self.tokens(code);
            Vec::new()
        }
    }

    /// Records the uses of the JavaScript expression `code`. A leading `...` is a spread attribute.
    fn expression(&mut self, code: &str) {
        self.read(code, false);
    }

    /// Records the uses of a Vue event handler: statements (`count++; save()`), which Vue wraps in a function, or a
    /// function expression.
    fn handler(&mut self, code: &str) {
        self.read(code, !is_function_handler(code.trim()));
    }

    /// Records the uses of `code`, parsed once: as statements or as an expression.
    fn read(&mut self, code: &str, statements: bool) {
        let code = code.trim();
        let code = code.strip_prefix("...").unwrap_or(code);
        if code.is_empty() {
            return;
        }
        if is_path(code) {
            self.tokens(code);
            return;
        }
        let parsed = if statements {
            self.analyse(code, Binds::Nothing)
        } else {
            self.analyse(&format!("({code}\n)"), Binds::Nothing)
        };
        if let Some((found, _)) = parsed {
            self.credit(found);
        } else {
            self.tokens(code);
        }
    }

    /// Records every identifier of `code` and the member read right after it, token by token.
    fn tokens(&mut self, code: &str) {
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
                    self.tokens(&code[i + 2..close]);
                    i = close + 1;
                }
                _ => i += 1,
            }
        }
        bytes.len()
    }
}

/// [`keyword`] of `code` after its leading whitespace.
fn keyword_of(code: &str) -> (&str, &str) {
    keyword(code.trim_start())
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

    /// Asserts that each of `used` is used in `text` and none of `bound` is.
    fn assert_scoped(text: &str, flavor: Flavor, used_names: &[&str], bound: &[&str]) {
        let uses = used(text, flavor);
        for name in used_names {
            assert!(uses.contains_key(*name), "`{name}` is used in {text}");
        }
        for name in bound {
            assert!(!uses.contains_key(*name), "`{name}` is a template local in {text}");
        }
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

    #[test]
    fn vue_template_locals_are_no_uses() {
        let cases: &[(&str, &[&str], &[&str])] = &[
            (
                "<li v-for=\"(item, index) in items\" :key=\"item.id\">{{ item.name }} {{ index }}</li>",
                &["items"],
                &["item", "index"],
            ),
            (
                "<li :title=\"entry\" v-for=\"{ entry } of entries\"/>",
                &["entries"],
                &["entry"],
            ),
            // The scope ends with the element: `item` after `</li>` is a use.
            (
                "<ul><li v-for=\"item in items\"><b>{{ item }}</b></li></ul><p>{{ item }}</p>",
                &["items", "item"],
                &[],
            ),
            (
                "<DataTable><template #row=\"{ row, cells = defaults }\">{{ row.label }}</template></DataTable>",
                &["DataTable", "defaults"],
                &["row", "cells"],
            ),
            (
                "<Pager v-slot=\"page\">{{ page.number }}</Pager>",
                &["Pager"],
                &["page"],
            ),
            ("<Pager v-slot=\"page\"/><p>{{ page }}</p>", &["Pager", "page"], &[]),
            ("<old slot-scope=\"scope\">{{ scope.row }}</old>", &[], &["scope"]),
            (
                "<p :title=\"list.map((entry) => entry.id)\" @click=\"count++; save(entry)\"/>",
                &["list", "count", "save", "entry"],
                &[],
            ),
            ("<p :title=\"list.map((entry) => entry.id)\"/>", &["list"], &["entry"]),
        ];
        for (text, used_names, bound) in cases {
            assert_scoped(text, Flavor::Vue, used_names, bound);
        }
    }

    #[test]
    fn svelte_template_locals_are_no_uses() {
        let cases: &[(&str, &[&str], &[&str])] = &[
            (
                "{#each rows as row, i (row.id)}<p>{row.name}{i}</p>{/each}",
                &["rows"],
                &["row", "i"],
            ),
            (
                "{#each rows as { id, label }}{id}{label}{/each}",
                &["rows"],
                &["id", "label"],
            ),
            ("{#each rows as row}{/each}{row}", &["rows", "row"], &[]),
            ("{#each rows as row}{:else}{row}{/each}", &["rows", "row"], &[]),
            (
                "{#await load() then data}{data.x}{:catch failure}{failure}{/await}",
                &["load"],
                &["data", "failure"],
            ),
            (
                "{#await load()}{:then data}{data}{:catch failure}{failure.message}{/await}",
                &["load"],
                &["data", "failure"],
            ),
            ("<List let:cell>{cell}</List>", &["List"], &["cell"]),
            ("<List let:cell={item}>{item}</List>{item}", &["List", "item"], &[]),
            ("<List let:cell={item}>{item}</List>", &["List"], &["item", "cell"]),
            (
                "{#snippet field(value, extra)}{value}{extra}{/snippet}",
                &[],
                &["value", "extra"],
            ),
            (
                "{#if ok}{@const total = sum(rows)}{total}{/if}",
                &["ok", "sum", "rows"],
                &["total"],
            ),
            ("{rows.map((row) => row.id)}", &["rows"], &["row"]),
        ];
        for (text, used_names, bound) in cases {
            assert_scoped(text, Flavor::Svelte, used_names, bound);
        }
    }

    #[test]
    fn astro_template_locals_are_no_uses() {
        let cases: &[(&str, &[&str], &[&str])] = &[
            (
                "<ul>{items.map((item) => <Card data={item} title={item.title} />)}</ul>",
                &["items", "Card"],
                &["item"],
            ),
            (
                "{posts.map(({ slug }, index) => <a href={slug}>{index}</a>)}<p>{slug}</p>",
                &["posts", "slug"],
                &["index"],
            ),
            ("{groups.map((group) => <group.Icon />)}", &["groups"], &["group"]),
        ];
        for (text, used_names, bound) in cases {
            assert_scoped(text, Flavor::Astro, used_names, bound);
        }
    }

    #[test]
    fn unparsable_expressions_are_read_token_by_token() {
        let uses = used("<p :title=\"broken ( value\"/>", Flavor::Vue);
        assert!(bare(&uses, "broken"));
        assert!(bare(&uses, "value"));
    }

    #[test]
    fn scan_returns_the_tag_names_as_written() {
        let mut uses = Uses::new();
        let tags = scan(
            "<template><AppButton/><app-card :x=\"1\"></app-card><div/></template>",
            Flavor::Vue,
            &mut uses,
        );
        let tags: Vec<&str> = tags.iter().map(String::as_str).collect();
        assert_eq!(tags, ["AppButton", "app-card", "div", "template"]);
    }

    #[test]
    fn vue_constructs_around_components_and_malformed_markup() {
        let cases: &[(&str, &[&str], &[&str])] = &[
            (
                "<AppItem v-for=\"item in items\" :item=\"item\" /><p>{{ count }}</p>",
                &["AppItem", "items", "count"],
                &["item"],
            ),
            // An element of the same name inside the one that binds closes first.
            (
                "<li v-for=\"item in items\"><ul><li>a</li></ul>{{ item }}</li>",
                &["items"],
                &["item"],
            ),
            // An element left open inside one that binds does not keep its locals visible after it.
            (
                "<div v-for=\"item in items\"><p>unclosed</div><p>{{ item }}</p>",
                &["items", "item"],
                &[],
            ),
            (
                "<Wrap><Icon/><template v-slot:item=\"{ entry }\">{{ entry.id }}</template><Badge/></Wrap>",
                &["Wrap", "Icon", "Badge"],
                &["entry"],
            ),
            (
                "<Wrap><template #item=\"{ entry }\"><Badge/></template><Icon/>{{ entry }}</Wrap>",
                &["Wrap", "Icon", "Badge", "entry"],
                &[],
            ),
        ];
        for (text, used_names, bound) in cases {
            assert_scoped(text, Flavor::Vue, used_names, bound);
        }
    }

    #[test]
    fn svelte_else_if_keeps_the_locals_of_the_enclosing_block() {
        assert_scoped(
            "{#each rows as row}{#if row.a}{:else if ready}{row.c}{/if}{/each}",
            Flavor::Svelte,
            &["rows", "ready"],
            &["row"],
        );
    }

    #[test]
    fn a_pattern_oxc_cannot_parse_binds_nothing() {
        // The pattern is read token by token, so `value` is used once there and once in the content.
        let uses = used("<Pager v-slot=\"broken ( value\">{{ value }}</Pager>", Flavor::Vue);
        assert_eq!(uses.get("value").map(|slot| slot.uses), Some(2));
        assert!(bare(&uses, "broken"));
    }

    #[test]
    fn a_backslash_in_an_attribute_value_escapes_nothing() {
        let uses = used("<p title=\"C:\\\" :alt=\"label\"/>", Flavor::Vue);
        assert!(bare(&uses, "label"));
    }

    #[test]
    fn vue_handlers_are_statements_or_functions() {
        let uses = used(
            "<p @click=\"let next = step + 1; save(next)\" v-on:keyup=\"function (event) { press(event) }\" @focus=\"(event) => focus(event)\"/>",
            Flavor::Vue,
        );
        for name in ["step", "save", "press", "focus"] {
            assert!(bare(&uses, name), "{name}");
        }
        for name in ["next", "event"] {
            assert!(!uses.contains_key(name), "{name}");
        }
    }

    #[test]
    fn static_is_attributes_name_components() {
        let mut uses = Uses::new();
        let tags = scan(
            "<component is=\"AppButton\"/><component :is=\"'app-card'\"/><div is=\"vue:AppMenu\"></div><component :is=\"current\"/><component v-bind:is=\"`App${x}`\"/>",
            Flavor::Vue,
            &mut uses,
        );
        let tags: Vec<&str> = tags.iter().map(String::as_str).collect();
        assert_eq!(tags, ["AppButton", "AppMenu", "app-card", "component", "div"]);
        assert!(bare(&uses, "current"));
    }

    #[test]
    fn tags_and_directives_credit_the_names_vue_resolves_them_to() {
        let uses = used("<myButton/><u-i-button/><p v-my-dir/>", Flavor::Vue);
        for name in ["myButton", "MyButton", "u-i-button", "uIButton", "UIButton", "vMyDir"] {
            assert!(bare(&uses, name), "{name}");
        }
    }
}
