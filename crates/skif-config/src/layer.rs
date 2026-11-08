//! One configuration file read for layering: its raw table, the position of every key, and the scope checks that
//! need the positions.

use std::path::{Path, PathBuf};

use serde::Deserialize as _;
use skif_core::{FileSystem, LineIndex};
use toml::de::{DeTable, DeValue, Deserializer};
use toml::{Table, Value};

use crate::error::ConfigError;
use crate::merge::Spans;
use crate::model::Config;
use crate::provenance::join_key;

/// The keys an `[[overrides]]` entry has.
const OVERRIDE_KEYS: [&str; 6] = ["packages", "files", "max-level", "rules", "ignore", "js"];

/// Top-level keys that cannot appear in an override.
const ROOT_SCOPED: [&str; 16] = [
    "root",
    "extends",
    "reset",
    "workspace",
    "lint",
    "policy",
    "version-groups",
    "semver-groups",
    "custom-types",
    "catalog",
    "workspace-protocol",
    "dependency-families",
    "update",
    "update-groups",
    "format",
    "overrides",
];

/// `[js]` keys that cannot appear in an override.
pub(crate) const ROOT_SCOPED_JS: [&str; 11] = [
    "enabled",
    "cycles",
    "production",
    "include-entry-exports",
    "tsconfig",
    "use-tsconfig-files",
    "include-fixtures",
    "workspaces",
    "ignore-workspaces",
    "ignore-exports-used-in-file",
    "ignore-members",
];

/// A configuration file, parsed and validated on its own.
pub(crate) struct ParsedFile {
    /// The directory holding the file.
    pub dir: PathBuf,
    pub table: Table,
    pub spans: Spans,
    pub root: bool,
    pub extends: Vec<String>,
}

impl ParsedFile {
    pub(crate) fn read(path: &Path, fs: &dyn FileSystem) -> Result<Self, ConfigError> {
        let source = fs.read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&source, path)
    }

    pub(crate) fn parse(source: &str, path: &Path) -> Result<Self, ConfigError> {
        let from_toml = |error: &toml::de::Error| ConfigError::from_toml(error, source, Some(path));
        let document = DeTable::parse(source).map_err(|error| from_toml(&error))?;
        let mut spans = Spans::new();
        collect_table("", document.get_ref(), &LineIndex::new(source), &mut spans);
        check_overrides(document.get_ref(), &spans, path)?;
        Config::from_document(document.clone(), source, Some(path), false)?;
        let table = Table::deserialize(Deserializer::from(document)).map_err(|error| from_toml(&error))?;
        let root = table.get("root").and_then(Value::as_bool).unwrap_or(false);
        let extends: Vec<String> = table
            .get("extends")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default();
        Ok(Self {
            dir: path.parent().map(Path::to_path_buf).unwrap_or_default(),
            table,
            spans,
            root,
            extends,
        })
    }

    /// Position of the key `key`, a dotted path.
    pub(crate) fn position(&self, key: &str) -> Option<(usize, usize)> {
        self.spans.get(key).copied()
    }
}

fn collect_table(prefix: &str, table: &DeTable<'_>, index: &LineIndex, spans: &mut Spans) {
    for (key, value) in table {
        let name: &str = key.get_ref();
        let path = join_key(prefix, name);
        spans.insert(path.clone(), locate(index, key.span().start));
        collect_value(&path, value.get_ref(), index, spans);
    }
}

fn collect_value(path: &str, value: &DeValue<'_>, index: &LineIndex, spans: &mut Spans) {
    match value {
        DeValue::Table(table) => collect_table(path, table, index, spans),
        DeValue::Array(items) => {
            for (position, item) in items.iter().enumerate() {
                let element = format!("{path}[{position}]");
                spans.insert(element.clone(), locate(index, item.span().start));
                collect_value(&element, item.get_ref(), index, spans);
            }
        }
        _ => {}
    }
}

fn locate(index: &LineIndex, offset: usize) -> (usize, usize) {
    let (line, column) = index.line_col(u32::try_from(offset).unwrap_or(u32::MAX));
    (line as usize, column as usize)
}

/// Rejects root-scoped keys in `[[overrides]]` with an error that names the key, its position and where it belongs.
/// Keys no override has at all are left to the typed parse, which reports unknown keys.
fn check_overrides(document: &DeTable<'_>, spans: &Spans, path: &Path) -> Result<(), ConfigError> {
    let Some(items) = document.get("overrides").and_then(|value| value.get_ref().as_array()) else {
        return Ok(());
    };
    for (index, item) in items.iter().enumerate() {
        let Some(table) = item.get_ref().as_table() else {
            continue;
        };
        let prefix = format!("overrides[{index}]");
        for (key, value) in table {
            let name: &str = key.get_ref();
            let scoped = |key: String, hint: &str| {
                let position = spans.get(&key).copied();
                ConfigError::scope(key, Some(path), position, hint)
            };
            if ROOT_SCOPED.contains(&name) {
                return Err(scoped(format!("{prefix}.{name}"), &root_scoped_hint(name)));
            }
            if !OVERRIDE_KEYS.contains(&name) {
                continue;
            }
            match (name, value.get_ref()) {
                ("js", DeValue::Table(js)) => {
                    for (js_key, _) in js {
                        let js_name: &str = js_key.get_ref();
                        if ROOT_SCOPED_JS.contains(&js_name) {
                            return Err(scoped(
                                format!("{prefix}.js.{js_name}"),
                                &format!(
                                    "`js.{js_name}` applies to the whole root; set it in `[js]` of a configuration file"
                                ),
                            ));
                        }
                    }
                }
                ("rules", DeValue::Table(rules)) => {
                    for (id, rule) in rules {
                        let DeValue::Table(options) = rule.get_ref() else {
                            continue;
                        };
                        for (option, _) in options {
                            let option_name: &str = option.get_ref();
                            if !matches!(option_name, "level" | "severity") {
                                return Err(scoped(
                                    format!("{prefix}.rules.{}.{option_name}", id.get_ref()),
                                    "rule options are root-scoped; an override can only set rule levels, so set the options in `[rules]` of a configuration file",
                                ));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn root_scoped_hint(name: &str) -> String {
    match name {
        "lint" => "an override can cap severity with `max-level`; `[lint]` settings apply to the whole root".to_owned(),
        "root" | "extends" | "reset" | "overrides" => {
            format!("`{name}` applies to a whole configuration file, not to one override")
        }
        _ => format!("`{name}` applies to the whole root; set it at the top level of a configuration file"),
    }
}
