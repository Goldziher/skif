//! `skif baseline`: record the current findings as `[[ignore]]` entries in a baseline file that the root configuration
//! extends, remove the entries that match nothing, and fail when the baseline gained entries (ADR 0071).
//!
//! Which finding an entry matches is decided by [`ignore_matches`], the code a lint run suppresses with. Reading the
//! files, the git history and writing happen here.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Map, Value, json};
use skif::{
    CONFIG_FILE_NAMES, CachedFs, Config, ConfigOverrides, Diagnostic, IgnoreEntry, OsFs, Selector, Severity, Target,
    Workspace, dependency_name, ignore_matches, slash,
};

use crate::args::{
    BaselineArgs, BaselineCheckArgs, BaselineCommand, BaselineFileArgs, BaselineWriteArgs, Format, RulesFormat,
};
use crate::config::{document, relative};
use crate::error::{CliError, Kind};
use crate::lint::canonical;
use crate::output::{emit_error, stderr_line, write_stdout};
use crate::roots::{self, Linters};
use crate::write::write_atomic;

const CLEAN: u8 = 0;
const GREW: u8 = 1;
const COULD_NOT_RUN: u8 = 2;

/// The baseline file of a root when `--file` does not name one.
const DEFAULT_FILE: &str = "skif.baseline.toml";

/// The first lines of every baseline file.
const HEADER: &str = "\
# skif baseline: findings recorded by `skif baseline write`, suppressed until they are fixed.
# `skif baseline prune` removes the entries that match nothing; `skif baseline check` fails when entries are added.
# These commands rewrite the file: edit the reasons, not the layout.
";

pub fn run(args: &BaselineArgs) -> u8 {
    let (common, result) = match &args.command {
        BaselineCommand::Write(write) => (&write.common, write_baseline(write)),
        BaselineCommand::Prune(prune) => (prune, prune_baseline(prune)),
        BaselineCommand::Check(check) => (&check.common, check_baseline(check)),
    };
    match result {
        Ok(code) => code,
        Err(error) => {
            emit_error(&error, Format::from(common.format));
            COULD_NOT_RUN
        }
    }
}

/// What a baseline file holds: its text and its entries.
struct Baseline {
    text: String,
    entries: Vec<IgnoreEntry>,
}

/// Baseline entries paired with the indices of the findings each one matches.
type Matched<'a> = Vec<(&'a IgnoreEntry, &'a Vec<usize>)>;

/// The root at `PATH`, resolved as for linting, and the absolute path of its baseline file.
fn locate(args: &BaselineFileArgs) -> Result<(Target, PathBuf), CliError> {
    let start = canonical(&args.path)?;
    let target = skif::find_target(&start, &CachedFs::new(OsFs), &ConfigOverrides::default())?;
    let file = match &args.file {
        None => target.root.join(DEFAULT_FILE),
        Some(file) => resolve_file(file)?,
    };
    Ok((target, file))
}

/// `file` made absolute, with its directory spelled as the OS resolves it when it exists, so it compares equal to the
/// paths configuration discovery records.
fn resolve_file(file: &Path) -> Result<PathBuf, CliError> {
    let absolute = std::path::absolute(file).map_err(|error| CliError::io("cannot resolve", file, &error))?;
    let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) else {
        return Err(CliError::new(
            Kind::Usage,
            format!("--file: {} does not name a file", file.display()),
        ));
    };
    Ok(canonical(parent).map_or_else(|_| absolute.clone(), |dir| dir.join(name)))
}

fn config_error(path: &Path, message: &str) -> CliError {
    CliError {
        path: Some(slash(path)),
        ..CliError::new(Kind::Config, format!("{}: {message}", path.display()))
    }
}

/// The baseline file at `path`, or `None` when there is none.
fn read_baseline(path: &Path) -> Result<Option<Baseline>, CliError> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(CliError::io("cannot read", path, &error)),
    };
    let entries = parse_entries(&text).map_err(|message| config_error(path, &message))?;
    Ok(Some(Baseline { text, entries }))
}

