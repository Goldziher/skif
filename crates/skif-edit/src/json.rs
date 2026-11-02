//! JSON and JSONC edits, located with `jsonc-parser` byte ranges.

use std::collections::{BTreeMap, HashMap};

use jsonc_parser::ast::{Object, ObjectProp, ObjectPropName, Value};
use jsonc_parser::common::Ranged;
use jsonc_parser::{CollectOptions, ParseOptions, parse_to_ast};
use skif_core::{FixOp, InsertPosition, JsonAction, PathSeg};

use crate::describe;
use crate::{EditPlanError, FileKind, Planned};

/// Indentation unit and line ending used when text has to be generated.
struct Style {
    unit: String,
    newline: &'static str,
}

impl Style {
    fn detect(source: &str) -> Self {
        let unit = source
            .lines()
            .filter(|line| !line.trim().is_empty())
            .find_map(|line| {
                let width = line.len() - line.trim_start_matches([' ', '\t']).len();
                let indent = &line[..width];
                (width > 0).then(|| {
                    if indent.starts_with('\t') {
                        "\t".to_owned()
                    } else {
                        indent.to_owned()
                    }
                })
            })
            .unwrap_or_else(|| "  ".to_owned());
        let newline = if source.contains("\r\n") { "\r\n" } else { "\n" };
        Self { unit, newline }
    }
}

struct Member<'a, 'b> {
    start: usize,
    end: usize,
    key: Option<&'b str>,
    /// Byte range of the key; empty for array elements.
    name: (usize, usize),
    value: &'b Value<'a>,
}

/// An object or array together with the byte offsets of its brackets and members.
struct Container<'a, 'b> {
    open: usize,
    close: usize,
    is_object: bool,
    members: Vec<Member<'a, 'b>>,
}

fn prop_key<'b>(prop: &'b ObjectProp<'_>) -> &'b str {
    match &prop.name {
        ObjectPropName::String(lit) => lit.value.as_ref(),
        ObjectPropName::Word(lit) => lit.value,
    }
}

fn object_container<'a, 'b>(object: &'b Object<'a>) -> Container<'a, 'b> {
    Container {
        open: object.range.start,
        close: object.range.end.saturating_sub(1),
        is_object: true,
        members: object
            .properties
            .iter()
            .map(|prop| Member {
                start: prop.range.start,
                end: prop.range.end,
                key: Some(prop_key(prop)),
                name: (prop.name.start(), prop.name.end()),
                value: &prop.value,
            })
            .collect(),
    }
}

fn container<'a, 'b>(value: &'b Value<'a>) -> Option<Container<'a, 'b>> {
    match value {
        Value::Object(object) => Some(object_container(object)),
        Value::Array(array) => Some(Container {
            open: array.range.start,
            close: array.range.end.saturating_sub(1),
            is_object: false,
            members: array
                .elements
                .iter()
                .map(|element| Member {
                    start: element.start(),
                    end: element.end(),
                    key: None,
                    name: (element.start(), element.start()),
                    value: element,
                })
                .collect(),
        }),
        _ => None,
    }
}

/// A node found by path, with the container that holds it.
struct Located<'a, 'b> {
    value: &'b Value<'a>,
    parent: Option<(Container<'a, 'b>, usize)>,
}

fn locate<'a, 'b>(root: &'b Value<'a>, path: &[PathSeg]) -> Option<Located<'a, 'b>> {
    path.iter().try_fold(
        Located {
            value: root,
            parent: None,
        },
        |current, seg| {
            let holder = container(current.value)?;
            let index = match seg {
                PathSeg::Key(key) => holder
                    .members
                    .iter()
                    .rposition(|member| member.key == Some(key.as_str()))?,
                PathSeg::Index(index) => (!holder.is_object && *index < holder.members.len()).then_some(*index)?,
            };
            Some(Located {
                value: holder.members[index].value,
                parent: Some((holder, index)),
            })
        },
    )
}

const BOM: char = '\u{feff}';

/// Parse options for checking edit results: separators between members must be present when the input had them.
fn parse_options(strict_commas: bool) -> ParseOptions {
    ParseOptions {
        allow_missing_commas: !strict_commas,
        ..ParseOptions::default()
    }
}

fn parse_error(error: impl ToString) -> EditPlanError {
    EditPlanError::Parse {
        kind: FileKind::Json,
        message: error.to_string(),
    }
}

