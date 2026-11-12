//! `--merge`: combining the generated sections with an existing `skif.toml`.
//!
//! The existing file is edited in place with `toml_edit`, so its comments, order and formatting survive. Tables and
//! keys it lacks are added (new top-level tables after everything it has, under a banner), arrays and arrays of
//! tables gain the entries they lack, and a key both files set to different values keeps the existing value (and is
//! reported).
//!
//! List entries match by name: a string and a `{ name, reason }` table with that name are one entry, which keeps the
//! existing reason or takes the migrated one. In the `[js]` lists a trailing `!` does not make another entry either.
//! In `entry` and `project` the united entry keeps the marker when either file has it: a plain pattern next to its
//! `x!` form would exclude the file from a production run, and in a normal run the two are the same. In the ignore
//! lists it keeps the marker only when both have it, since `x` applies in every mode and `x!` in production only.
//!
//! The `exact` profile writes `off` values for the rules the migrated tools did not run, each with a trailing
//! `# skif migrate: not checked by <tools>` comment. Those values are not user choices, so a later migration of
//! another tool replaces them when it does check the rule, and removes them when its own profile leaves the rule on.
//! A file also records the tools it was migrated from in a `# skif migrate: sources = [...]` line.
//!
//! A merge adds no new profile value: a rule an existing file does not mention runs, either because one of its
//! recorded sources checked it or because the file (written by hand, or for other tools) relies on skif's default.
//! The new migration only adds what its tools need, and [`Merged::left_as_existing`] lists the profile values it left
//! out. Migrating tools one after the other therefore ends with the same enabled checks as migrating them in one
//! command. Only a file with no keys at all takes the profile values, as a new file does.
//!
//! Migrating knip into a file with `[js] enabled = false` sets it to `true` ([`Merged::replaced_existing`]): the knip
//! settings the migration writes are `[js]` settings, which take effect only with the JavaScript analysis on.

use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, Value};

use crate::notes::Tool;
use crate::{MigrateError, Migration, Profile, keys};

/// The result of merging into an existing file.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Merged {
    /// The merged `skif.toml`.
    pub toml: String,
    /// The keys where the existing value was kept over the generated one, as dotted paths.
    pub kept_existing: Vec<String>,
    /// The values the `exact` profile would have written (`rules.<id> = "off"`, `js.enabled = false`) that the merge
    /// left out, as dotted paths: the existing file runs those checks, as it has them.
    pub left_as_existing: Vec<String>,
    /// The keys whose existing value was replaced because it would leave the migration without effect, as dotted
    /// paths: `js.enabled` when knip is migrated into a file that turns the JavaScript analysis off, and an `entry` or
    /// `project` list whose plain pattern took the `!` production marker of the same migrated pattern.
    pub replaced_existing: Vec<String>,
}

/// The line that records the tools a file was migrated from.
pub(crate) fn sources_line(tools: &[Tool]) -> String {
    let names: Vec<String> = tools.iter().map(|tool| format!("\"{}\"", tool.name())).collect();
    format!("{}[{}]", keys::SOURCES_PREFIX, names.join(", "))
}

/// The tools recorded by the sources line of `text`, if it has one.
fn read_sources(text: &str) -> Option<Vec<Tool>> {
    text.lines().find_map(|line| {
        let list = line.trim().strip_prefix(keys::SOURCES_PREFIX)?;
        let inner = list.trim().strip_prefix('[')?.strip_suffix(']')?;
        Some(
            inner
                .split(',')
                .filter_map(|name| Tool::from_name(name.trim().trim_matches('"')))
                .collect(),
        )
    })
}