/// The `[[ignore]]` entries of a baseline file. Any other key is an error: the commands rewrite the file whole.
fn parse_entries(text: &str) -> Result<Vec<IgnoreEntry>, String> {
    let table = text.parse::<toml::Table>().map_err(|error| error.to_string())?;
    if let Some(key) = table.keys().find(|key| *key != "ignore") {
        return Err(format!(
            "`{key}` is set, but a baseline file holds only `[[ignore]]` entries; move it to a configuration file"
        ));
    }
    Config::from_toml_str(text)
        .map(|config| config.ignore)
        .map_err(|error| error.to_string())
}

/// Lints `target` with its configuration minus the entries of the baseline file, and returns the workspace and the
/// findings.
fn lint_without(mut target: Target, baseline: &[IgnoreEntry]) -> Result<(Workspace, Vec<Diagnostic>), CliError> {
    let recorded: BTreeSet<(Key, Option<&str>)> = baseline
        .iter()
        .map(|entry| (key(entry), entry.reason.as_deref()))
        .collect();
    target
        .config
        .ignore
        .retain(|entry| !recorded.contains(&(key(entry), entry.reason.as_deref())));
    let root = target.root.clone();
    let linters = Linters::build(std::slice::from_ref(&target))?;
    let run = roots::lint_targets(&root, vec![target], &linters)?
        .into_iter()
        .next()
        .ok_or_else(|| CliError::new(Kind::Internal, "the root was not linted"))?;
    Ok((run.workspace, run.report.diagnostics))
}

/// What an entry is compared by: its rules, packages and dependencies, each sorted and without repeats, since neither
/// changes what the entry matches; not its reason.
type Key = [Vec<String>; 3];

fn key(entry: &IgnoreEntry) -> Key {
    let sorted = |values: &[String]| {
        let mut values = values.to_vec();
        values.sort_unstable();
        values.dedup();
        values
    };
    [
        sorted(&entry.rules),
        sorted(entry.packages.patterns()),
        sorted(entry.dependencies.patterns()),
    ]
}

/// `entries` sorted by their key, each key once: the first entry with it (an existing one before an added one) stays.
fn sorted_unique(mut entries: Vec<IgnoreEntry>) -> Vec<IgnoreEntry> {
    entries.sort_by_cached_key(key);
    entries.dedup_by(|later, earlier| key(later) == key(earlier));
    entries
}

/// `text` with the characters that are glob syntax in a selector written as one-character classes, so a pattern made
/// of it matches exactly that text. A leading `!`, which would make the pattern an exclusion, is written as the
/// one-branch alternation `{!}`: a class cannot hold it, since `[!` opens a negated class.
fn literal(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    let (negation, rest) = text.strip_prefix('!').map_or(("", text), |rest| ("{!}", rest));
    escaped.push_str(negation);
    for character in rest.chars() {
        if "*?[]{}".contains(character) {
            escaped.push('[');
            escaped.push(character);
            escaped.push(']');
        } else {
            escaped.push(character);
        }
    }
    escaped
}

/// Why a finding is left out of the baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unrecorded {
    /// Neither a package nor a dependency: its entry would turn the rule off for the whole workspace.
    WorkspaceWide,
    /// In the root package, which has no name of its own: its directory is the root itself, so no package pattern
    /// selects it alone, and an entry without one would cover every package.
    UnnamedRoot,
}

impl Unrecorded {
    /// The `cause` of the finding in the JSON document.
    const fn cause(self) -> &'static str {
        match self {
            Self::WorkspaceWide => "workspace-wide",
            Self::UnnamedRoot => "unnamed-root",
        }
    }

    /// What the warning on stderr says after the finding.
    const fn note(self) -> &'static str {
        match self {
            Self::WorkspaceWide => {
                "an `[[ignore]]` entry without a package or dependency would turn the rule off for the whole workspace"
            }
            Self::UnnamedRoot => {
                "the root package has no name of its own, so no `[[ignore]]` package pattern selects it alone and an \
                 entry would cover every package; give the root package a unique `name` to record it"
            }
        }
    }
}