/// Checks that `after` is a document as well formed as `before`.
pub(crate) fn validate(before: &str, after: &str) -> Result<(), EditPlanError> {
    let body = |text: &str| text.strip_prefix(BOM).unwrap_or(text).to_owned();
    let (before, after) = (body(before), body(after));
    let strict = parse_to_ast(&before, &CollectOptions::default(), &parse_options(true)).is_ok();
    let parsed =
        parse_to_ast(&after, &CollectOptions::default(), &parse_options(strict)).map_err(|error| {
            EditPlanError::InvalidOutput {
                kind: FileKind::Json,
                message: error.to_string(),
            }
        })?;
    if parsed.value.is_none() {
        return Err(EditPlanError::InvalidOutput {
            kind: FileKind::Json,
            message: "document is empty".to_owned(),
        });
    }
    Ok(())
}

pub(crate) fn plan(source: &str, ops: &[&FixOp]) -> Result<Vec<Planned>, EditPlanError> {
    let (bom, body) = source
        .strip_prefix(BOM)
        .map_or((0, source), |rest| (BOM.len_utf8(), rest));
    let parsed = parse_to_ast(body, &CollectOptions::default(), &ParseOptions::default()).map_err(parse_error)?;
    let root = parsed
        .value
        .ok_or_else(|| parse_error("document is empty"))?;
    let doc = Doc {
        source: body,
        root,
        style: Style::detect(body),
    };
    let mut planned = Vec::new();
    let mut texts = Vec::new();
    let mut removals: HashMap<usize, Removals<'_, '_>> = HashMap::new();
    let mut insertions: HashMap<usize, Insertions<'_, '_>> = HashMap::new();
    for (index, op) in ops.iter().enumerate() {
        let edits = match op {
            FixOp::Json {
                pointer,
                action: JsonAction::Remove,
                ..
            } => {
                doc.queue_removal(&mut removals, &describe::op(op), index, pointer)?;
                Vec::new()
            }
            FixOp::Json {
                pointer,
                action: JsonAction::Insert { key, value, position },
                ..
            } => {
                doc.queue_insertion(&mut insertions, &describe::op(op), index, pointer, (key, value, *position))?;
                Vec::new()
            }
            FixOp::Json { pointer, action, .. } => doc.edits(&describe::op(op), pointer, action)?,
            FixOp::Text { edit, .. } => {
                // Text edits address the whole source, byte order mark included.
                texts.push(Planned {
                    op: index,
                    start: edit.start as usize,
                    end: edit.end as usize,
                    replacement: edit.replacement.clone(),
                });
                Vec::new()
            }
            _ => {
                return Err(EditPlanError::Unsupported {
                    op: describe::op(op),
                    kind: FileKind::Json,
                });
            }
        };
        planned.extend(edits.into_iter().map(|edit| Planned {
            op: index,
            start: edit.start,
            end: edit.end,
            replacement: edit.replacement,
        }));
    }
    for removal in removals.values() {
        planned.extend(doc.removal_edits(removal));
    }
    for insertion in insertions.values() {
        planned.extend(doc.insertion_edits(insertion, removals.get(&insertion.holder.open)));
    }
    for edit in &mut planned {
        edit.start += bom;
        edit.end += bom;
    }
    planned.extend(texts);
    Ok(planned)
}

/// Members of one container queued for removal: member index to the operation that removes it.
struct Removals<'a, 'b> {
    holder: Container<'a, 'b>,
    members: BTreeMap<usize, usize>,
}

/// A key to insert into an object, with the operation that asks for it.
struct PendingInsert {
    op: usize,
    key: String,
    value: serde_json::Value,
    position: InsertPosition,
}

/// Inserts queued for one object, in operation order.
struct Insertions<'a, 'b> {
    holder: Container<'a, 'b>,
    pending: Vec<PendingInsert>,
}

/// Where a key to insert ends up relative to the members of the object that exist already.
enum Slot {
    Existing(usize),
    New(usize),
}

struct Edit {
    start: usize,
    end: usize,
    replacement: String,
}

impl Edit {
    const fn new(start: usize, end: usize, replacement: String) -> Self {
        Self {
            start,
            end,
            replacement,
        }
    }
}

struct Doc<'s, 'a> {
    source: &'s str,
    root: Value<'a>,
    style: Style,
}

