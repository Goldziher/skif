//! Merging configuration layers on the raw TOML tables, recording where every key came from.
//!
//! Layers are applied one after the other, the farthest first. Scalars and scalar arrays replace what is inherited,
//! tables merge key by key, and the keys listed in `EXTEND_KEYS` append. `[rules]`, `[js.plugins]` and the arrays of
//! tables have their own rules, see [`Merger::apply`].

use std::collections::{BTreeMap, HashMap};

use toml::{Table, Value};

use crate::provenance::{Assignment, Op, Origins, join_key};

/// Positions of the keys of one file: dotted key path (`rules.unused-files`, `version-groups[0]`) to 1-based
/// `(line, column)`.
pub(crate) type Spans = HashMap<String, (usize, usize)>;

/// `(table, key, extend key)`: the extend key appends to the key it names, after the key itself applied. The table
/// [`WORKSPACE_TABLE`] stands for every `[js.workspaces."<dir>"]` table.
const EXTEND_KEYS: [(&str, &str, &str); 10] = [
    ("workspace", "ignore-packages", "extend-ignore-packages"),
    ("js", "ignore", "extend-ignore"),
    ("js", "ignore-dependencies", "extend-ignore-dependencies"),
    ("js", "ignore-binaries", "extend-ignore-binaries"),
    ("js", "ignore-unresolved", "extend-ignore-unresolved"),
    (WORKSPACE_TABLE, "ignore", "extend-ignore"),
    (WORKSPACE_TABLE, "ignore-dependencies", "extend-ignore-dependencies"),
    (WORKSPACE_TABLE, "ignore-binaries", "extend-ignore-binaries"),
    (WORKSPACE_TABLE, "ignore-unresolved", "extend-ignore-unresolved"),
    ("catalog", "exclude", "extend-exclude"),
];

/// The result of applying every layer.
pub(crate) struct Merged {
    pub table: Table,
    pub origins: Origins,
    /// For each entry of `overrides`, the index of the layer that declared it.
    pub overrides: Vec<usize>,
}

/// The layers applied so far.
#[derive(Default)]
pub(crate) struct Merger {
    table: Table,
    origins: Origins,
    /// Origins of the entries of the arrays of tables, aligned with the arrays in `table`.
    lists: BTreeMap<String, Vec<Assignment>>,
}

#[derive(Clone, Copy)]
enum Placement {
    Append,
    Prepend,
}

struct Cx<'a> {
    source: usize,
    spans: &'a Spans,
    origins: &'a mut Origins,
}

impl Cx<'_> {
    fn assignment(&self, span_path: &str, op: Op) -> Assignment {
        let (line, column) = self.spans.get(span_path).copied().unwrap_or((0, 0));
        Assignment {
            source: self.source,
            line,
            column,
            op,
        }
    }

    fn record(&mut self, key: &str, span_path: &str, op: Op) {
        let assignment = self.assignment(span_path, op);
        self.origins.push(key.to_owned(), assignment);
    }
}