/// The package pattern of a finding: the package's name, or its directory when another package has the same name;
/// `None` for a finding outside any package. A finding in a root package without a name of its own (none, or one
/// another package shares) is [`Unrecorded::UnnamedRoot`]: the root's directory is empty, so neither selects it.
fn package_pattern(workspace: &Workspace, diagnostic: &Diagnostic) -> Result<Option<String>, Unrecorded> {
    let Some(package) = diagnostic.package_id.and_then(|id| workspace.package(id)) else {
        return match diagnostic.package.as_deref() {
            Some(".") => Err(Unrecorded::UnnamedRoot),
            name => Ok(name.map(literal)),
        };
    };
    let shared = |name: &str| {
        workspace
            .iter()
            .filter(|(_, other)| other.name.as_deref() == Some(name))
            .count()
            > 1
    };
    match package.name.as_deref().filter(|name| !name.is_empty()) {
        Some(name) if !shared(name) => Ok(Some(literal(name))),
        _ => {
            let dir = package.dir.to_string_lossy();
            if dir.is_empty() {
                Err(Unrecorded::UnnamedRoot)
            } else {
                Ok(Some(literal(&dir)))
            }
        }
    }
}

/// The dependency pattern of a finding: `name@specifier` when it carries the specifier as written, so only that
/// version is silenced; else the dependency.
fn dependency_pattern(diagnostic: &Diagnostic) -> Option<String> {
    let dependency = diagnostic.dependency.as_deref()?;
    Some(match diagnostic.specifier.as_deref() {
        Some(specifier) => format!("{}@{}", literal(dependency_name(dependency)), literal(specifier)),
        None => literal(dependency),
    })
}

fn selector(pattern: Option<String>) -> Result<Selector, CliError> {
    Ok(match pattern {
        Some(pattern) => Selector::new([pattern])?,
        None => Selector::default(),
    })
}

/// The entry that records `diagnostic`, or why there is none: a finding with neither a package nor a dependency
/// would need an entry of its rule alone, which turns the rule off for the whole workspace, and one in an unnamed
/// root package an entry no package pattern narrows to it.
fn entry_for(
    workspace: &Workspace,
    diagnostic: &Diagnostic,
    reason: &str,
) -> Result<Result<IgnoreEntry, Unrecorded>, CliError> {
    let package = match package_pattern(workspace, diagnostic) {
        Ok(package) => package,
        Err(unrecorded) => return Ok(Err(unrecorded)),
    };
    let dependency = dependency_pattern(diagnostic);
    if package.is_none() && dependency.is_none() {
        return Ok(Err(Unrecorded::WorkspaceWide));
    }
    Ok(Ok(IgnoreEntry::default()
        .with_rules([diagnostic.rule.as_str().to_owned()])
        .with_packages(selector(package)?)
        .with_dependencies(selector(dependency)?)
        .with_reason(reason)))
}

fn quoted(text: &str) -> String {
    toml::Value::String(text.to_owned()).to_string()
}

fn render_list(out: &mut String, key: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    let items: Vec<String> = values.iter().map(String::as_str).map(quoted).collect();
    let _ = writeln!(out, "{key} = [{}]", items.join(", "));
}

/// The text of a baseline file holding `entries`, in their order.
fn render(entries: &[IgnoreEntry]) -> String {
    let mut out = HEADER.to_owned();
    for entry in entries {
        out.push_str("\n[[ignore]]\n");
        render_list(&mut out, "rules", &entry.rules);
        render_list(&mut out, "packages", entry.packages.patterns());
        render_list(&mut out, "dependencies", entry.dependencies.patterns());
        if let Some(reason) = &entry.reason {
            let _ = writeln!(out, "reason = {}", quoted(reason));
        }
    }
    out
}

