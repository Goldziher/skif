//! The comments of a multi-line TOML array, assigned to the entries they annotate.
//!
//! `toml_edit` keeps the text around an entry in three places depending on the commas: the entry's suffix (before its
//! comma), the prefix of the next entry (after the comma) and, for the last entry of an array with a trailing comma,
//! `Array::trailing`. A comment on the same line as an entry belongs to that entry wherever it sits; comment lines on
//! their own above an entry belong to the entry below; comment lines after the last entry belong to nobody and stay
//! at the end. [`Layout`] reads an array into that model, lets entries be added and replaced, and writes the array
//! back, so a change never moves a comment to another entry.

use toml_edit::{Array, Value};

/// An entry with the comments that belong to it.
struct Slot {
    value: Value,
    /// Lines above the entry: comments (starting with `#`) and blank lines (empty).
    lead: Vec<String>,
    /// The comment on the line of the entry.
    same_line: Option<String>,
}

/// An array read into [`Slot`]s plus what is needed to write it out in the style it had.
pub(super) struct Layout {
    slots: Vec<Slot>,
    /// Comment lines after the last entry.
    dangling: Vec<String>,
    multiline: bool,
    indent: String,
    close_indent: String,
    trailing_comma: bool,
    /// For an array on one line: what precedes the first entry, what separates entries and what follows the last.
    first_prefix: String,
    separator: String,
    trailing: String,
}

/// The text before the first line break of `text`, and the text after it when there is one.
fn first_line(text: &str) -> (&str, Option<&str>) {
    match text.split_once('\n') {
        Some((head, rest)) => (head, Some(rest)),
        None => (text, None),
    }
}

fn comment(text: &str) -> Option<String> {
    let text = text.trim();
    text.starts_with('#').then(|| text.to_owned())
}

/// The lines of `rest` (the text after a line break) before its last line, and that last line: the indentation of
/// whatever follows. A line that is not a comment is a blank one.
fn lines_and_indent(rest: &str) -> (Vec<String>, &str) {
    let mut pieces: Vec<&str> = rest.split('\n').collect();
    let indent = pieces.pop().unwrap_or("");
    let lines = pieces
        .into_iter()
        .map(|line| comment(line).unwrap_or_default())
        .collect();
    (lines, indent)
}

fn decor_text(text: Option<&toml_edit::RawString>) -> &str {
    text.and_then(|raw| raw.as_str()).unwrap_or("")
}

impl Layout {
    pub(super) fn read(array: &Array) -> Self {
        let mut layout = Self {
            slots: Vec::new(),
            dangling: Vec::new(),
            multiline: false,
            indent: "  ".to_owned(),
            close_indent: String::new(),
            trailing_comma: array.trailing_comma(),
            first_prefix: String::new(),
            separator: " ".to_owned(),
            trailing: array.trailing().as_str().unwrap_or("").to_owned(),
        };
        let mut carried: Vec<String> = Vec::new();
        for (index, value) in array.iter().enumerate() {
            let (head, rest) = first_line(decor_text(value.decor().prefix()));
            let mut lead = std::mem::take(&mut carried);
            if let Some(found) = comment(head) {
                layout.attach(found, &mut lead);
            }
            if let Some(rest) = rest {
                let (lines, indent) = lines_and_indent(rest);
                lead.extend(lines);
                layout.multiline = true;
                indent.clone_into(&mut layout.indent);
            } else if index == 0 {
                head.clone_into(&mut layout.first_prefix);
            } else if index == 1 {
                head.clone_into(&mut layout.separator);
            }
            let (head, rest) = first_line(decor_text(value.decor().suffix()));
            if let Some(rest) = rest {
                let (lines, indent) = lines_and_indent(rest);
                layout.multiline = true;
                if index + 1 == array.len() {
                    indent.clone_into(&mut layout.close_indent);
                }
                carried = lines;
            }
            let mut plain = value.clone();
            plain.decor_mut().clear();
            layout.slots.push(Slot {
                value: plain,
                lead,
                same_line: comment(head),
            });
        }
        layout.read_end(carried);
        layout
    }