impl Merger {
    /// Applies `layer`, the parsed file number `source`, over what was applied before.
    ///
    /// - `root` and `extends` are directives of the loader and are not merged.
    /// - `reset` first discards the inherited value of the lists it names.
    /// - `[rules]` merges per rule: a level (a bare string, or `level` / `severity`) replaces the level and keeps the
    ///   options; a non-empty options table replaces the options and keeps the level.
    /// - `[js.plugins]` merges per plugin: `true` / `false` sets `enabled` and keeps the options; a table replaces the
    ///   options and sets `enabled` only when it gives it.
    /// - `[[ignore]]` and `[[overrides]]` append; `[[version-groups]]`, `[[semver-groups]]` and `[[update-groups]]` are
    ///   put before the inherited ones, so the nearer file wins the first-match; `[[dependency-families]]` and `[[custom-types]]` replace the
    ///   entry of the same name and append the others.
    /// - Every other table merges key by key, and every other value replaces.
    /// - An `extend-*` key appends to the list it extends, after that list was replaced when the same layer sets it.
    pub(crate) fn apply(&mut self, source: usize, layer: &Table, spans: &Spans) {
        let Self { table, origins, lists } = self;
        let mut cx = Cx { source, spans, origins };
        if let Some(Value::Array(keys)) = layer.get("reset") {
            for (index, name) in keys.iter().filter_map(Value::as_str).enumerate() {
                table.remove(name);
                lists.remove(name);
                cx.record(name, &format!("reset[{index}]"), Op::Reset);
            }
        }
        for (key, value) in layer {
            match (key.as_str(), value) {
                ("root" | "extends" | "reset", _) => {}
                ("rules", Value::Table(rules)) => merge_rules(table, rules, &mut cx),
                ("version-groups" | "semver-groups" | "update-groups", Value::Array(items)) => {
                    merge_list(table, lists, key, items, Placement::Prepend, &mut cx);
                }
                ("ignore" | "overrides", Value::Array(items)) => {
                    merge_list(table, lists, key, items, Placement::Append, &mut cx);
                }
                ("dependency-families" | "custom-types", Value::Array(items)) => {
                    merge_named(table, lists, key, items, &mut cx);
                }
                (_, Value::Table(incoming)) => merge_table(section(table, key), incoming, key, &mut cx),
                _ => {
                    table.insert(key.clone(), value.clone());
                    cx.record(key, key, Op::Set);
                }
            }
        }
    }

    /// The merged table, without origins.
    pub(crate) fn into_table(self) -> Table {
        self.table
    }

    pub(crate) fn finish(self) -> Merged {
        let Self {
            table,
            mut origins,
            lists,
        } = self;
        let overrides: Vec<usize> = lists
            .get("overrides")
            .map(|assigned| assigned.iter().map(|assignment| assignment.source).collect())
            .unwrap_or_default();
        for (key, assigned) in lists {
            for (index, assignment) in assigned.into_iter().enumerate() {
                origins.push(format!("{key}[{index}]"), assignment);
            }
        }
        Merged {
            table,
            origins,
            overrides,
        }
    }
}

/// The table at `key`, created (or replaced, when it is something else) when missing.
fn section<'t>(parent: &'t mut Table, key: &str) -> &'t mut Table {
    let slot = parent
        .entry(key.to_owned())
        .or_insert_with(|| Value::Table(Table::new()));
    if !slot.is_table() {
        *slot = Value::Table(Table::new());
    }
    match slot {
        Value::Table(table) => table,
        _ => unreachable!("a table was just stored"),
    }
}

/// The array at `key`, created (or replaced, when it is something else) when missing.
fn list<'t>(parent: &'t mut Table, key: &str) -> &'t mut Vec<Value> {
    let slot = parent.entry(key.to_owned()).or_insert_with(|| Value::Array(Vec::new()));
    if !slot.is_array() {
        *slot = Value::Array(Vec::new());
    }
    match slot {
        Value::Array(items) => items,
        _ => unreachable!("an array was just stored"),
    }
}

const WORKSPACE_TABLE: &str = "js.workspaces.*";

/// The name `EXTEND_KEYS` gives the table at `path`: [`WORKSPACE_TABLE`] for a `[js.workspaces."<dir>"]` table, else
/// `path` itself.
fn extend_table(path: &str) -> &str {
    match path.strip_prefix("js.workspaces.") {
        Some(dir) if is_one_segment(dir) => WORKSPACE_TABLE,
        _ => path,
    }
}

/// Whether `key`, spelled as [`join_key`] spells a segment, is a single segment: a bare key without `.`, or one quoted
/// key with no unescaped quote inside.
fn is_one_segment(key: &str) -> bool {
    match key.strip_prefix('"').and_then(|rest| rest.strip_suffix('"')) {
        Some(inner) => !inner.replace("\\\\", "").replace("\\\"", "").contains('"'),
        None => !key.contains(['.', '[', ']', '"']),
    }
}

