//! `skif config`: the files a root's configuration is built from, the merged result, and where each key came from.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value, json};
use skif::{
    Assignment, CachedFs, Config, ConfigOverrides, ConfigSource, Op, OsFs, SourceKind, Target, slash,
    unapplied_config_files_all,
};

use crate::args::{ConfigArgs, Format, RulesFormat};
use crate::config_toml::{self, Item};
use crate::error::{CliError, Kind};
use crate::lint::canonical;
use crate::output::{emit_error, stderr_line, write_stdout};
use crate::roots::{self, Resolved, RootError};

const CLEAN: u8 = 0;
const COULD_NOT_RUN: u8 = 2;

/// `path` relative to `base` with forward slashes, `..` segments included; the full path when the two share nothing.
pub fn relative(base: &Path, path: &Path) -> String {
    let base: Vec<Component<'_>> = base.components().collect();
    let target: Vec<Component<'_>> = path.components().collect();
    let common = base
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return slash(path);
    }
    let mut out = PathBuf::new();
    for _ in common..base.len() {
        out.push("..");
    }
    for component in &target[common..] {
        out.push(component.as_os_str());
    }
    if out.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        slash(&out)
    }
}

pub fn run(args: &ConfigArgs) -> u8 {
    let format = Format::from(args.format);
    match execute(args) {
        Ok(code) => code,
        Err(error) => {
            emit_error(&error, format);
            COULD_NOT_RUN
        }
    }
}

fn to_value<T: Serialize>(value: &T) -> Result<Value, CliError> {
    serde_json::to_value(value).map_err(|error| CliError::new(Kind::Io, error.to_string()))
}

fn execute(args: &ConfigArgs) -> Result<u8, CliError> {
    let fs = CachedFs::new(OsFs);
    let start = canonical(&args.path)?;
    let Resolved {
        base,
        targets,
        multi,
        failures,
        ..
    } = roots::resolve(&start, args.recursive, &fs, &ConfigOverrides::default())?;
    let mut unapplied = if args.explain.is_none() {
        unapplied_config_files_all(&targets, &fs)
    } else {
        Vec::new()
    };
    unapplied.resize_with(targets.len(), Vec::new);
    let views: Vec<RootView<'_>> = targets
        .iter()
        .zip(unapplied)
        .map(|(target, files)| RootView {
            target,
            path: relative(&base, &target.root),
            unapplied: files
                .into_iter()
                .map(|file| Unapplied {
                    path: relative(&base, &file.path),
                    reason: file.reason.describe(),
                })
                .collect(),
        })
        .collect();
    let text = match (&args.explain, args.format) {
        (Some(key), format) => explain(&views, &base, key, args.package.as_deref(), multi, format, &failures)?,
        (None, RulesFormat::Human) => show_human(&views, &base)?,
        (None, RulesFormat::Json) => show_json(&views, &base, &failures)?,
    };
    write_stdout(text.as_bytes())?;
    if args.format == RulesFormat::Human {
        for failure in &failures {
            stderr_line(&format!("error: {}", failure.describe()));
        }
    }
    Ok(if failures.is_empty() { CLEAN } else { COULD_NOT_RUN })
}

/// A configuration file below a root that its configuration was not built from.
struct Unapplied {
    path: String,
    reason: &'static str,
}

struct RootView<'a> {
    target: &'a Target,
    path: String,
    unapplied: Vec<Unapplied>,
}

fn kind_label(kind: &SourceKind) -> &'static str {
    match kind {
        SourceKind::Root => "root",
        SourceKind::Ancestor => "ancestor",
        SourceKind::Extends { .. } => "extends",
        SourceKind::Explicit => "explicit",
        SourceKind::Package => "package",
        _ => "unknown",
    }
}

fn describe_source(base: &Path, source: &ConfigSource) -> String {
    let path = relative(base, &source.path);
    match &source.kind {
        SourceKind::Extends { by } => format!("{path} (extended by {})", relative(base, by)),
        other => format!("{path} ({})", kind_label(other)),
    }
}

fn source_json(base: &Path, source: &ConfigSource) -> Value {
    let mut entry = json!({
        "path": relative(base, &source.path),
        "kind": kind_label(&source.kind),
    });
    if let SourceKind::Extends { by } = &source.kind {
        entry["extended-by"] = json!(relative(base, by));
    }
    entry
}

fn strip_nulls(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(key, value)| (key, strip_nulls(value)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(strip_nulls).collect()),
        other => other,
    }
}