    /// Puts `found`, a comment after the comma of the last entry read, on that entry's line; when that entry
    /// already has a comment there, it goes to `own_line` instead, as a comment line of its own.
    fn attach(&mut self, found: String, own_line: &mut Vec<String>) {
        match self.slots.last_mut() {
            Some(last) if last.same_line.is_none() => last.same_line = Some(found),
            _ => own_line.push(found),
        }
    }

    /// What follows the last entry: the comments after the closing comma and the whitespace before `]`.
    fn read_end(&mut self, mut dangling: Vec<String>) {
        let trailing = std::mem::take(&mut self.trailing);
        let (head, rest) = first_line(&trailing);
        if let Some(found) = comment(head) {
            self.attach(found, &mut dangling);
        }
        if let Some(rest) = rest {
            let (lines, indent) = lines_and_indent(rest);
            dangling.extend(lines);
            self.multiline = true;
            self.close_indent = indent.to_owned();
        }
        self.dangling = dangling;
        self.trailing = if self.slots.is_empty() || self.multiline {
            String::new()
        } else {
            trailing
        };
    }

    /// Adds an entry at the end.
    pub(super) fn push(&mut self, value: Value) {
        if self.slots.is_empty() && self.multiline {
            self.trailing_comma = true;
        }
        self.slots.push(Slot {
            value,
            lead: Vec::new(),
            same_line: None,
        });
    }

    /// Replaces the entry at `first` by `value` and removes the entries at `rest` (after `first`, ascending). The
    /// comments of the removed entries move to the replacement, the entry that stands for them.
    pub(super) fn replace(&mut self, first: usize, rest: &[usize], value: Value) {
        let mut folded: Vec<String> = Vec::new();
        for &index in rest {
            if let Some(slot) = self.slots.get(index) {
                folded.extend(slot.lead.iter().cloned());
                folded.extend(slot.same_line.iter().cloned());
            }
        }
        for &index in rest.iter().rev() {
            self.slots.remove(index);
        }
        if let Some(slot) = self.slots.get_mut(first) {
            slot.value = value;
            slot.lead.extend(folded);
        }
    }

    /// Writes the entries and their comments into `array`.
    pub(super) fn write(mut self, array: &mut Array) {
        array.clear();
        let multiline = self.multiline;
        let slots = std::mem::take(&mut self.slots);
        let count = slots.len();
        let mut previous: Option<String> = None;
        let mut closing: Option<String> = None;
        for (index, slot) in slots.into_iter().enumerate() {
            let mut value = slot.value;
            let prefix = if multiline {
                let mut text = previous.take().map_or_else(String::new, |found| format!(" {found}"));
                text.push('\n');
                text.push_str(&own_lines(&slot.lead, &self.indent));
                text.push_str(&self.indent);
                text
            } else if index == 0 {
                self.first_prefix.clone()
            } else {
                self.separator.clone()
            };
            value.decor_mut().set_prefix(prefix);
            value.decor_mut().set_suffix("");
            if index + 1 == count {
                closing = slot.same_line;
            } else {
                previous = slot.same_line;
            }
            array.push_formatted(value);
        }
        self.finish(array, count, closing);
    }

    /// The comma, the comment on the last entry's line and the closing bracket.
    fn finish(&self, array: &mut Array, count: usize, closing: Option<String>) {
        array.set_trailing_comma(self.trailing_comma && count > 0);
        if !self.multiline {
            array.set_trailing(if count == 0 {
                String::new()
            } else {
                self.trailing.clone()
            });
            return;
        }
        let mut trailing = String::new();
        let comment = closing.map_or_else(String::new, |found| format!(" {found}"));
        if self.trailing_comma && count > 0 {
            trailing.push_str(&comment);
        } else if let Some(last) = count.checked_sub(1).and_then(|index| array.get_mut(index)) {
            last.decor_mut().set_suffix(comment);
        }
        trailing.push('\n');
        trailing.push_str(&own_lines(&self.dangling, &self.indent));
        trailing.push_str(&self.close_indent);
        array.set_trailing(trailing);
    }
}

/// `lines` one per line at `indent`, each ending in a line break; a blank line has no indentation.
fn own_lines(lines: &[String], indent: &str) -> String {
    lines
        .iter()
        .map(|line| {
            if line.is_empty() {
                "\n".to_owned()
            } else {
                format!("{indent}{line}\n")
            }
        })
        .collect()
}