/// Whether `table`, one parsed file, sets one of the `EXTEND_KEYS` in the table that owns it.
pub(crate) fn has_extend_keys(table: &Table) -> bool {
    fn sets_one(table: &Table, path: &str) -> bool {
        let name = extend_table(path);
        table.iter().any(|(key, value)| {
            EXTEND_KEYS
                .iter()
                .any(|(owner, _, extend)| *owner == name && *extend == key)
                || matches!(value, Value::Table(child) if sets_one(child, &join_key(path, key)))
        })
    }
    sets_one(table, "")
}

fn merge_table(base: &mut Table, layer: &Table, path: &str, cx: &mut Cx<'_>) {
    let table_name = extend_table(path);
    for (key, value) in layer {
        if EXTEND_KEYS
            .iter()
            .any(|(table, _, extend)| *table == table_name && *extend == key)
        {
            continue;
        }
        let child = join_key(path, key);
        if path == "js.plugins" {
            let merged = merge_plugin(base.get(key), value);
            base.insert(key.clone(), merged);
            cx.record(&child, &child, Op::Set);
        } else if let Value::Table(incoming) = value {
            merge_table(section(base, key), incoming, &child, cx);
        } else {
            base.insert(key.clone(), value.clone());
            cx.record(&child, &child, Op::Set);
        }
    }
    for (_, key, extend) in EXTEND_KEYS.iter().filter(|(table, _, _)| *table == table_name) {
        if let Some(Value::Array(extra)) = layer.get(*extend) {
            list(base, key).extend(extra.iter().cloned());
            cx.record(&join_key(path, key), &join_key(path, extend), Op::Append);
        }
    }
}

fn merge_plugin(inherited: Option<&Value>, incoming: &Value) -> Value {
    match incoming {
        Value::Boolean(enabled) => match inherited {
            Some(Value::Table(options)) => {
                let mut options = options.clone();
                options.insert("enabled".to_owned(), Value::Boolean(*enabled));
                Value::Table(options)
            }
            _ => Value::Boolean(*enabled),
        },
        Value::Table(options) => {
            let mut options = options.clone();
            if !options.contains_key("enabled") {
                let carried = match inherited {
                    Some(Value::Boolean(enabled)) => Some(*enabled),
                    Some(Value::Table(table)) => table.get("enabled").and_then(Value::as_bool),
                    _ => None,
                };
                if let Some(enabled) = carried {
                    options.insert("enabled".to_owned(), Value::Boolean(enabled));
                }
            }
            Value::Table(options)
        }
        other => other.clone(),
    }
}

/// A rule entry split into its level and its options.
fn split_rule(value: &Value) -> (Option<String>, Table) {
    match value {
        Value::String(level) => (Some(level.clone()), Table::new()),
        Value::Table(table) => {
            let mut options = table.clone();
            let severity = options.remove("severity");
            let level = options.remove("level").or(severity);
            (level.and_then(|level| level.as_str().map(str::to_owned)), options)
        }
        _ => (None, Table::new()),
    }
}

fn compose_rule(level: Option<String>, options: Table) -> Value {
    if options.is_empty() {
        return level.map_or_else(|| Value::Table(Table::new()), Value::String);
    }
    let mut table = Table::new();
    if let Some(level) = level {
        table.insert("level".to_owned(), Value::String(level));
    }
    table.extend(options);
    Value::Table(table)
}

fn merge_rules(table: &mut Table, layer: &Table, cx: &mut Cx<'_>) {
    let base = section(table, "rules");
    for (id, value) in layer {
        let (level, options) = split_rule(value);
        let (inherited_level, inherited_options) = base.get(id).map_or_else(|| (None, Table::new()), split_rule);
        let own = join_key("rules", id);
        if level.is_some() {
            let at = [format!("{own}.level"), format!("{own}.severity")]
                .into_iter()
                .find(|path| cx.spans.contains_key(path))
                .unwrap_or_else(|| own.clone());
            cx.record(&format!("{own}.level"), &at, Op::Set);
        }
        if !options.is_empty() {
            cx.record(&format!("{own}.options"), &own, Op::Set);
        }
        let options = if options.is_empty() { inherited_options } else { options };
        base.insert(id.clone(), compose_rule(level.or(inherited_level), options));
    }
}

