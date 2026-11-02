//! JSON and JSONC edits, located with `jsonc-parser` byte ranges.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

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
                _ => return None,
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

fn parse_error(error: &(impl ToString + ?Sized)) -> EditPlanError {
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
    let parsed = parse_to_ast(&after, &CollectOptions::default(), &parse_options(strict)).map_err(|error| {
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
    let parsed = parse_to_ast(body, &CollectOptions::default(), &ParseOptions::default())
        .map_err(|error| parse_error(&error))?;
    let root = parsed.value.ok_or_else(|| parse_error("document is empty"))?;
    let doc = Doc {
        source: body,
        root,
        style: Style::detect(body),
    };
    let mut planned = Vec::new();
    let mut texts = Vec::new();
    let mut removals: HashMap<usize, Removals<'_, '_>> = HashMap::new();
    let mut insertions: HashMap<usize, Insertions<'_, '_>> = HashMap::new();
    // Index in `planned` of each edit that moves a member, with the byte range of the member it moves.
    let mut moves: Vec<(usize, (usize, usize))> = Vec::new();
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
                doc.queue_insertion(
                    &mut insertions,
                    &describe::op(op),
                    index,
                    pointer,
                    (key.as_str(), value, *position),
                )?;
                Vec::new()
            }
            FixOp::Json {
                pointer,
                action: JsonAction::SortKeys,
                ..
            } => {
                for (edit, from) in doc.sort_edits(&describe::op(op), pointer)? {
                    moves.push((planned.len(), from));
                    planned.push(Planned {
                        op: index,
                        start: edit.start,
                        end: edit.end,
                        replacement: edit.replacement,
                    });
                }
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
    rebase_moves(&mut planned, &moves);
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
            JsonAction::RenameKey(new_key) => Self::rename(op, target, new_key),
            // `Remove`, `Insert` and `SortKeys` are planned per container; any action added later has no in-place JSON
            // edit.
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

    /// The edits sorting the object at `pointer`, each with the byte range of the member whose text it writes.
    fn sort_edits(&self, op: &str, pointer: &[PathSeg]) -> Result<Vec<(Edit, (usize, usize))>, EditPlanError> {
        let target = locate(&self.root, pointer).ok_or_else(|| EditPlanError::PathNotFound { op: op.to_owned() })?;
        let holder = container(target.value)
            .filter(|holder| holder.is_object)
            .ok_or_else(|| EditPlanError::WrongNodeKind {
                op: op.to_owned(),
                expected: "an object",
            })?;
        Ok(self.sort_keys(&holder))
    }

    fn sort_keys(&self, holder: &Container<'_, '_>) -> Vec<(Edit, (usize, usize))> {
        let mut order: Vec<usize> = (0..holder.members.len()).collect();
        order.sort_by(|&a, &b| holder.members[a].key.cmp(&holder.members[b].key));
        order
            .iter()
            .enumerate()
            .filter(|&(slot, &from)| slot != from)
            .map(|(slot, &from)| {
                let (target, source) = (&holder.members[slot], &holder.members[from]);
                (
                    Edit::new(
                        target.start,
                        target.end,
                        self.source[source.start..source.end].to_owned(),
                    ),
                    (source.start, source.end),
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
                _ => order
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
                    let text: String = if multiline {
                        entries.iter().fold(String::new(), |mut out, text| {
                            let _ = write!(out, "{text},{newline}{inner}");
                            out
                        })
                    } else {
                        entries.iter().fold(String::new(), |mut out, text| {
                            let _ = write!(out, "{text}, ");
                            out
                        })
                    };
                    (start, start, text)
                }
                None => {
                    if let Some(last) = (0..count).rfind(|&index| !gone(index)) {
                        let end = holder.members[last].end;
                        let text: String = if multiline {
                            entries.iter().fold(String::new(), |mut out, text| {
                                let _ = write!(out, ",{newline}{inner}{text}");
                                out
                            })
                        } else {
                            entries.iter().fold(String::new(), |mut out, text| {
                                let _ = write!(out, ", {text}");
                                out
                            })
                        };
                        (end, end, text)
                    } else {
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
                }
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
    /// Every removed member owns its edits, so operations never conflict with their neighbours:
    ///
    /// * a member with a survivor after it takes the comma and the whitespace that follow it, and its own line when it
    ///   occupies one;
    /// * the members of a run that reaches the end of the container, with a survivor before it, take the comma and
    ///   whitespace that precede them, so no comma is left dangling;
    /// * the comma and the whitespace after it are always one edit, so a member planned alone and the member after it
    ///   planned alone agree on it and the two plans combine.
    ///
    /// Comments between members stay.
    fn removal_edits(&self, removals: &Removals<'_, '_>) -> Vec<Planned> {
        let holder = &removals.holder;
        let count = holder.members.len();
        if removals.members.len() == count {
            return removals
                .members
                .iter()
                .map(|(&member, &op)| {
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
                    Self::removal(op, start, end)
                })
                .collect();
        }
        let tail_start = (0..count)
            .rev()
            .take_while(|member| removals.members.contains_key(member))
            .last()
            .unwrap_or(count);
        removals
            .members
            .iter()
            .flat_map(|(&member, &op)| {
                if member >= tail_start {
                    self.removal_with_preceding_separator(holder, member, op)
                } else {
                    self.removal_with_following_separator(holder, member, op)
                }
            })
            .collect()
    }

    const fn removal(op: usize, start: usize, end: usize) -> Planned {
        Planned {
            op,
            start,
            end,
            replacement: String::new(),
        }
    }

    /// The comma after member `previous` and the whitespace behind it, as a byte range, and whether the comma ends its
    /// line. The whitespace includes the line break when nothing but whitespace follows the comma on its line; a
    /// comment after the comma is not part of the separator.
    fn separator(&self, holder: &Container<'_, '_>, previous: usize) -> Option<(usize, usize, bool)> {
        let next = holder.members[previous + 1].start;
        let comma = find_comma(self.source, holder.members[previous].end, next)?;
        let after = comma + 1;
        if let Some(end) = rest_of_line(self.source, after).filter(|&end| end <= next) {
            return Some((comma, end, true));
        }
        let blank = self.source[after..next].trim_matches([' ', '\t', '\r', '\n']).is_empty();
        Some((comma, if blank { next } else { after }, false))
    }

    /// Start of the line `offset` is on, when only blanks precede it there.
    fn own_line_start(&self, offset: usize) -> Option<usize> {
        let start = line_start(self.source, offset);
        self.source[start..offset].trim_matches([' ', '\t']).is_empty().then_some(start)
    }

    fn removal_with_following_separator(&self, holder: &Container<'_, '_>, member: usize, op: usize) -> Vec<Planned> {
        let (start, next) = (holder.members[member].start, holder.members[member + 1].start);
        let Some((comma, end, ends_line)) = self.separator(holder, member) else {
            return vec![Self::removal(op, start, next)];
        };
        let from = if ends_line {
            self.own_line_start(start).unwrap_or(start)
        } else {
            start
        };
        vec![Self::removal(op, from, comma), Self::removal(op, comma, end)]
    }

    fn removal_with_preceding_separator(&self, holder: &Container<'_, '_>, member: usize, op: usize) -> Vec<Planned> {
        let (start, end) = (holder.members[member].start, holder.members[member].end);
        let Some((comma, gap_end, _)) = self.separator(holder, member - 1) else {
            return vec![Self::removal(op, holder.members[member - 1].end, end)];
        };
        if gap_end > comma + 1 {
            return vec![Self::removal(op, comma, gap_end), Self::removal(op, gap_end, end)];
        }
        // A comment follows the comma and stays: the member goes alone, with its line when it occupies one.
        let own_line = self.own_line_start(start);
        let stop = own_line.and_then(|_| rest_of_line(self.source, end)).unwrap_or(end);
        vec![
            Self::removal(op, comma, comma + 1),
            Self::removal(op, own_line.unwrap_or(start), stop),
        ]
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

/// Whether `edit` changes something inside the member spanning `from_start..from_end` and not the member as a whole.
const fn is_inside(edit: &Planned, from_start: usize, from_end: usize) -> bool {
    if edit.start < from_start || edit.end > from_end {
        return false;
    }
    if edit.start == edit.end {
        return edit.start > from_start && edit.start < from_end;
    }
    !(edit.start == from_start && edit.end == from_end)
}

/// Rebases the edits inside a member that a `SortKeys` moves onto the text it moves, so that sorting an object and
/// changing what is in it agree.
///
/// `moves` lists, for each edit of `planned` that writes a member's text, its index and the byte range of that member.
/// Smaller members are handled first, so a sort nested in a moved member travels along with it.
fn rebase_moves(planned: &mut Vec<Planned>, moves: &[(usize, (usize, usize))]) {
    if moves.is_empty() {
        return;
    }
    let mut order: Vec<&(usize, (usize, usize))> = moves.iter().collect();
    order.sort_by_key(|(_, (start, end))| end - start);
    let mut absorbed = vec![false; planned.len()];
    for &&(at, (from_start, from_end)) in &order {
        let mut inside: Vec<usize> = (0..planned.len())
            .filter(|&index| index != at && !absorbed[index] && is_inside(&planned[index], from_start, from_end))
            .collect();
        inside.sort_by_key(|&index| (planned[index].start, planned[index].end));
        if inside.is_empty() || inside.windows(2).any(|pair| planned[pair[0]].end > planned[pair[1]].start) {
            continue;
        }
        let moved = planned[at].replacement.clone();
        let mut rebased = String::with_capacity(moved.len());
        let mut cursor = 0;
        let mut fits = true;
        for &index in &inside {
            let edit = &planned[index];
            let Some(kept) = moved.get(cursor..edit.start - from_start) else {
                fits = false;
                break;
            };
            rebased.push_str(kept);
            rebased.push_str(&edit.replacement);
            cursor = edit.end - from_start;
        }
        let Some(tail) = moved.get(cursor..).filter(|_| fits) else {
            continue;
        };
        rebased.push_str(tail);
        planned[at].replacement = rebased;
        for index in inside {
            absorbed[index] = true;
        }
    }
    let mut index = 0;
    planned.retain(|_| {
        let keep = !absorbed[index];
        index += 1;
        keep
    });
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