/// The key an origin is recorded under for the value or header at `key` of the rendered configuration: a rule is
/// recorded as `rules.ID.level` and `rules.ID.options`, whatever form it is written in.
fn origin_key(key: &str, item: Item) -> Option<String> {
    let Some(rest) = key.strip_prefix("rules.") else {
        return Some(key.to_owned());
    };
    let (id, field) = rest
        .split_once('.')
        .map_or((rest, None), |(id, field)| (id, Some(field)));
    match (field, item) {
        (None, Item::Header) => None,
        (None, Item::Value) | (Some("level"), _) => Some(format!("rules.{id}.level")),
        (Some(_), _) => Some(format!("rules.{id}.options")),
    }
}

/// The files that decided the value at `key`: the last file that set it and the ones that appended after it.
fn origin_text(target: &Target, base: &Path, key: &str, item: Item) -> Option<String> {
    let assignments = target.origins.get(&origin_key(key, item)?);
    let start = assignments
        .iter()
        .rposition(|assignment| matches!(assignment.op, Op::Set | Op::Reset))
        .unwrap_or(0);
    let mut places: Vec<String> = Vec::new();
    for assignment in assignments[start..]
        .iter()
        .filter(|assignment| assignment.op != Op::Reset)
    {
        let located = Located::of(target, base, assignment);
        if located.path.is_empty() {
            continue;
        }
        let place = located.short();
        if !places.contains(&place) {
            places.push(place);
        }
    }
    (!places.is_empty()).then(|| places.join(", "))
}

fn config_toml(target: &Target, base: &Path) -> Result<String, CliError> {
    let value = strip_nulls(to_value(&target.config)?);
    let table = toml::Value::try_from(&value).map_err(|error| CliError::new(Kind::Io, error.to_string()))?;
    let toml::Value::Table(table) = table else {
        return Err(CliError::new(Kind::Io, "the configuration is not a table".to_owned()));
    };
    Ok(config_toml::render(&table, &|key, item| {
        origin_text(target, base, key, item)
    }))
}

fn show_human(views: &[RootView<'_>], base: &Path) -> Result<String, CliError> {
    let mut out = String::new();
    for (index, view) in views.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let _ = writeln!(out, "# root: {}", view.path);
        out.push_str("# sources, in the order applied:\n");
        if view.target.sources.is_empty() {
            out.push_str("#   (none: the built-in defaults)\n");
        }
        for (number, source) in view.target.sources.iter().enumerate() {
            let _ = writeln!(out, "#   {}. {}", number + 1, describe_source(base, source));
        }
        if !view.unapplied.is_empty() {
            out.push_str("# configuration files not applied:\n");
            for file in &view.unapplied {
                let _ = writeln!(out, "#   {} ({})", file.path, file.reason);
            }
        }
        out.push('\n');
        out.push_str(&config_toml(view.target, base)?);
    }
    Ok(out)
}

pub(crate) fn document(mut members: Map<String, Value>) -> Result<String, CliError> {
    let mut document = Map::new();
    document.insert("version".to_owned(), json!(1));
    document.insert("skif".to_owned(), json!(env!("CARGO_PKG_VERSION")));
    document.append(&mut members);
    let mut text = serde_json::to_string_pretty(&Value::Object(document))
        .map_err(|error| CliError::new(Kind::Io, error.to_string()))?;
    text.push('\n');
    Ok(text)
}

pub(crate) fn with_errors(members: &mut Map<String, Value>, failures: &[RootError]) {
    if !failures.is_empty() {
        members.insert(
            "errors".to_owned(),
            Value::Array(failures.iter().map(RootError::to_json).collect()),
        );
    }
}

/// Every key some file set, with the assignments that built it, oldest first.
fn origins_json(target: &Target, base: &Path) -> Value {
    Value::Object(
        target
            .origins
            .iter()
            .map(|(key, assignments)| {
                (
                    key.to_owned(),
                    Value::Array(
                        locations(target, base, assignments)
                            .iter()
                            .map(Located::to_json)
                            .collect(),
                    ),
                )
            })
            .collect(),
    )
}