impl<'a> Doc<'_, 'a> {
    fn edits(&self, op: &str, pointer: &[PathSeg], action: &JsonAction) -> Result<Vec<Edit>, EditPlanError> {
        let target = locate(&self.root, pointer).ok_or_else(|| EditPlanError::PathNotFound { op: op.to_owned() })?;
        match action {
            JsonAction::Set(value) => Ok(vec![self.set(target.value, value)]),
            JsonAction::SortKeys => {
                let holder = container(target.value)
                    .filter(|holder| holder.is_object)
                    .ok_or_else(|| EditPlanError::WrongNodeKind {
                        op: op.to_owned(),
                        expected: "an object",
                    })?;
                Ok(self.sort_keys(&holder))
            }
            JsonAction::RenameKey(new_key) => Self::rename(op, target, new_key),
            // `Remove` and `Insert` are planned per container; any action added later has no in-place JSON edit.
            _ => Err(EditPlanError::Unsupported {
                op: op.to_owned(),
                kind: FileKind::Json,
            }),
        }
    }

    fn set(&self, target: &Value<'_>, value: &serde_json::Value) -> Edit {
        let indent = line_indent(self.source, target.start());
        Edit::new(target.start(), target.end(), self.render(value, indent))
    }

    fn rename(op: &str, target: Located<'_, '_>, new_key: &str) -> Result<Vec<Edit>, EditPlanError> {
        let Some((holder, index)) = target.parent.filter(|(holder, _)| holder.is_object) else {
            return Err(EditPlanError::WrongNodeKind {
                op: op.to_owned(),
                expected: "an object member",
            });
        };
        let member = &holder.members[index];
        if member.key == Some(new_key) {
            return Ok(Vec::new());
        }
        if holder.members.iter().any(|other| other.key == Some(new_key)) {
            return Err(EditPlanError::DuplicateKey {
                op: op.to_owned(),
                key: new_key.to_owned(),
            });
        }
        let (name_start, name_end) = member.name;
        Ok(vec![Edit::new(name_start, name_end, json_string(new_key))])
    }

    fn sort_keys(&self, holder: &Container<'_, '_>) -> Vec<Edit> {
        let mut order: Vec<usize> = (0..holder.members.len()).collect();
        order.sort_by(|&a, &b| holder.members[a].key.cmp(&holder.members[b].key));
        order
            .iter()
            .enumerate()
            .filter(|&(slot, &from)| slot != from)
            .map(|(slot, &from)| {
                let (target, source) = (&holder.members[slot], &holder.members[from]);
                Edit::new(
                    target.start,
                    target.end,
                    self.source[source.start..source.end].to_owned(),
                )
            })
            .collect()
    }

    fn queue_insertion<'b>(
        &'b self,
        insertions: &mut HashMap<usize, Insertions<'a, 'b>>,
        op: &str,
        op_index: usize,
        pointer: &[PathSeg],
        (key, value, position): (&str, &serde_json::Value, InsertPosition),
    ) -> Result<(), EditPlanError> {
        let target = locate(&self.root, pointer).ok_or_else(|| EditPlanError::PathNotFound { op: op.to_owned() })?;
        let holder = container(target.value)
            .filter(|holder| holder.is_object)
            .ok_or_else(|| EditPlanError::WrongNodeKind {
                op: op.to_owned(),
                expected: "an object",
            })?;
        insertions
            .entry(holder.open)
            .or_insert_with(|| Insertions {
                holder,
                pending: Vec::new(),
            })
            .pending
            .push(PendingInsert {
                op: op_index,
                key: key.to_owned(),
                value: value.clone(),
                position,
            });
        Ok(())
    }

    /// Edits inserting every queued key of one object.
    ///
    /// The inserts are applied in order to the list of members, so the keys end up exactly where the operations asked
    /// for, whatever their number, and all keys that land between the same two members share one edit. Members queued
    /// for removal are skipped: keys go before the next surviving member, or after the last one.
    fn insertion_edits(&self, insertions: &Insertions<'_, '_>, removed: Option<&Removals<'_, '_>>) -> Vec<Planned> {
        let holder = &insertions.holder;
        let count = holder.members.len();
        let gone = |index: usize| removed.is_some_and(|removals| removals.members.contains_key(&index));
        let mut order: Vec<Slot> = (0..count).map(Slot::Existing).collect();
        let mut fresh: Vec<PendingInsert> = Vec::new();
        let mut edits = Vec::new();
        for pending in &insertions.pending {
            if let Some(existing) =
                (0..count).rfind(|&index| !gone(index) && holder.members[index].key == Some(pending.key.as_str()))
            {
                let edit = self.set(holder.members[existing].value, &pending.value);
                edits.push(Planned {
                    op: pending.op,
                    start: edit.start,
                    end: edit.end,
                    replacement: edit.replacement,
                });
                continue;
            }
            if let Some(entry) = fresh.iter_mut().find(|entry| entry.key == pending.key) {
                entry.value = pending.value.clone();
                continue;
            }
            let at = match pending.position {
                InsertPosition::Start => 0,
                InsertPosition::End => order.len(),
                InsertPosition::Sorted => order
                    .iter()
                    .position(|slot| {
                        let key = match *slot {
                            Slot::Existing(index) if gone(index) => None,
                            Slot::Existing(index) => holder.members[index].key,
                            Slot::New(index) => Some(fresh[index].key.as_str()),
                        };
                        key.is_some_and(|existing| existing > pending.key.as_str())
                    })
                    .unwrap_or(order.len()),
            };
            fresh.push(PendingInsert {
                op: pending.op,
                key: pending.key.clone(),
                value: pending.value.clone(),
                position: pending.position,
            });
            order.insert(at, Slot::New(fresh.len() - 1));
        }
        if fresh.is_empty() {
            return edits;
        }

        let multiline = holder
            .members
            .first()
            .is_none_or(|first| self.source[holder.open + 1..first.start].contains('\n'));
        let outer = line_indent(self.source, holder.open);
        let inner = match holder.members.first() {
            Some(first) if multiline => line_indent(self.source, first.start).to_owned(),
            _ => format!("{outer}{}", self.style.unit),
        };
        let newline = self.style.newline;
        let entry = |index: usize| {
            let item = &fresh[index];
            format!("{}: {}", json_string(&item.key), self.render(&item.value, &inner))
        };
        let owner = |indexes: &[usize]| indexes.iter().map(|&index| fresh[index].op).min().unwrap_or_default();

        // Groups of new keys, each with the surviving member it goes before; `None` stands for the end.
        let mut groups: Vec<(Option<usize>, Vec<usize>)> = Vec::new();
        let mut waiting: Vec<usize> = Vec::new();
        for slot in &order {
            match *slot {
                Slot::New(index) => waiting.push(index),
                Slot::Existing(index) if gone(index) => {}
                Slot::Existing(_) if waiting.is_empty() => {}
                Slot::Existing(index) => groups.push((Some(index), std::mem::take(&mut waiting))),
            }
        }
        if !waiting.is_empty() {
            groups.push((None, waiting));
        }

        for (before, indexes) in groups {
            let entries: Vec<String> = indexes.iter().map(|&index| entry(index)).collect();
            let (start, end, replacement) = match before {
                Some(member) => {
                    let start = holder.members[member].start;
                    let text = if multiline {
                        entries.iter().map(|text| format!("{text},{newline}{inner}")).collect()
                    } else {
                        entries.iter().map(|text| format!("{text}, ")).collect()
                    };
                    (start, start, text)
                }
                None => match (0..count).rfind(|&index| !gone(index)) {
                    Some(last) => {
                        let end = holder.members[last].end;
                        let text = if multiline {
                            entries.iter().map(|text| format!(",{newline}{inner}{text}")).collect()
                        } else {
                            entries.iter().map(|text| format!(", {text}")).collect()
                        };
                        (end, end, text)
                    }
                    None => {
                        let joined = entries.join(&format!(",{newline}{inner}"));
                        let text = format!("{newline}{inner}{joined}{newline}{outer}");
                        if count == 0 {
                            // Comments between the brackets stay: the new members go after them.
                            let between = &self.source[holder.open + 1..holder.close];
                            let kept = between.trim_end_matches([' ', '\t', '\r', '\n']).len();
                            (holder.open + 1 + kept, holder.close, text)
                        } else {
                            (holder.open + 1, holder.open + 1, text)
                        }
                    }
                },
            };
            edits.push(Planned {
                op: owner(&indexes),
                start,
                end,
                replacement,
            });
        }
        edits
    }

    fn queue_removal<'b>(
        &'b self,
        removals: &mut HashMap<usize, Removals<'a, 'b>>,
        op: &str,
        op_index: usize,
        pointer: &[PathSeg],
    ) -> Result<(), EditPlanError> {
        let target = locate(&self.root, pointer).ok_or_else(|| EditPlanError::PathNotFound { op: op.to_owned() })?;
        let (holder, index) = target.parent.ok_or_else(|| EditPlanError::WrongNodeKind {
            op: op.to_owned(),
            expected: "a removable member",
        })?;
        removals
            .entry(holder.open)
            .or_insert_with(|| Removals {
                holder,
                members: BTreeMap::new(),
            })
            .members
            .entry(index)
            .or_insert(op_index);
        Ok(())
    }

    /// Edits removing the queued members of one container.
    ///
    /// Consecutive members are removed together, and the edit of each member covers a slice of the combined range, so
    /// that every operation owns its part and neighbours never conflict.
    fn removal_edits(&self, removals: &Removals<'_, '_>) -> Vec<Planned> {
        let holder = &removals.holder;
        let count = holder.members.len();
        let removed: Vec<(usize, usize)> = removals.members.iter().map(|(&member, &op)| (member, op)).collect();
        let slice = |op: usize, start: usize, end: usize| Planned {
            op,
            start,
            end,
            replacement: String::new(),
        };
        if removed.len() == count {
            return removed
                .iter()
                .map(|&(member, op)| {
                    let start = if member == 0 {
                        holder.open + 1
                    } else {
                        holder.members[member - 1].end
                    };
                    let end = if member + 1 == count {
                        holder.close
                    } else {
                        holder.members[member].end
                    };
                    slice(op, start, end)
                })
                .collect();
        }
        let mut edits = Vec::new();
        let mut rest = removed.as_slice();
        while let Some(&(first, _)) = rest.first() {
            let run_len = rest
                .iter()
                .zip(first..)
                .take_while(|&(&(member, _), expected)| member == expected)
                .count();
            let (run, tail) = rest.split_at(run_len);
            rest = tail;
            let last = first + run_len - 1;
            if last + 1 == count {
                edits.extend(
                    run.iter()
                        .map(|&(member, op)| slice(op, holder.members[member - 1].end, holder.members[member].end)),
                );
                continue;
            }
            let (run_start, run_end) = self.run_bounds(holder, first, last);
            edits.extend(run.iter().map(|&(member, op)| {
                let start = if member == first {
                    run_start
                } else {
                    holder.members[member].start
                };
                let end = if member == last {
                    run_end
                } else {
                    holder.members[member + 1].start
                };
                slice(op, start, end)
            }));
        }
        edits
    }

    /// Range to delete for members `first..=last` when a member follows them: their whole lines when they occupy
    /// their own lines, otherwise up to the start of the next member.
    fn run_bounds(&self, holder: &Container<'_, '_>, first: usize, last: usize) -> (usize, usize) {
        let head = holder.members[first].start;
        let next = holder.members[last + 1].start;
        find_comma(self.source, holder.members[last].end, next)
            .and_then(|comma| {
                let start = line_start(self.source, head);
                let own_line = self.source[start..head].trim_matches([' ', '\t']).is_empty();
                own_line
                    .then(|| rest_of_line(self.source, comma + 1))
                    .flatten()
                    .map(|end| (start, end))
            })
            .unwrap_or((head, next))
    }

    /// Serializes `value`; objects and arrays are laid out with the document's indentation, one level below `base`.
    fn render(&self, value: &serde_json::Value, base: &str) -> String {
        match value {
            serde_json::Value::Object(map) if !map.is_empty() => {
                let inner = format!("{base}{}", self.style.unit);
                let newline = self.style.newline;
                let members: Vec<String> = map
                    .iter()
                    .map(|(key, child)| format!("{inner}{}: {}", json_string(key), self.render(child, &inner)))
                    .collect();
                format!("{{{newline}{}{newline}{base}}}", members.join(&format!(",{newline}")))
            }
            serde_json::Value::Array(items) if !items.is_empty() => {
                let inner = format!("{base}{}", self.style.unit);
                let newline = self.style.newline;
                let members: Vec<String> = items
                    .iter()
                    .map(|child| format!("{inner}{}", self.render(child, &inner)))
                    .collect();
                format!("[{newline}{}{newline}{base}]", members.join(&format!(",{newline}")))
            }
            other => other.to_string(),
        }
    }
}