/// `text` with its sources line replaced by one that lists `tools`, or with the line added at the end.
fn with_sources(text: &str, tools: &[Tool]) -> String {
    let line = sources_line(tools);
    let mut out = String::with_capacity(text.len() + line.len() + 1);
    let mut replaced = false;
    for piece in text.split_inclusive('\n') {
        if !replaced && piece.trim().starts_with(keys::SOURCES_PREFIX) {
            out.push_str(&line);
            if piece.ends_with('\n') {
                out.push('\n');
            }
            replaced = true;
        } else {
            out.push_str(piece);
        }
    }
    if !replaced {
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}

pub(crate) fn merge_into(migration: &Migration, existing: &str) -> Result<Merged, MigrateError> {
    if existing.trim().is_empty() {
        return Ok(Merged {
            toml: migration.toml.clone(),
            kept_existing: Vec::new(),
            left_as_existing: Vec::new(),
            replaced_existing: Vec::new(),
        });
    }
    let mut current: DocumentMut = existing
        .parse()
        .map_err(|error: toml_edit::TomlError| MigrateError::Existing(error.to_string()))?;
    let added: DocumentMut = migration
        .body
        .parse()
        .map_err(|error: toml_edit::TomlError| MigrateError::Invalid(error.to_string()))?;
    let previous = read_sources(existing);
    let mut sources = previous.clone().unwrap_or_default();
    for tool in &migration.tools {
        if !sources.contains(tool) {
            sources.push(*tool);
        }
    }
    let mut merge = Merge {
        skip_profile: previous.is_some() || !current.is_empty(),
        kept: Vec::new(),
        left: Vec::new(),
        replaced: Vec::new(),
        banner: Some(banner(&migration.tools, migration.profile)),
    };
    merge.table(current.as_table_mut(), added.as_table(), "");
    let mut replaced = Vec::new();
    if migration.tools.contains(&Tool::Knip) && enable_js(&mut current) {
        replaced.push(format!("{}.{}", keys::JS, keys::JS_ENABLED));
    }
    replaced.append(&mut merge.replaced);
    Ok(Merged {
        toml: with_sources(&current.to_string(), &sources),
        kept_existing: merge.kept,
        left_as_existing: merge.left,
        replaced_existing: replaced,
    })
}

/// Sets `[js] enabled = false` of `document` to `true`, keeping its comments; whether there was such a value.
fn enable_js(document: &mut DocumentMut) -> bool {
    let Some(Item::Value(value)) = document
        .get_mut(keys::JS)
        .and_then(Item::as_table_like_mut)
        .and_then(|js| js.get_mut(keys::JS_ENABLED))
    else {
        return false;
    };
    if value.as_bool() != Some(false) {
        return false;
    }
    let decor = value.decor().clone();
    *value = Value::from(true);
    *value.decor_mut() = decor;
    true
}

/// Removes the profile values of `table` and of the tables inside it, adding their dotted paths to `left`.
fn strip_profile(table: &mut Table, path: &str, left: &mut Vec<String>) {
    let marked: Vec<String> = table
        .iter()
        .filter(|(_, item)| marker(item).is_some())
        .map(|(key, _)| key.to_owned())
        .collect();
    for key in marked {
        table.remove(&key);
        left.push(format!("{path}.{key}"));
    }
    for (key, item) in table.iter_mut() {
        if let Item::Table(child) = item {
            let child_path = format!("{path}.{}", key.get());
            strip_profile(child, &child_path, left);
        }
    }
}

fn banner(tools: &[Tool], profile: Profile) -> String {
    let names: Vec<&str> = tools.iter().map(|tool| tool.name()).collect();
    let profile = match profile {
        Profile::Exact => "exact",
        Profile::Full => "full",
    };
    format!(
        "# Added by `skif migrate --merge` from {} (profile: {profile}).\n",
        names.join(", ")
    )
}

fn same(left: &impl std::fmt::Display, right: &impl std::fmt::Display) -> bool {
    left.to_string().trim() == right.to_string().trim()
}

/// How the entries of a list treat a trailing `!`, the production marker, when two lists are united.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MarkerRule {
    /// `[js]` `entry` and `project`: `x!` is `x` in a normal run, and in a production run only `x!` counts while `x`
    /// is excluded, so `x` next to `x!` would drop the file there. The entry keeps the marker when either has it.
    Production,
    /// The `[js]` ignore lists: `x!` applies in production mode only and `x` in every mode, so `x` covers `x!`. The
    /// entry keeps the marker only when both have it.
    Plain,
    /// Every other list: the marker is part of the entry.
    Literal,
}

impl MarkerRule {
    /// The rule for the list at the dotted `path`, whose own key is `key`.
    fn of(path: &str, key: &str) -> Self {
        if path.split('.').next() != Some(keys::JS) {
            return Self::Literal;
        }
        if keys::JS_PRODUCTION_LISTS.contains(&key) {
            Self::Production
        } else if keys::JS_IGNORE_LISTS.contains(&key) {
            Self::Plain
        } else {
            Self::Literal
        }
    }

    /// `name` without its production marker, and whether it had one.
    fn split(self, name: &str) -> (&str, bool) {
        match name.strip_suffix('!') {
            Some(body) if self != Self::Literal => (body, true),
            _ => (name, false),
        }
    }

