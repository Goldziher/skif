//! A reader for manifests written as strict JSON without escape sequences, which most are.
//!
//! It builds the value and hands the dependency sections to the [`Collector`] in one pass over the text, without the
//! syntax tree `jsonc-parser` builds first: on a workspace of thousands of packages that tree was most of the time
//! spent reading manifests. Anything else (a comment, a trailing comma, an escape sequence, a control character in a
//! string, a document nested deeper than [`MAX_DEPTH`], an error) makes it give up, and the manifest is read again by
//! `jsonc-parser`, so the value, the entries and the errors are always the ones that reader gives.

use jsonc_parser::common::Range;
use serde_json::{Map, Value};
use skif_core::DependencyKind;

use super::Collector;

/// Deeper documents are left to `jsonc-parser`.
const MAX_DEPTH: usize = 64;

/// The value of `text` (a manifest after its BOM) and its dependency entries handed to `collector`, or `None` when
/// the text is not a JSON object this reader takes; `collector` is then left in an unspecified state.
pub(super) fn parse(text: &str, collector: &mut Collector<'_>) -> Option<Value> {
    let mut reader = Reader {
        text,
        pos: 0,
        members: Vec::new(),
        items: Vec::new(),
    };
    reader.skip_whitespace();
    if reader.peek()? != b'{' {
        return None;
    }
    let value = reader.object(0, Some(collector))?;
    reader.skip_whitespace();
    (reader.pos == text.len()).then_some(value)
}

struct Reader<'s> {
    text: &'s str,
    pos: usize,
    /// The members of the objects being read, innermost last: each object is built from its own once complete, so
    /// its map is allocated once at its final size instead of growing member by member.
    members: Vec<(String, Value)>,
    /// The same for the elements of arrays.
    items: Vec<Value>,
}