/// `text` (a configuration file) with `entry` added to its `extends`: appended to the array, or written as a new
/// `extends = [...]` line at the top, after a `#:schema` line, so the rest of the file stays as it was.
fn with_extends(text: &str, entry: &str) -> Result<String, String> {
    let mut document = text
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| error.to_string())?;
    let appended = match document.get_mut("extends") {
        Some(item) => {
            let array = item
                .as_array_mut()
                .ok_or_else(|| "`extends` is not an array of paths".to_owned())?;
            array.push(entry);
            true
        }
        None => false,
    };
    let updated = if appended {
        document.to_string()
    } else {
        insert_extends(text, entry)
    };
    updated
        .parse::<toml_edit::DocumentMut>()
        .map_err(|error| error.to_string())?;
    Ok(updated)
}

fn insert_extends(text: &str, entry: &str) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let line = format!("extends = [{}]{newline}", quoted(entry));
    if text.trim().is_empty() {
        return line;
    }
    let (head, body) = match text.split_once('\n') {
        Some((first, rest)) if first.trim_start().starts_with("#:schema") => (format!("{first}\n"), rest),
        None if text.trim_start().starts_with("#:schema") => (format!("{text}{newline}"), ""),
        _ => (String::new(), text),
    };
    let separator = if body.trim().is_empty() || body.starts_with(['\n', '\r']) {
        ""
    } else {
        newline
    };
    format!("{head}{line}{separator}{body}")
}

/// Makes the configuration file of `root` (`skif.toml` or `.skif.toml`, created when there is none) extend `file`,
/// and returns its path.
fn add_extends(root: &Path, file: &Path) -> Result<PathBuf, CliError> {
    let config = CONFIG_FILE_NAMES
        .iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file())
        .unwrap_or_else(|| root.join(CONFIG_FILE_NAMES[0]));
    let text = match fs::read_to_string(&config) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(CliError::io("cannot read", &config, &error)),
    };
    let entry = relative(root, file);
    let updated = with_extends(&text, &entry).map_err(|message| config_error(&config, &message))?;
    write_atomic(&config, updated.as_bytes()).map_err(|error| CliError::io("cannot write", &config, &error))?;
    Ok(config)
}

/// Writes `text` to `path` unless the file already holds it; returns whether it was written.
fn write_if_changed(path: &Path, text: &str, current: Option<&str>) -> Result<bool, CliError> {
    if current == Some(text) {
        return Ok(false);
    }
    write_atomic(path, text.as_bytes()).map_err(|error| CliError::io("cannot write", path, &error))?;
    Ok(true)
}

fn entries_json(entries: &[&IgnoreEntry]) -> Value {
    Value::Array(
        entries
            .iter()
            .map(|entry| serde_json::to_value(entry).unwrap_or(Value::Null))
            .collect(),
    )
}

/// One line about `entry`: `version-mismatch: package @acme/web, dependency react@^17.0.2`.
fn describe(entry: &IgnoreEntry) -> String {
    let rules = if entry.rules.is_empty() {
        "every rule".to_owned()
    } else {
        entry.rules.join(", ")
    };
    let mut scope = Vec::new();
    if !entry.packages.is_empty() {
        scope.push(format!("package {}", entry.packages.patterns().join(", ")));
    }
    if !entry.dependencies.is_empty() {
        scope.push(format!("dependency {}", entry.dependencies.patterns().join(", ")));
    }
    if scope.is_empty() {
        rules
    } else {
        format!("{rules}: {}", scope.join(", "))
    }
}