    /// Whether the united entry has the marker, from whether the existing and the added entry have it.
    const fn unite(self, held: bool, added: bool) -> bool {
        match self {
            Self::Production => held || added,
            Self::Plain => held && added,
            Self::Literal => held,
        }
    }
}

/// The name and the reason of a list entry: a string, or a `{ name, reason }` table (`reason` optional). `None` for
/// any other value, which matches only an entry with the same text.
fn named(value: &Value) -> Option<(&str, Option<&str>)> {
    match value {
        Value::String(text) => Some((text.value().as_str(), None)),
        Value::InlineTable(table) => {
            if table.iter().any(|(key, _)| key != "name" && key != "reason") {
                return None;
            }
            let name = table.get("name")?.as_str()?;
            let reason = match table.get("reason") {
                Some(reason) => Some(reason.as_str()?),
                None => None,
            };
            Some((name, reason))
        }
        _ => None,
    }
}

/// A list entry with `name` and an optional `reason`, in the form the migration writes.
fn entry_value(name: &str, reason: Option<&str>) -> Value {
    let Some(reason) = reason else {
        return Value::from(name);
    };
    let mut table = InlineTable::new();
    table.insert("name", Value::from(name));
    table.insert("reason", Value::from(reason));
    Value::InlineTable(table)
}

fn push_once(paths: &mut Vec<String>, path: &str) {
    if !paths.iter().any(|held| held == path) {
        paths.push(path.to_owned());
    }
}

/// The tools named by the profile marker after `item`, if it has one.
fn marker(item: &Item) -> Option<Vec<String>> {
    let suffix = item.as_value()?.decor().suffix()?.as_str()?;
    let (_, tools) = suffix.split_once(keys::PROFILE_MARKER)?;
    Some(
        tools
            .split(',')
            .map(str::trim)
            .filter(|tool| !tool.is_empty())
            .map(str::to_owned)
            .collect(),
    )
}

fn set_marker(item: &mut Item, tools: &[String]) {
    if let Item::Value(value) = item {
        value
            .decor_mut()
            .set_suffix(format!(" # {}{}", keys::PROFILE_MARKER, tools.join(", ")));
    }
}

/// Drops the positions of `item` and of every table inside it, so the tables are written where they are inserted
/// instead of between the tables of the file they were added to.
fn clear_positions(item: &mut Item) {
    match item {
        Item::Table(table) => clear_table_positions(table),
        Item::ArrayOfTables(tables) => {
            for table in tables.iter_mut() {
                clear_table_positions(table);
            }
        }
        _ => {}
    }
}

fn clear_table_positions(table: &mut Table) {
    table.set_position(None);
    for (_, child) in table.iter_mut() {
        clear_positions(child);
    }
}

struct Merge {
    /// The existing file has content, so a profile value it lacks is a check it runs: by a recorded source, or by
    /// skif's default.
    skip_profile: bool,
    kept: Vec<String>,
    /// The profile values left out because the existing file runs the check.
    left: Vec<String>,
    /// The lists whose plain `entry` or `project` patterns took the production marker of the migrated ones.
    replaced: Vec<String>,
    /// The banner for the first top-level table that is added.
    banner: Option<String>,
}

impl Merge {
    /// Puts the banner before the first top-level table added.
    fn announce(&mut self, item: &mut Item) {
        let Some(banner) = self.banner.take() else {
            return;
        };
        let table = match item {
            Item::Table(table) => Some(table),
            Item::ArrayOfTables(tables) => tables.iter_mut().next(),
            _ => None,
        };
        if let Some(table) = table {
            let old = table.decor().prefix().and_then(|prefix| prefix.as_str()).unwrap_or("");
            let prefix = format!("\n{banner}{old}");
            table.decor_mut().set_prefix(prefix);
        } else {
            self.banner = Some(banner);
        }
    }

    fn table(&mut self, into: &mut Table, from: &Table, prefix: &str) {
        for (key, item) in from {
            let path = if prefix.is_empty() {
                key.to_owned()
            } else {
                format!("{prefix}.{key}")
            };
            match (into.get_mut(key), item) {
                (None, _) => {
                    if self.skip_profile && marker(item).is_some() {
                        self.left.push(path);
                        continue;
                    }
                    let mut inserted = item.clone();
                    if self.skip_profile
                        && let Item::Table(table) = &mut inserted
                    {
                        let had_keys = !table.is_empty();
                        strip_profile(table, &path, &mut self.left);
                        if had_keys && table.is_empty() {
                            continue;
                        }
                    }
                    clear_positions(&mut inserted);
                    if prefix.is_empty() {
                        self.announce(&mut inserted);
                    }
                    into.insert(key, inserted);
                }
                (Some(Item::Table(existing)), Item::Table(added)) => self.table(existing, added, &path),
                (Some(Item::ArrayOfTables(existing)), Item::ArrayOfTables(added)) => {
                    for table in added {
                        if !existing.iter().any(|present| same(present, table)) {
                            let mut inserted = table.clone();
                            clear_table_positions(&mut inserted);
                            existing.push(inserted);
                        }
                    }
                }
                (Some(Item::Value(Value::Array(existing))), Item::Value(Value::Array(added))) => {
                    self.list(existing, added, &path, MarkerRule::of(&path, key));
                }
                (Some(existing), added) => self.value(existing, added, path),
            }
        }
        self.drop_stale(into, from, prefix);
    }