impl<'s> Reader<'s> {
    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.pos).copied()
    }

    fn skip_whitespace(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.pos += 1;
        }
    }

    fn eat(&mut self, byte: u8) -> Option<()> {
        (self.peek()? == byte).then(|| self.pos += 1)
    }

    /// Whether the token that just ended is followed by whitespace, a delimiter or the end: `truex` and `01` are not
    /// JSON.
    fn at_delimiter(&self) -> bool {
        matches!(
            self.peek(),
            None | Some(b' ' | b'\t' | b'\n' | b'\r' | b',' | b'}' | b']')
        )
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > MAX_DEPTH {
            return None;
        }
        match self.peek()? {
            b'"' => Some(Value::String(self.string()?.0.to_owned())),
            b'{' => self.object(depth, None),
            b'[' => self.array(depth),
            b't' => self.word("true", Value::Bool(true)),
            b'f' => self.word("false", Value::Bool(false)),
            b'n' => self.word("null", Value::Null),
            b'-' | b'0'..=b'9' => self.number(),
            _ => None,
        }
    }

    /// A string without escape sequences or control characters: its contents and its range, quotes included.
    fn string(&mut self) -> Option<(&'s str, Range)> {
        let start = self.pos;
        self.eat(b'"')?;
        let rest = &self.text.as_bytes()[self.pos..];
        let len = rest
            .iter()
            .position(|&byte| byte == b'"' || byte == b'\\' || byte < 0x20)?;
        if rest[len] != b'"' {
            return None;
        }
        let contents = &self.text[self.pos..self.pos + len];
        self.pos += len + 1;
        Some((contents, Range::new(start, self.pos)))
    }

    fn word(&mut self, word: &str, value: Value) -> Option<Value> {
        if !self.text[self.pos..].starts_with(word) {
            return None;
        }
        self.pos += word.len();
        self.at_delimiter().then_some(value)
    }

    fn digits(&mut self) -> usize {
        let start = self.pos;
        while let Some(b'0'..=b'9') = self.peek() {
            self.pos += 1;
        }
        self.pos - start
    }

    /// A number as JSON writes one, converted as `jsonc-parser`'s number literal is: by `serde_json`, `null` when it
    /// is out of range.
    fn number(&mut self) -> Option<Value> {
        let start = self.pos;
        if self.peek()? == b'-' {
            self.pos += 1;
        }
        match self.peek()? {
            b'0' => self.pos += 1,
            b'1'..=b'9' => {
                self.digits();
            }
            _ => return None,
        }
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if self.digits() == 0 {
                return None;
            }
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.pos += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.pos += 1;
            }
            if self.digits() == 0 {
                return None;
            }
        }
        if !self.at_delimiter() {
            return None;
        }
        Some(serde_json::from_str(&self.text[start..self.pos]).unwrap_or(Value::Null))
    }

    fn array(&mut self, depth: usize) -> Option<Value> {
        self.eat(b'[')?;
        let start = self.items.len();
        self.skip_whitespace();
        if self.peek()? != b']' {
            loop {
                self.skip_whitespace();
                let item = self.value(depth + 1)?;
                self.items.push(item);
                self.skip_whitespace();
                match self.peek()? {
                    b',' => self.pos += 1,
                    b']' => break,
                    _ => return None,
                }
            }
        }
        self.pos += 1;
        Some(Value::Array(self.items.drain(start..).collect()))
    }

    /// An object; the root object is read with the `collector`, which receives its dependency sections.
    fn object(&mut self, depth: usize, mut collector: Option<&mut Collector<'_>>) -> Option<Value> {
        self.eat(b'{')?;
        let start = self.members.len();
        self.skip_whitespace();
        if self.peek()? == b'}' {
            self.pos += 1;
            return Some(Value::Object(Map::new()));
        }
        loop {
            self.skip_whitespace();
            let (name, _) = self.string()?;
            self.skip_whitespace();
            self.eat(b':')?;
            self.skip_whitespace();
            let section = collector
                .as_deref_mut()
                .and_then(|collector| Some((DependencyKind::from_field(name)?, collector)));
            let value = match section {
                Some((kind, collector)) => {
                    collector.section(kind);
                    if self.peek()? == b'{' {
                        self.section(kind, depth + 1, collector)?
                    } else {
                        self.value(depth + 1)?
                    }
                }
                None => self.value(depth + 1)?,
            };
            self.members.push((name.to_owned(), value));
            self.skip_whitespace();
            match self.peek()? {
                b',' => self.pos += 1,
                b'}' => {
                    self.pos += 1;
                    return Some(self.complete(start));
                }
                _ => return None,
            }
        }
    }

    /// The object of the members from `start` on, inserted in order as `serde_json` collects them: a key written
    /// twice keeps its first position and its last value.
    fn complete(&mut self, start: usize) -> Value {
        Value::Object(self.members.drain(start..).collect())
    }

    /// A dependency section object: its string members are the entries of the section `kind`.
    fn section(&mut self, kind: DependencyKind, depth: usize, collector: &mut Collector<'_>) -> Option<Value> {
        self.eat(b'{')?;
        let start = collector.entries.len();
        let members = self.members.len();
        self.skip_whitespace();
        if self.peek()? != b'}' {
            loop {
                self.skip_whitespace();
                let (name, key) = self.string()?;
                self.skip_whitespace();
                self.eat(b':')?;
                self.skip_whitespace();
                let value = if self.peek()? == b'"' {
                    let (specifier, range) = self.string()?;
                    collector.entry(kind, name, key, specifier, range);
                    Value::String(specifier.to_owned())
                } else {
                    self.value(depth + 1)?
                };
                self.members.push((name.to_owned(), value));
                self.skip_whitespace();
                match self.peek()? {
                    b',' => self.pos += 1,
                    b'}' => break,
                    _ => return None,
                }
            }
        }
        self.pos += 1;
        collector.end_section(start);
        Some(self.complete(members))
    }
}