fn show_json(views: &[RootView<'_>], base: &Path, failures: &[RootError]) -> Result<String, CliError> {
    let roots = views
        .iter()
        .map(|view| {
            Ok(json!({
                "path": view.path,
                "sources": view.target.sources.iter().map(|source| source_json(base, source)).collect::<Vec<_>>(),
                "not-applied": view
                    .unapplied
                    .iter()
                    .map(|file| json!({"path": file.path, "reason": file.reason}))
                    .collect::<Vec<_>>(),
                "config": to_value(&view.target.config)?,
                "origins": origins_json(view.target, base),
            }))
        })
        .collect::<Result<Vec<Value>, CliError>>()?;
    let mut members = Map::new();
    members.insert("roots".to_owned(), Value::Array(roots));
    with_errors(&mut members, failures);
    document(members)
}

/// One place that set a key, with the file relative to the run's base.
struct Located {
    path: String,
    line: usize,
    column: usize,
    op: &'static str,
}

impl Located {
    fn of(target: &Target, base: &Path, assignment: &Assignment) -> Self {
        Self {
            path: target
                .sources
                .get(assignment.source)
                .map_or_else(String::new, |source| relative(base, &source.path)),
            line: assignment.line,
            column: assignment.column,
            op: match assignment.op {
                Op::Append => "append",
                Op::Prepend => "prepend",
                Op::Reset => "reset",
                _ => "set",
            },
        }
    }

    /// `file:line` for a comment; the file alone when the line is unknown.
    fn short(&self) -> String {
        if self.line == 0 {
            self.path.clone()
        } else {
            format!("{}:{}", self.path, self.line)
        }
    }

    fn position(&self) -> String {
        if self.line == 0 {
            self.path.clone()
        } else {
            format!("{}:{}:{}", self.path, self.line, self.column)
        }
    }

    fn to_json(&self) -> Value {
        json!({"path": self.path, "line": self.line, "column": self.column, "op": self.op})
    }
}

/// A key with its final value and every assignment, oldest first.
struct Explained {
    key: String,
    value: Option<Value>,
    set_by: Vec<Located>,
}

struct PackageOverrides {
    name: String,
    dir: String,
    overrides: Vec<(usize, Located)>,
}

struct RootExplanation {
    path: String,
    entries: Vec<Explained>,
    package: Option<PackageOverrides>,
}

/// The value of a dotted key, with `name[index]` segments for the entries of a list.
fn value_of(config: &Config, json: &Value, key: &str) -> Option<Value> {
    if let Some((id, field)) = key.strip_prefix("rules.").and_then(|rest| rest.rsplit_once('.')) {
        let rule = config.rules.get(id)?;
        return match field {
            "level" => serde_json::to_value(rule.severity).ok(),
            "options" => serde_json::to_value(&rule.options).ok(),
            _ => None,
        };
    }
    let mut current = json;
    for segment in key.split('.') {
        let (name, indices) = split_indices(segment)?;
        if !name.is_empty() {
            current = current.get(name)?;
        }
        for index in indices {
            current = current.get(index)?;
        }
    }
    Some(current.clone())
}

fn split_indices(segment: &str) -> Option<(&str, Vec<usize>)> {
    let Some(open) = segment.find('[') else {
        return Some((segment, Vec::new()));
    };
    let (name, tail) = segment.split_at(open);
    let mut indices = Vec::new();
    for part in tail.split('[').skip(1) {
        indices.push(part.strip_suffix(']')?.parse().ok()?);
    }
    Some((name, indices))
}

fn locations(target: &Target, base: &Path, assignments: &[Assignment]) -> Vec<Located> {
    assignments
        .iter()
        .map(|assignment| Located::of(target, base, assignment))
        .collect()
}

/// The entries that explain `key`: the key itself, and when it is a table or a list the keys below it that some file
/// set. Empty when this root knows nothing of the key.
fn explain_root(target: &Target, base: &Path, key: &str) -> Result<Vec<Explained>, CliError> {
    let json = to_value(&target.config)?;
    let own = target.origins.get(key);
    let value = value_of(&target.config, &json, key);
    let below: Vec<(&str, &[Assignment])> = target
        .origins
        .iter()
        .filter(|(origin, _)| {
            origin
                .strip_prefix(key)
                .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
        })
        .collect();
    if own.is_empty() && value.is_none() && below.is_empty() {
        return Ok(Vec::new());
    }
    let mut entries = vec![Explained {
        key: key.to_owned(),
        value,
        set_by: locations(target, base, own),
    }];
    for (origin, assignments) in below {
        entries.push(Explained {
            key: origin.to_owned(),
            value: value_of(&target.config, &json, origin),
            set_by: locations(target, base, assignments),
        });
    }
    Ok(entries)
}

fn package_overrides(target: &Target, base: &Path, name: &str) -> Option<PackageOverrides> {
    let package = target
        .workspace
        .all_packages()
        .find(|package| package.name.as_deref() == Some(name))?;
    let dir = if package.dir.as_os_str().is_empty() {
        target.root.clone()
    } else {
        target.root.join(&package.dir)
    };
    let overrides = target
        .config
        .overrides
        .iter()
        .enumerate()
        .filter(|(_, scoped)| scoped.applies_to(Some((name, &dir)), None))
        .filter_map(|(index, scoped)| {
            let assignment = target
                .origins
                .last(&format!("overrides[{index}]"))
                .copied()
                .or_else(|| {
                    scoped.scope.as_ref().map(|scope| Assignment {
                        source: scope.source,
                        line: 0,
                        column: 0,
                        op: Op::Append,
                    })
                })?;
            Some((index, Located::of(target, base, &assignment)))
        })
        .collect();
    Some(PackageOverrides {
        name: name.to_owned(),
        dir: relative(base, &dir),
        overrides,
    })
}

fn flatten(value: &Value, prefix: &str, out: &mut BTreeSet<String>) {
    if let Value::Object(map) = value {
        for (key, child) in map {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            out.insert(path.clone());
            flatten(child, &path, out);
        }
    }
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut previous: Vec<usize> = (0..=right.len()).collect();
    for (row, left_char) in left.chars().enumerate() {
        let mut current = vec![row + 1];
        for (column, right_char) in right.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_char != *right_char);
            current.push(substitution.min(previous[column + 1] + 1).min(current[column] + 1));
        }
        previous = current;
    }
    previous[right.len()]
}