fn counted(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn list_entries(out: &mut String, sign: char, entries: &[&IgnoreEntry]) {
    for entry in entries {
        let _ = writeln!(out, "  {sign} {}", describe(entry));
    }
}

/// The members every `skif baseline` JSON document starts with.
fn members(
    command: &str,
    file: &str,
    entries: usize,
    added: &[&IgnoreEntry],
    removed: &[&IgnoreEntry],
) -> Map<String, Value> {
    let mut members = Map::new();
    members.insert("command".to_owned(), json!(command));
    members.insert("file".to_owned(), json!(file));
    members.insert("entries".to_owned(), json!(entries));
    members.insert("added".to_owned(), entries_json(added));
    members.insert("removed".to_owned(), entries_json(removed));
    members
}

fn is_recorded(diagnostic: &Diagnostic) -> bool {
    matches!(diagnostic.severity, Severity::Error | Severity::Warning)
}

fn unrecorded_json((diagnostic, unrecorded): &(&Diagnostic, Unrecorded)) -> Value {
    json!({
        "rule": diagnostic.rule.as_str(),
        "package": diagnostic.package,
        "dependency": diagnostic.dependency,
        "message": diagnostic.message,
        "cause": unrecorded.cause(),
    })
}

fn write_baseline(args: &BaselineWriteArgs) -> Result<u8, CliError> {
    let (target, file) = locate(&args.common)?;
    let root = target.root.clone();
    let extended_already = target.sources.iter().any(|source| source.path == file);
    let current = read_baseline(&file)?;
    let existing = current
        .as_ref()
        .map(|baseline| baseline.entries.clone())
        .unwrap_or_default();
    let (workspace, diagnostics) = lint_without(target, &existing)?;
    let covered: BTreeSet<usize> = ignore_matches(&workspace, &existing, &diagnostics)
        .into_iter()
        .flatten()
        .collect();
    let mut added: Vec<IgnoreEntry> = Vec::new();
    let mut unrecorded: Vec<(&Diagnostic, Unrecorded)> = Vec::new();
    for (index, diagnostic) in diagnostics.iter().enumerate() {
        if covered.contains(&index) || !is_recorded(diagnostic) {
            continue;
        }
        match entry_for(&workspace, diagnostic, &args.reason)? {
            Ok(entry) => added.push(entry),
            Err(cause) => unrecorded.push((diagnostic, cause)),
        }
    }
    let added = sorted_unique(added);
    let entries = sorted_unique(existing.iter().cloned().chain(added.iter().cloned()).collect());
    let text = render(&entries);
    let mut written = write_if_changed(&file, &text, current.as_ref().map(|baseline| baseline.text.as_str()))?;
    // After the baseline file, so the configuration never extends a file that does not exist.
    let extended = if extended_already {
        None
    } else {
        Some(add_extends(&root, &file)?)
    };
    written |= extended.is_some();

    let shown = relative(&root, &file);
    let added: Vec<&IgnoreEntry> = added.iter().collect();
    let extended = extended.map(|config| relative(&root, &config));
    match args.common.format {
        RulesFormat::Json => {
            let mut members = members("write", &shown, entries.len(), &added, &[]);
            members.insert("extended".to_owned(), json!(extended));
            members.insert(
                "unrecorded".to_owned(),
                Value::Array(unrecorded.iter().map(unrecorded_json).collect()),
            );
            members.insert("written".to_owned(), json!(written));
            write_stdout(document(members)?.as_bytes())?;
        }
        RulesFormat::Human => {
            for (diagnostic, cause) in &unrecorded {
                stderr_line(&format!(
                    "warning: not recorded: {} ({}): {}",
                    diagnostic.rule.as_str(),
                    diagnostic.message,
                    cause.note()
                ));
            }
            let mut out = String::new();
            if added.is_empty() {
                let _ = writeln!(
                    out,
                    "{shown} is up to date ({})",
                    counted(entries.len(), "entry", "entries")
                );
            } else {
                let _ = writeln!(
                    out,
                    "Recorded {} in {shown} ({} in total)",
                    counted(added.len(), "new entry", "new entries"),
                    entries.len()
                );
                list_entries(&mut out, '+', &added);
            }
            if let Some(config) = &extended {
                let _ = writeln!(out, "Added {shown} to `extends` in {config}");
            }
            write_stdout(out.as_bytes())?;
        }
    }
    Ok(CLEAN)
}

fn prune_baseline(args: &BaselineFileArgs) -> Result<u8, CliError> {
    let (target, file) = locate(args)?;
    let root = target.root.clone();
    let Some(current) = read_baseline(&file)? else {
        return Err(CliError {
            path: Some(slash(&file)),
            ..CliError::new(
                Kind::Config,
                format!("{} does not exist; `skif baseline write` creates it", file.display()),
            )
        });
    };
    let (workspace, diagnostics) = lint_without(target, &current.entries)?;
    let matches = ignore_matches(&workspace, &current.entries, &diagnostics);
    let (kept, removed): (Matched<'_>, Matched<'_>) = current
        .entries
        .iter()
        .zip(&matches)
        .partition(|(_, found)| !found.is_empty());
    let kept = sorted_unique(kept.into_iter().map(|(entry, _)| entry.clone()).collect());
    let removed = sorted_unique(removed.into_iter().map(|(entry, _)| entry.clone()).collect());
    let written = write_if_changed(&file, &render(&kept), Some(current.text.as_str()))?;

    let shown = relative(&root, &file);
    let removed: Vec<&IgnoreEntry> = removed.iter().collect();
    match args.format {
        RulesFormat::Json => {
            let mut members = members("prune", &shown, kept.len(), &[], &removed);
            members.insert("written".to_owned(), json!(written));
            write_stdout(document(members)?.as_bytes())?;
        }
        RulesFormat::Human => {
            let mut out = String::new();
            if removed.is_empty() {
                let _ = writeln!(
                    out,
                    "Nothing to prune in {shown} ({})",
                    counted(kept.len(), "entry", "entries")
                );
            } else {
                let _ = writeln!(
                    out,
                    "Removed {} from {shown} ({} left)",
                    counted(removed.len(), "entry", "entries"),
                    kept.len()
                );
                list_entries(&mut out, '-', &removed);
            }
            write_stdout(out.as_bytes())?;
        }
    }
    Ok(CLEAN)
}

fn git(dir: &Path, args: &[&str]) -> Result<Output, CliError> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|error| CliError::new(Kind::Io, format!("cannot run git: {error}")))
}