fn json_string(text: &str) -> String {
    serde_json::Value::String(text.to_owned()).to_string()
}

fn line_start(source: &str, offset: usize) -> usize {
    source[..offset].rfind('\n').map_or(0, |newline| newline + 1)
}

/// Leading whitespace of the line containing `offset`.
fn line_indent(source: &str, offset: usize) -> &str {
    let start = line_start(source, offset);
    let line = &source[start..];
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// Offset just past the line break ending the line at `from`, when only whitespace precedes it.
fn rest_of_line(source: &str, from: usize) -> Option<usize> {
    let rest = &source[from..];
    let newline = rest.find('\n')?;
    rest[..newline]
        .trim_matches([' ', '\t', '\r'])
        .is_empty()
        .then_some(from + newline + 1)
}

/// Offset of the first comma in `source[from..to]` that is not inside a comment.
fn find_comma(source: &str, from: usize, to: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut at = from;
    while at < to {
        match bytes[at] {
            b',' => return Some(at),
            b'/' if bytes.get(at + 1) == Some(&b'/') => {
                at = source[at..to].find('\n').map_or(to, |newline| at + newline);
            }
            b'/' if bytes.get(at + 1) == Some(&b'*') => {
                at = source[at + 2..to].find("*/").map_or(to, |close| at + 2 + close + 2);
            }
            _ => at += 1,
        }
    }
    None
}