/// Keys close to `key`: a small edit distance, else the keys that share its parent table, else its first segment.
fn near_keys(key: &str, candidates: &BTreeSet<String>) -> Vec<String> {
    let limit = (key.len() / 4).max(3);
    let mut close: Vec<(usize, &String)> = candidates
        .iter()
        .map(|candidate| (edit_distance(candidate, key), candidate))
        .filter(|(distance, _)| *distance <= limit)
        .collect();
    close.sort();
    if !close.is_empty() {
        return close
            .into_iter()
            .take(5)
            .map(|(_, candidate)| candidate.clone())
            .collect();
    }
    let shares = |prefix: &str| -> Vec<String> {
        candidates
            .iter()
            .filter(|candidate| candidate.starts_with(prefix))
            .take(8)
            .cloned()
            .collect()
    };
    if let Some((parent, _)) = key.rsplit_once('.') {
        let siblings = shares(&format!("{parent}."));
        if !siblings.is_empty() {
            return siblings;
        }
    }
    let first = key.split(['.', '[']).next().unwrap_or(key);
    shares(&format!("{first}."))
}

fn unknown_key(key: &str, views: &[RootView<'_>]) -> CliError {
    let mut candidates = BTreeSet::new();
    for view in views {
        if let Ok(json) = to_value(&view.target.config) {
            flatten(&json, "", &mut candidates);
        }
        candidates.extend(view.target.origins.iter().map(|(origin, _)| origin.to_owned()));
    }
    let near = near_keys(key, &candidates);
    let hint = if near.is_empty() {
        "run `skif config --show` to see the effective configuration".to_owned()
    } else {
        format!(
            "did you mean {}?",
            near.iter()
                .map(|candidate| format!("`{candidate}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    CliError::new(Kind::Usage, format!("unknown configuration key `{key}`; {hint}"))
}

fn explain(
    views: &[RootView<'_>],
    base: &Path,
    key: &str,
    package: Option<&str>,
    multi: bool,
    format: RulesFormat,
    failures: &[RootError],
) -> Result<String, CliError> {
    let mut roots = Vec::with_capacity(views.len());
    for view in views {
        roots.push(RootExplanation {
            path: view.path.clone(),
            entries: explain_root(view.target, base, key)?,
            package: package.and_then(|name| package_overrides(view.target, base, name)),
        });
    }
    if roots.iter().all(|root| root.entries.is_empty()) {
        return Err(unknown_key(key, views));
    }
    if let Some(name) = package
        && roots.iter().all(|root| root.package.is_none())
    {
        return Err(CliError::new(
            Kind::Usage,
            format!("no package named `{name}` in the discovered workspace"),
        ));
    }
    match format {
        RulesFormat::Human => Ok(explain_human(&roots, multi)),
        RulesFormat::Json => explain_json(&roots, key, failures),
    }
}

fn value_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "(unset)".to_owned(),
        Some(value) => value.to_string(),
    }
}

fn explain_human(roots: &[RootExplanation], multi: bool) -> String {
    let mut out = String::new();
    for (index, root) in roots.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        if multi {
            let _ = writeln!(out, "# root: {}", root.path);
        }
        for (position, entry) in root.entries.iter().enumerate() {
            if position > 0 {
                out.push('\n');
            }
            let _ = writeln!(out, "{}", entry.key);
            let _ = writeln!(out, "  value: {}", value_text(entry.value.as_ref()));
            if entry.set_by.is_empty() {
                out.push_str("  set by: nothing (the built-in default)\n");
            } else {
                out.push_str("  set by, oldest first:\n");
                for located in &entry.set_by {
                    let _ = writeln!(out, "    {}  {}", located.position(), located.op);
                }
            }
        }
        if let Some(package) = &root.package {
            let _ = writeln!(out, "\npackage {} ({})", package.name, package.dir);
            out.push_str("  overrides that apply:\n");
            if package.overrides.is_empty() {
                out.push_str("    none\n");
            }
            for (index, located) in &package.overrides {
                let _ = writeln!(out, "    overrides[{index}]  {}", located.position());
            }
        }
    }
    out
}

fn explain_json(roots: &[RootExplanation], key: &str, failures: &[RootError]) -> Result<String, CliError> {
    let roots: Vec<Value> = roots
        .iter()
        .map(|root| {
            let entries: Vec<Value> = root
                .entries
                .iter()
                .map(|entry| {
                    json!({
                        "key": entry.key,
                        "value": entry.value.clone().unwrap_or(Value::Null),
                        "set-by": entry.set_by.iter().map(Located::to_json).collect::<Vec<_>>(),
                    })
                })
                .collect();
            let mut object = json!({"path": root.path, "entries": entries});
            if let Some(package) = &root.package {
                object["package"] = json!({
                    "name": package.name,
                    "dir": package.dir,
                    "overrides": package
                        .overrides
                        .iter()
                        .map(|(index, located)| {
                            let mut entry = located.to_json();
                            entry["index"] = json!(index);
                            entry
                        })
                        .collect::<Vec<_>>(),
                });
            }
            object
        })
        .collect();
    let mut members = Map::new();
    members.insert("key".to_owned(), json!(key));
    members.insert("roots".to_owned(), Value::Array(roots));
    with_errors(&mut members, failures);
    document(members)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_climb_with_dot_dot() {
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/b/c/d")), "c/d");
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/skif.toml")), "../skif.toml");
        assert_eq!(relative(Path::new("/a/b"), Path::new("/a/b")), ".");
        assert_eq!(relative(Path::new("/a/b/c"), Path::new("/a/x/y")), "../../x/y");
    }

    #[test]
    fn indices_split_off_list_segments() {
        assert_eq!(split_indices("version-groups[2]"), Some(("version-groups", vec![2])));
        assert_eq!(split_indices("lint"), Some(("lint", Vec::new())));
        assert_eq!(split_indices("a[x]"), None);
    }

    #[test]
    fn values_are_found_by_dotted_key_and_index() {
        let config = Config::default();
        let json = json!({"lint": {"fail-on-warnings": true}, "version-groups": [{"policy": "banned"}]});
        assert_eq!(value_of(&config, &json, "lint.fail-on-warnings"), Some(json!(true)));
        assert_eq!(
            value_of(&config, &json, "version-groups[0].policy"),
            Some(json!("banned"))
        );
        assert_eq!(value_of(&config, &json, "version-groups[1]"), None);
        assert_eq!(value_of(&config, &json, "lint.nope"), None);
    }

    #[test]
    fn near_keys_prefer_small_edits_then_siblings() {
        let candidates: BTreeSet<String> = ["lint.fail-on-warnings", "lint.max-level", "policy.prefer"]
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            near_keys("lint.fail-on-warning", &candidates),
            ["lint.fail-on-warnings"]
        );
        assert_eq!(
            near_keys("lint.completely-different-key", &candidates),
            ["lint.fail-on-warnings", "lint.max-level"]
        );
    }

    #[test]
    fn nulls_are_dropped_recursively() {
        assert_eq!(
            strip_nulls(json!({"a": null, "b": {"c": null, "d": 1}, "e": [{"f": null}]})),
            json!({"b": {"d": 1}, "e": [{}]})
        );
    }
}