/// The text of the baseline file `file` at the git revision `revision`, or `None` when the file did not exist there.
fn at_revision(file: &Path, revision: &str) -> Result<Option<String>, CliError> {
    let usage = |message: String| CliError::new(Kind::Usage, message);
    if revision.is_empty() || revision.starts_with('-') {
        return Err(usage(format!("--base: `{revision}` is not a git revision")));
    }
    let (Some(dir), Some(name)) = (file.parent(), file.file_name().and_then(|name| name.to_str())) else {
        return Err(usage(format!("{} does not name a file", file.display())));
    };
    let verified = git(
        dir,
        &["rev-parse", "--verify", "--quiet", &format!("{revision}^{{commit}}")],
    )?;
    if !verified.status.success() {
        let detail = String::from_utf8_lossy(&verified.stderr).trim().to_owned();
        let mut message = format!(
            "--base: `{revision}` is not a commit of the git repository at {}",
            dir.display()
        );
        if !detail.is_empty() {
            let _ = write!(message, " ({detail})");
        }
        return Err(usage(message));
    }
    let shown = git(dir, &["show", &format!("{revision}:./{name}")])?;
    if !shown.status.success() {
        return Ok(None);
    }
    String::from_utf8(shown.stdout)
        .map(Some)
        .map_err(|_| CliError::new(Kind::Config, format!("{revision}:{name} is not UTF-8")))
}