    /// A key both files set. A profile value yields to a real one; two profile values are united.
    fn value(&mut self, existing: &mut Item, added: &Item, path: String) {
        match (marker(existing), marker(added)) {
            (Some(old), Some(new)) => {
                let mut tools = old.clone();
                for tool in new {
                    if !tools.contains(&tool) {
                        tools.push(tool);
                    }
                }
                if tools != old {
                    set_marker(existing, &tools);
                }
            }
            (Some(_), None) => *existing = added.clone(),
            (None, Some(_)) => self.left.push(path),
            (None, None) => {
                if !same(existing, added) {
                    self.kept.push(path);
                }
            }
        }
    }

    /// Unites a list both files set: an entry of `added` that names an entry of `existing` (a string and a
    /// `{ name, reason }` table with that `name` are the same entry, and so are `x` and `x!` in the lists of
    /// [`MarkerRule`]) is merged into it, any other entry is appended.
    fn list(&mut self, existing: &mut Array, added: &Array, path: &str, rule: MarkerRule) {
        for value in added {
            if existing.iter().any(|present| same(present, value)) {
                continue;
            }
            let Some((name, _)) = named(value) else {
                existing.push_formatted(value.clone());
                continue;
            };
            let body = rule.split(name).0;
            let held = existing
                .iter()
                .position(|present| named(present).is_some_and(|(other, _)| rule.split(other).0 == body));
            match held {
                Some(index) => self.unite(existing, index, value, path, rule),
                None => existing.push_formatted(value.clone()),
            }
        }
    }

    /// Merges `added` into the entry at `index` of `list`, which has the same name: the production marker follows
    /// `rule`, the existing reason is kept and a missing one is taken from `added`. The entry keeps its place and
    /// its formatting.
    fn unite(&mut self, list: &mut Array, index: usize, added: &Value, path: &str, rule: MarkerRule) {
        let (Some((held_name, held_reason)), Some((added_name, added_reason))) =
            (list.get(index).and_then(named), named(added))
        else {
            return;
        };
        let (body, held_marked) = rule.split(held_name);
        let marked = rule.unite(held_marked, rule.split(added_name).1);
        let name = if marked { format!("{body}!") } else { body.to_owned() };
        let reason = held_reason.or(added_reason);
        if held_reason.is_some_and(|held| added_reason.is_some_and(|new| new != held)) {
            push_once(&mut self.kept, path);
        }
        if marked && !held_marked {
            push_once(&mut self.replaced, path);
        }
        if name == held_name && reason == held_reason {
            return;
        }
        let value = if name == added_name && reason == added_reason {
            added.clone()
        } else {
            entry_value(&name, reason)
        };
        if let Some(slot) = list.get_mut(index) {
            let decor = slot.decor().clone();
            *slot = value;
            *slot.decor_mut() = decor;
        }
    }