fn merge_list(
    table: &mut Table,
    lists: &mut BTreeMap<String, Vec<Assignment>>,
    key: &str,
    items: &[Value],
    placement: Placement,
    cx: &mut Cx<'_>,
) {
    let op = match placement {
        Placement::Append => Op::Append,
        Placement::Prepend => Op::Prepend,
    };
    let mut assigned: Vec<Assignment> = (0..items.len())
        .map(|index| cx.assignment(&format!("{key}[{index}]"), op))
        .collect();
    let existing = list(table, key);
    let tracked = lists.entry(key.to_owned()).or_default();
    match placement {
        Placement::Append => {
            existing.extend(items.iter().cloned());
            tracked.extend(assigned);
        }
        Placement::Prepend => {
            let mut merged = items.to_vec();
            merged.append(existing);
            *existing = merged;
            assigned.append(tracked);
            *tracked = assigned;
        }
    }
}

/// Merges the entries of the list `key` by their `name`: an entry replaces the inherited one of the same name, the
/// others append.
fn merge_named(
    table: &mut Table,
    lists: &mut BTreeMap<String, Vec<Assignment>>,
    key: &str,
    items: &[Value],
    cx: &mut Cx<'_>,
) {
    let existing = list(table, key);
    let tracked = lists.entry(key.to_owned()).or_default();
    for (index, item) in items.iter().enumerate() {
        let assignment = cx.assignment(&format!("{key}[{index}]"), Op::Append);
        let name = item.get("name").and_then(Value::as_str);
        let position = name.and_then(|name| {
            existing
                .iter()
                .position(|family| family.get("name").and_then(Value::as_str) == Some(name))
        });
        if let Some(at) = position {
            existing[at] = item.clone();
            tracked[at] = Assignment {
                op: Op::Set,
                ..assignment
            };
        } else {
            existing.push(item.clone());
            tracked.push(assignment);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{EXTEND_KEYS, WORKSPACE_TABLE, has_extend_keys};
    use crate::{IgnoreItem, Js, JsWorkspace};

    fn table(text: &str) -> toml::Table {
        text.parse().unwrap()
    }

    #[test]
    fn an_extend_key_counts_only_in_the_table_that_owns_it() {
        assert!(has_extend_keys(&table("[js]\nextend-ignore = [\"a\"]\n")));
        assert!(has_extend_keys(&table(
            "[workspace]\nextend-ignore-packages = [\"a\"]\n"
        )));
        assert!(has_extend_keys(&table("[catalog]\nextend-exclude = [\"a\"]\n")));
        assert!(has_extend_keys(&table(
            "[js.workspaces.\"apps/web\"]\nextend-ignore-binaries = [\"a\"]\n"
        )));
        assert!(!has_extend_keys(&table("[js]\nignore = [\"a\"]\n")));
        assert!(!has_extend_keys(&table("[catalog]\nextend-ignore = [\"a\"]\n")));
        assert!(!has_extend_keys(&table(
            "[js.plugins.vitest]\nextend-ignore = [\"a\"]\n"
        )));
    }

    #[test]
    fn the_extend_lists_checked_for_a_reason_are_the_extend_keys_of_their_table() {
        let listed = |lists: &[(&'static str, &[IgnoreItem])]| -> BTreeSet<&'static str> {
            lists
                .iter()
                .map(|(key, _)| *key)
                .filter(|key| key.starts_with("extend-"))
                .collect()
        };
        // `extend-ignore` holds globs of files that are not analysed, not suppressions with a reason.
        let declared = |owner: &str| -> BTreeSet<&'static str> {
            EXTEND_KEYS
                .iter()
                .filter(|(table, _, extend)| *table == owner && *extend != "extend-ignore")
                .map(|(_, _, extend)| *extend)
                .collect()
        };
        assert_eq!(listed(&Js::default().ignore_lists()), declared("js"));
        assert_eq!(
            listed(&JsWorkspace::default().ignore_lists()),
            declared(WORKSPACE_TABLE)
        );
    }
}