fn check_baseline(args: &BaselineCheckArgs) -> Result<u8, CliError> {
    let (target, file) = locate(&args.common)?;
    let current = sorted_unique(
        read_baseline(&file)?
            .map(|baseline| baseline.entries)
            .unwrap_or_default(),
    );
    let (base, text) = match (&args.base, &args.base_file) {
        (Some(revision), _) => (revision.clone(), at_revision(&file, revision)?),
        (None, Some(path)) => (
            slash(path),
            Some(fs::read_to_string(path).map_err(|error| CliError::io("cannot read", path, &error))?),
        ),
        (None, None) => return Err(CliError::new(Kind::Usage, "give --base <REF> or --base-file <FILE>")),
    };
    let old = match text {
        Some(text) => sorted_unique(
            parse_entries(&text).map_err(|message| CliError::new(Kind::Config, format!("{base}: {message}")))?,
        ),
        None => Vec::new(),
    };
    let old_keys: BTreeSet<Key> = old.iter().map(key).collect();
    let current_keys: BTreeSet<Key> = current.iter().map(key).collect();
    let added: Vec<&IgnoreEntry> = current.iter().filter(|entry| !old_keys.contains(&key(entry))).collect();
    let removed: Vec<&IgnoreEntry> = old.iter().filter(|entry| !current_keys.contains(&key(entry))).collect();

    let shown = relative(&target.root, &file);
    match args.common.format {
        RulesFormat::Json => {
            let mut members = members("check", &shown, current.len(), &added, &removed);
            members.insert("base".to_owned(), json!(base));
            write_stdout(document(members)?.as_bytes())?;
        }
        RulesFormat::Human => {
            let mut out = String::new();
            if added.is_empty() {
                let _ = write!(out, "{shown} gained no entries compared with {base}");
                if !removed.is_empty() {
                    let _ = write!(out, " ({} fewer)", removed.len());
                }
                out.push('\n');
            } else {
                let _ = writeln!(
                    out,
                    "{shown} gained {} compared with {base}",
                    counted(added.len(), "entry", "entries")
                );
                list_entries(&mut out, '+', &added);
            }
            write_stdout(out.as_bytes())?;
        }
    }
    Ok(if added.is_empty() { CLEAN } else { GREW })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(rules: &[&str], packages: &[&str], dependencies: &[&str], reason: Option<&str>) -> IgnoreEntry {
        let mut entry = IgnoreEntry::default()
            .with_rules(rules.iter().copied())
            .with_packages(Selector::new(packages.iter().copied()).unwrap())
            .with_dependencies(Selector::new(dependencies.iter().copied()).unwrap());
        if let Some(reason) = reason {
            entry = entry.with_reason(reason);
        }
        entry
    }

    #[test]
    fn glob_syntax_is_written_as_literal_classes() {
        assert_eq!(literal("react@^18.0.0"), "react@^18.0.0");
        assert_eq!(literal("x@*"), "x@[*]");
        assert_eq!(literal("a?{b}[c]"), "a[?][{]b[}][[]c[]]");
        let selector = Selector::new([literal("x@*")]).unwrap();
        assert!(selector.matches("x@*", false));
        assert!(!selector.matches("x@1.0.0", false));
    }

    #[test]
    fn a_leading_exclamation_mark_is_not_a_negation() {
        assert_eq!(literal("!legacy"), "{!}legacy");
        assert_eq!(literal("a!b"), "a!b");
        let selector = Selector::new([literal("!legacy")]).unwrap();
        assert!(selector.matches("!legacy", false));
        assert!(!selector.matches("legacy", false));
        assert!(
            !selector.matches("other", false),
            "not an exclusion, which would select everything else"
        );
        let both = Selector::new([literal("!{x}")]).unwrap();
        assert!(both.matches("!{x}", false));
        assert!(!both.matches("!x", false));
    }

    #[test]
    fn entries_compare_by_their_patterns_in_any_order() {
        let written = entry(&["b-rule", "a-rule"], &["y", "x"], &["d"], Some("edited"));
        let reordered = entry(&["a-rule", "b-rule", "a-rule"], &["x", "y"], &["d"], None);
        assert_eq!(key(&written), key(&reordered));
        assert_ne!(key(&written), key(&entry(&["a-rule", "b-rule"], &["x"], &["d"], None)));
        assert_eq!(sorted_unique(vec![written.clone(), reordered]), [written]);
    }

    #[test]
    fn entries_render_sorted_and_parse_back() {
        let entries = sorted_unique(vec![
            entry(&["version-mismatch"], &["b"], &["react@^17.0.0"], Some("baseline")),
            entry(&["version-mismatch"], &["a"], &["react@^17.0.0"], Some("kept")),
            entry(&["banned-dependency"], &[], &["left-pad"], None),
            entry(&["version-mismatch"], &["a"], &["react@^17.0.0"], Some("dropped")),
        ]);
        let text = render(&entries);
        assert_eq!(
            text,
            format!(
                "{HEADER}\n[[ignore]]\nrules = [\"banned-dependency\"]\ndependencies = [\"left-pad\"]\n\
                 \n[[ignore]]\nrules = [\"version-mismatch\"]\npackages = [\"a\"]\ndependencies = [\"react@^17.0.0\"]\nreason = \"kept\"\n\
                 \n[[ignore]]\nrules = [\"version-mismatch\"]\npackages = [\"b\"]\ndependencies = [\"react@^17.0.0\"]\nreason = \"baseline\"\n"
            )
        );
        assert_eq!(parse_entries(&text).unwrap(), entries);
        assert_eq!(parse_entries(&render(&[])).unwrap(), Vec::<IgnoreEntry>::new());
    }

    #[test]
    fn a_baseline_file_holds_only_ignore_entries() {
        let error = parse_entries("[lint]\nfail-on-warnings = true\n").unwrap_err();
        assert!(error.contains("`lint`"), "{error}");
        assert!(parse_entries("[[ignore]]\nrules = [\"x\"]\nbogus = 1\n").is_err());
    }

    #[test]
    fn entries_describe_their_rule_and_scope() {
        assert_eq!(
            describe(&entry(&["version-mismatch"], &["@acme/web"], &["react@^17.0.2"], None)),
            "version-mismatch: package @acme/web, dependency react@^17.0.2"
        );
        assert_eq!(describe(&entry(&[], &["a"], &[], None)), "every rule: package a");
    }

    #[test]
    fn extends_is_added_at_the_top_after_a_schema_line() {
        let cases = [
            ("", "extends = [\"b.toml\"]\n"),
            (
                "[js]\nenabled = false\n",
                "extends = [\"b.toml\"]\n\n[js]\nenabled = false\n",
            ),
            (
                "#:schema ./s.json\n\n# comment\n[lint]\n",
                "#:schema ./s.json\nextends = [\"b.toml\"]\n\n# comment\n[lint]\n",
            ),
            (
                "#:schema ./s.json\n[lint]\n",
                "#:schema ./s.json\nextends = [\"b.toml\"]\n\n[lint]\n",
            ),
            ("root = true\r\n", "extends = [\"b.toml\"]\r\n\r\nroot = true\r\n"),
        ];
        for (text, expected) in cases {
            assert_eq!(with_extends(text, "b.toml").unwrap(), expected, "{text:?}");
        }
    }

    #[test]
    fn an_existing_extends_array_is_appended_to() {
        assert_eq!(
            with_extends("# shared\nextends = [\"a.toml\"] # first\n[js]\n", "b.toml").unwrap(),
            "# shared\nextends = [\"a.toml\", \"b.toml\"] # first\n[js]\n"
        );
        assert!(with_extends("extends = \"a.toml\"\n", "b.toml").is_err());
        assert!(with_extends("[[ignore\n", "b.toml").is_err());
    }
}