    /// Removes the profile values of `into` that the generated table does not repeat: the migrated tools run those
    /// checks, so the rule is back to its default.
    fn drop_stale(&mut self, into: &mut Table, from: &Table, prefix: &str) {
        let stale: Vec<String> = into
            .iter()
            .filter(|(key, item)| marker(item).is_some() && from.get(key).is_none_or(|added| marker(added).is_none()))
            .map(|(key, _)| key.to_owned())
            .collect();
        for key in stale {
            into.remove(&key);
        }
        let missing: Vec<String> = into
            .iter()
            .filter(|(key, item)| item.is_table() && from.get(key).is_none())
            .map(|(key, _)| key.to_owned())
            .collect();
        for key in missing {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            let emptied = match into.get_mut(&key) {
                Some(Item::Table(table)) if !table.is_empty() => {
                    self.table(table, &Table::new(), &path);
                    table.is_empty()
                }
                _ => false,
            };
            if emptied {
                into.remove(&key);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use skif_config::Config;

    use super::*;

    fn generated(body: &str, tools: &[Tool], profile: Profile) -> Migration {
        Migration {
            toml: format!("{}{body}", banner_for_tests(tools)),
            config: Config::default(),
            notes: Vec::new(),
            extra_checks: Vec::new(),
            profile,
            tools: tools.to_vec(),
            body: body.to_owned(),
        }
    }

    fn banner_for_tests(tools: &[Tool]) -> String {
        format!("{}\n", sources_line(tools))
    }

    fn sample() -> Migration {
        generated(
            "\n[lint]\nfail-on-warnings = true\n\n[[ignore]]\ndependencies = [\"react\"]\n",
            &[Tool::Sherif],
            Profile::Full,
        )
    }

    #[test]
    fn new_tables_are_appended_after_the_existing_text_under_a_banner() {
        let existing = "# mine\n[workspace]\nignore-packages = [\"a\"]\n";
        let merged = merge_into(&sample(), existing).unwrap();
        assert_eq!(
            merged.toml,
            "# mine\n[workspace]\nignore-packages = [\"a\"]\n\n\
             # Added by `skif migrate --merge` from sherif (profile: full).\n\n\
             [lint]\nfail-on-warnings = true\n\n\
             [[ignore]]\ndependencies = [\"react\"]\n\
             # skif migrate: sources = [\"sherif\"]\n"
        );
    }

    #[test]
    fn existing_tables_keep_their_order_and_comments() {
        let existing =
            "[js]\nentry = [\"a\"]\n\n[rules] # inline\nunused-files = \"warn\"\n\n[[version-groups]]\nlabel = \"x\"\n";
        let migration = generated(
            "\n[workspace]\nignore-packages = [\"a\"]\n\n[lint]\nfail-on-warnings = true\n\n[rules]\nversion-mismatch = \"warn\"\n",
            &[Tool::Sherif],
            Profile::Full,
        );
        let merged = merge_into(&migration, existing).unwrap();
        let order: Vec<usize> = [
            "[js]",
            "[rules] # inline",
            "[[version-groups]]",
            "[workspace]",
            "[lint]",
        ]
        .iter()
        .map(|header| merged.toml.find(header).unwrap_or(usize::MAX))
        .collect();
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{}", merged.toml);
        assert!(merged.toml.contains("version-mismatch = \"warn\""), "{}", merged.toml);
        assert_eq!(merged.toml.matches("#:schema").count(), 0, "{}", merged.toml);
        assert_eq!(merged.toml.matches("Added by").count(), 1, "{}", merged.toml);
    }

    #[test]
    fn merging_the_output_into_itself_changes_nothing() {
        let migration = sample();
        let merged = merge_into(&migration, &migration.toml).unwrap();
        assert_eq!(merged.toml, migration.toml);
        assert_eq!(merged.kept_existing, Vec::<String>::new());
    }

    #[test]
    fn an_existing_value_wins_over_a_different_one() {
        let existing = "[lint]\nfail-on-warnings = false\n";
        let merged = merge_into(&sample(), existing).unwrap();
        assert_eq!(merged.kept_existing, ["lint.fail-on-warnings"]);
        assert!(merged.toml.contains("fail-on-warnings = false"));
        assert!(merged.toml.contains("react"));
    }

    #[test]
    fn invalid_existing_text_is_an_error() {
        let result = merge_into(&sample(), "[lint");
        assert!(matches!(result, Err(MigrateError::Existing(_))));
    }

    fn profile_off(tools: &[Tool], body: &str) -> Migration {
        generated(body, tools, Profile::Exact)
    }

    #[test]
    fn a_profile_value_yields_to_a_rule_the_new_tool_checks() {
        let first = profile_off(
            &[Tool::Sherif],
            "\n[js]\nenabled = false # skif migrate: not checked by sherif\n\n[rules]\nunused-files = \"off\" # skif migrate: not checked by sherif\n",
        );
        let second = profile_off(
            &[Tool::Knip],
            "\n[js]\nentry = [\"a\"]\n\n[rules]\nunused-files = \"error\"\n",
        );
        let merged = merge_into(&second, &first.toml).unwrap();
        assert!(merged.toml.contains("unused-files = \"error\""), "{}", merged.toml);
        assert!(!merged.toml.contains("enabled"), "{}", merged.toml);
        assert!(merged.toml.contains("entry = [\"a\"]"), "{}", merged.toml);
        assert_eq!(merged.kept_existing, Vec::<String>::new());
        assert!(
            merged.toml.contains("sources = [\"sherif\", \"knip\"]"),
            "{}",
            merged.toml
        );
    }

    #[test]
    fn a_profile_value_for_a_rule_an_earlier_tool_checks_is_not_added() {
        let first = profile_off(
            &[Tool::Sherif],
            "\n[rules]\nunused-files = \"off\" # skif migrate: not checked by sherif\n",
        );
        let second = profile_off(
            &[Tool::Knip],
            "\n[rules]\nunused-files = \"error\"\nversion-mismatch = \"off\" # skif migrate: not checked by knip\n",
        );
        let merged = merge_into(&second, &first.toml).unwrap();
        assert!(!merged.toml.contains("version-mismatch"), "{}", merged.toml);
    }

    #[test]
    fn two_profile_values_name_both_tools() {
        let first = profile_off(
            &[Tool::Sherif],
            "\n[rules]\nunused-files = \"off\" # skif migrate: not checked by sherif\n",
        );
        let second = profile_off(
            &[Tool::Syncpack],
            "\n[rules]\nunused-files = \"off\" # skif migrate: not checked by syncpack\n",
        );
        let merged = merge_into(&second, &first.toml).unwrap();
        assert!(
            merged
                .toml
                .contains("unused-files = \"off\" # skif migrate: not checked by sherif, syncpack"),
            "{}",
            merged.toml
        );
    }

    #[test]
    fn a_profile_value_the_new_profile_leaves_out_is_removed() {
        let first = profile_off(
            &[Tool::Knip],
            "\n[rules]\nversion-mismatch = \"off\" # skif migrate: not checked by knip\n",
        );
        let second = profile_off(&[Tool::Sherif], "\n[lint]\nfail-on-warnings = true\n");
        let merged = merge_into(&second, &first.toml).unwrap();
        assert!(!merged.toml.contains("version-mismatch"), "{}", merged.toml);
        assert!(!merged.toml.contains("[rules]"), "{}", merged.toml);
    }

    #[test]
    fn a_real_value_beats_a_profile_value_of_the_new_run() {
        let existing = "[rules]\nunused-files = \"warn\"\n";
        let second = profile_off(
            &[Tool::Sherif],
            "\n[rules]\nunused-files = \"off\" # skif migrate: not checked by sherif\n",
        );
        let merged = merge_into(&second, existing).unwrap();
        assert!(merged.toml.contains("unused-files = \"warn\""), "{}", merged.toml);
        assert_eq!(merged.kept_existing, Vec::<String>::new());
        assert_eq!(merged.left_as_existing, ["rules.unused-files"]);
    }

    #[test]
    fn a_file_without_a_sources_line_keeps_running_the_default_rules_it_does_not_name() {
        let existing = "[rules]\nversion-mismatch = \"error\"\n";
        let knip = profile_off(
            &[Tool::Knip],
            "\n[rules]\nunused-dependencies = \"error\"\nstray-lockfile = \"off\" # skif migrate: not checked by knip\n\n\
             [catalog]\nreport-unused-entries = true\n",
        );
        let merged = merge_into(&knip, existing).unwrap();
        assert_eq!(
            merged.toml,
            "[rules]\nversion-mismatch = \"error\"\nunused-dependencies = \"error\"\n\n\
             # Added by `skif migrate --merge` from knip (profile: exact).\n\n\
             [catalog]\nreport-unused-entries = true\n\
             # skif migrate: sources = [\"knip\"]\n"
        );
        assert_eq!(merged.left_as_existing, ["rules.stray-lockfile"]);
        assert_eq!(merged.kept_existing, Vec::<String>::new());
    }

    #[test]
    fn a_table_the_existing_file_lacks_is_added_without_its_profile_values() {
        let existing = "[workspace]\nignore-packages = [\"a\"]\n";
        let sherif = profile_off(
            &[Tool::Sherif],
            "\n[js]\nenabled = false # skif migrate: not checked by sherif\n\n\
             [rules]\nversion-mismatch = \"warn\"\nunused-files = \"off\" # skif migrate: not checked by sherif\n",
        );
        let merged = merge_into(&sherif, existing).unwrap();
        assert!(!merged.toml.contains("[js]"), "{}", merged.toml);
        assert!(!merged.toml.contains("unused-files"), "{}", merged.toml);
        assert!(
            merged.toml.contains("[rules]\nversion-mismatch = \"warn\"\n"),
            "{}",
            merged.toml
        );
        assert_eq!(merged.left_as_existing, ["js.enabled", "rules.unused-files"]);
    }

    #[test]
    fn an_empty_existing_file_takes_the_profile_values() {
        let knip = profile_off(
            &[Tool::Knip],
            "\n[rules]\nstray-lockfile = \"off\" # skif migrate: not checked by knip\n",
        );
        let merged = merge_into(&knip, "# nothing yet\n").unwrap();
        assert!(
            merged
                .toml
                .contains("stray-lockfile = \"off\" # skif migrate: not checked by knip"),
            "{}",
            merged.toml
        );
        assert_eq!(merged.left_as_existing, Vec::<String>::new());
    }

    #[test]
    fn the_merge_summary_lists_the_checks_left_as_the_existing_file_has_them() {
        let sherif = profile_off(
            &[Tool::Sherif],
            "\n[rules]\nunused-files = \"off\" # skif migrate: not checked by sherif\n",
        );
        let mut syncpack = profile_off(
            &[Tool::Syncpack],
            "\n[rules]\nunused-files = \"off\" # skif migrate: not checked by syncpack\nstray-lockfile = \"off\" # skif migrate: not checked by syncpack\n",
        );
        syncpack.extra_checks = vec!["unused-files".to_owned(), "stray-lockfile".to_owned()];
        let merged = merge_into(&syncpack, &sherif.toml).unwrap();
        assert_eq!(merged.left_as_existing, ["rules.stray-lockfile"]);
        assert!(
            syncpack.merge_summary(&merged).ends_with(
                "Checks skif runs that the old tools did not (2):\n  \
                 turned off in [rules] by --profile exact: unused-files\n  \
                 left as the existing configuration has them by --merge: stray-lockfile\n"
            ),
            "{}",
            syncpack.merge_summary(&merged)
        );
        assert!(
            syncpack.summary().ends_with(
                "Checks skif runs that the old tools did not (2): turned off in [rules] by --profile exact\n  \
                 unused-files, stray-lockfile\n"
            ),
            "{}",
            syncpack.summary()
        );
    }

    #[test]
    fn migrating_knip_turns_the_javascript_analysis_back_on() {
        let knip = profile_off(&[Tool::Knip], "\n[js]\nignore-dependencies = [\"left-pad\"]\n");
        for (existing, expected) in [
            ("[js]\nenabled = false # mine\n", "[js]\nenabled = true # mine\n"),
            ("js.enabled = false\n", "js.enabled = true\n"),
        ] {
            let merged = merge_into(&knip, existing).unwrap();
            assert!(merged.toml.starts_with(expected), "{existing:?}: {}", merged.toml);
            assert!(merged.toml.contains("left-pad"), "{}", merged.toml);
            assert_eq!(merged.replaced_existing, ["js.enabled"], "{existing:?}");
        }
    }

    fn merged_knip(existing: &str, body: &str) -> Merged {
        merge_into(&profile_off(&[Tool::Knip], body), existing).unwrap()
    }

    #[test]
    fn a_production_entry_replaces_the_plain_entry_of_the_same_glob() {
        for key in ["entry", "project"] {
            let existing = format!("[js.workspaces.app]\n{key} = [\"src/main.js\", \"bin/*.js\"] # mine\n");
            let merged = merged_knip(
                &existing,
                &format!("\n[js.workspaces.app]\n{key} = [\"src/main.js!\", \"scripts/*.js\"]\n"),
            );
            assert!(
                merged.toml.starts_with(&format!(
                    "[js.workspaces.app]\n{key} = [\"src/main.js!\", \"bin/*.js\", \"scripts/*.js\"] # mine\n"
                )),
                "{key}: {}",
                merged.toml
            );
            assert_eq!(merged.replaced_existing, [format!("js.workspaces.app.{key}")], "{key}");
            assert_eq!(merged.kept_existing, Vec::<String>::new(), "{key}");
        }
    }

    #[test]
    fn an_existing_production_entry_stays_when_the_migration_has_the_plain_glob() {
        let existing = "[js]\nentry = [\"src/main.js!\"]\n";
        let merged = merged_knip(existing, "\n[js]\nentry = [\"src/main.js\"]\n");
        assert!(merged.toml.starts_with(existing), "{}", merged.toml);
        assert_eq!(merged.replaced_existing, Vec::<String>::new());
        assert_eq!(merged.kept_existing, Vec::<String>::new());
    }

    #[test]
    fn an_ignore_entry_for_every_mode_covers_the_production_only_one() {
        for (existing, added, expected) in [
            ("[\"left-pad\"]", "[\"left-pad!\"]", "[\"left-pad\"]"),
            ("[\"left-pad!\"]", "[\"left-pad\"]", "[\"left-pad\"]"),
            ("[\"left-pad!\"]", "[\"left-pad!\"]", "[\"left-pad!\"]"),
        ] {
            let merged = merged_knip(
                &format!("[js]\nignore-dependencies = {existing}\n"),
                &format!("\n[js]\nignore-dependencies = {added}\n"),
            );
            assert!(
                merged
                    .toml
                    .starts_with(&format!("[js]\nignore-dependencies = {expected}\n")),
                "{existing} + {added}: {}",
                merged.toml
            );
            assert_eq!(merged.replaced_existing, Vec::<String>::new());
        }
    }

    #[test]
    fn a_plain_list_entry_takes_the_reason_of_the_same_entry() {
        let merged = merged_knip(
            "[js]\nignore-workspaces = [\"playground/**\", \"tools/*\"]\n",
            "\n[js]\nignore-workspaces = [{ name = \"playground/**\", reason = \"test suites\" }]\n",
        );
        assert!(
            merged.toml.starts_with(
                "[js]\nignore-workspaces = [{ name = \"playground/**\", reason = \"test suites\" }, \"tools/*\"]\n"
            ),
            "{}",
            merged.toml
        );
        assert_eq!(merged.kept_existing, Vec::<String>::new());
        assert_eq!(merged.replaced_existing, Vec::<String>::new());
    }

    #[test]
    fn an_entry_with_a_reason_is_not_replaced_by_the_plain_form() {
        let existing = "[js]\nignore-workspaces = [{ name = \"playground/**\", reason = \"mine\" }]\n";
        for added in ["\"playground/**\"", "{ name = \"playground/**\", reason = \"mine\" }"] {
            let merged = merged_knip(existing, &format!("\n[js]\nignore-workspaces = [{added}]\n"));
            assert!(merged.toml.starts_with(existing), "{added}: {}", merged.toml);
            assert_eq!(merged.kept_existing, Vec::<String>::new(), "{added}");
        }
    }

    #[test]
    fn a_different_reason_for_the_same_entry_keeps_the_existing_one() {
        let existing = "[js]\nignore-dependencies = [{ name = \"left-pad\", reason = \"mine\" }]\n";
        let merged = merged_knip(
            existing,
            "\n[js]\nignore-dependencies = [{ name = \"left-pad\", reason = \"theirs\" }]\n",
        );
        assert!(merged.toml.starts_with(existing), "{}", merged.toml);
        assert_eq!(merged.kept_existing, ["js.ignore-dependencies"]);
    }

    #[test]
    fn an_ignore_entry_for_every_mode_keeps_the_reason_of_the_production_only_one() {
        let merged = merged_knip(
            "[js]\nignore-binaries = [{ name = \"tsc!\", reason = \"mine\" }]\n",
            "\n[js]\nignore-binaries = [\"tsc\"]\n",
        );
        let parsed: toml::Table = merged.toml.parse().unwrap();
        let list = parsed["js"]["ignore-binaries"].as_array().unwrap();
        assert_eq!(list.len(), 1, "{}", merged.toml);
        assert_eq!(list[0]["name"].as_str(), Some("tsc"), "{}", merged.toml);
        assert_eq!(list[0]["reason"].as_str(), Some("mine"), "{}", merged.toml);
    }

    #[test]
    fn the_marker_is_part_of_the_entry_outside_the_js_lists() {
        let merged = merge_into(
            &generated(
                "\n[workspace]\nignore-packages = [\"a!\"]\n",
                &[Tool::Sherif],
                Profile::Full,
            ),
            "[workspace]\nignore-packages = [\"a\"]\n",
        )
        .unwrap();
        assert!(
            merged
                .toml
                .starts_with("[workspace]\nignore-packages = [\"a\",\"a!\"]\n"),
            "{}",
            merged.toml
        );
    }

    #[test]
    fn migrating_another_tool_leaves_the_javascript_analysis_off() {
        let sherif = profile_off(&[Tool::Sherif], "\n[lint]\nfail-on-warnings = true\n");
        let merged = merge_into(&sherif, "[js]\nenabled = false\n").unwrap();
        assert!(merged.toml.starts_with("[js]\nenabled = false\n"), "{}", merged.toml);
        assert_eq!(merged.replaced_existing, Vec::<String>::new());
    }
}
