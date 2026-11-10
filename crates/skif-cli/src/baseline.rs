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
    CONFIG_FILE_NAMES, CachedFs, Config, Diagnostic, IgnoreEntry, OsFs, Selector, Severity, Target, Workspace,
    dependency_name, ignore_matches, slash,
};
use skif_report::escape_controls;

use crate::args::{
    BaselineArgs, BaselineCheckArgs, BaselineCommand, BaselineFileArgs, BaselinePruneArgs, BaselineWriteArgs, Format,
    LintArgs, RulesFormat,
};
use crate::config::{document, relative, with_errors};
use crate::error::{CliError, Kind};
use crate::filter::Filter;
use crate::lint::{self, canonical};
use crate::output::{emit_error, stderr_line, write_stdout};
use crate::roots::{self, Linters, Resolved, RootError};
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
        BaselineCommand::Prune(prune) => (&prune.common, prune_baseline(prune)),
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

/// A root a command covers and the absolute path of its baseline file.
struct Root {
    target: Target,
    file: PathBuf,
}

/// The roots a command covers.
struct Roots {
    /// The workspace root of a single-root run, the invocation directory of a multi-root run; the files the human
    /// output names are relative to it.
    base: PathBuf,
    /// Whether the run covers every root below `PATH`: the JSON document then lists one member per root.
    multi: bool,
    all: Vec<Root>,
    /// Roots a multi-root run found but could not load.
    failures: Vec<RootError>,
}

/// The roots at `PATH`, resolved as `lint` resolves them, each with its baseline file: `--file`, which names the
/// baseline of a single root, or `skif.baseline.toml` in the root.
fn locate(args: &BaselineFileArgs, lint: &LintArgs) -> Result<Roots, CliError> {
    let start = canonical(&args.path)?;
    let Resolved {
        base,
        targets,
        multi,
        failures,
        truncated: _,
    } = roots::resolve(&start, lint.recursive, &CachedFs::new(OsFs), &lint::overrides(lint))?;
    let file = args.file.as_deref().map(resolve_file).transpose()?;
    if file.is_some() && targets.len() > 1 {
        return Err(CliError::new(
            Kind::Usage,
            format!(
                "--file names one baseline file, but {} holds {} roots; leave it out to use {DEFAULT_FILE} in each root",
                args.path.display(),
                targets.len()
            ),
        ));
    }
    let roots = targets
        .into_iter()
        .map(|target| Root {
            file: file.clone().unwrap_or_else(|| target.root.join(DEFAULT_FILE)),
            target,
        })
        .collect();
    Ok(Roots {
        base,
        multi,
        all: roots,
        failures,
    })
}

/// Fails when the configuration of `target` does not apply the baseline file `file` holding `entries`: no
/// configuration file it reads extends the file (`--config` names one that does not), or a file applied after the
/// baseline resets `ignore` and so drops its entries. A baseline that does not apply would leave the lint run failing
/// on every finding it records, while `write` and `prune` report success.
fn check_applied(target: &Target, file: &Path, entries: &[IgnoreEntry]) -> Result<(), CliError> {
    if entries.is_empty() {
        return Ok(());
    }
    let cause = if target.sources.iter().all(|source| source.path.as_path() != file) {
        "no configuration file of the run extends it (a file given with --config must extend it itself)"
    } else {
        let applied: BTreeSet<(Key, Option<&str>)> = target
            .config
            .ignore
            .iter()
            .map(|entry| (key(entry), entry.reason.as_deref()))
            .collect();
        if entries
            .iter()
            .all(|entry| applied.contains(&(key(entry), entry.reason.as_deref())))
        {
            return Ok(());
        }
        "a configuration file applied after it resets `ignore` (`reset = [\"ignore\"]`), which drops its entries; \
         put the reset in a file listed before the baseline in `extends`, or remove it"
    };
    Err(config_error(
        file,
        &format!("the baseline does not apply to the lint run: {cause}"),
    ))
}

/// Whether the assignment that decided `key` of `target`'s configuration comes from `file`.
fn is_from(target: &Target, key: &str, file: &Path) -> bool {
    target
        .origins
        .last(key)
        .and_then(|assignment| target.sources.get(assignment.source))
        .is_some_and(|source| source.path.as_path() == file)
}

/// `file` made absolute, with its directory spelled as the OS resolves it when it exists, so it compares equal to the
/// paths configuration discovery records.
fn resolve_file(file: &Path) -> Result<PathBuf, CliError> {
    // Checked before `absolute`, which resolves `..` on Windows and would turn `--file ..` into a directory name.
    let absolute = match file.file_name() {
        Some(_) => std::path::absolute(file).map_err(|error| CliError::io("cannot resolve", file, &error))?,
        None => PathBuf::new(),
    };
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

/// What the lint run of one root found: its workspace and its findings.
struct Linted {
    workspace: Workspace,
    diagnostics: Vec<Diagnostic>,
}

/// What the lint run of every root found, in the order of the roots, and the IDs of the rules whose findings are about
/// a specifier ([`skif::RuleMeta::version_scoped`]).
struct Lints {
    roots: Vec<Linted>,
    version_scoped: BTreeSet<&'static str>,
}

/// Lints `targets` together, as `lint` lints them (its flags applied, roots of a multi-root run crediting each
/// other's uses), each with its configuration minus the entries its baseline file in `files` contributes. An equal
/// entry of another configuration file keeps applying.
fn lint_without(mut targets: Vec<Target>, files: &[&Path], lint: &LintArgs, base: &Path) -> Result<Lints, CliError> {
    let filter = Filter::new(lint)?;
    for (target, file) in targets.iter_mut().zip(files) {
        let keep: Vec<bool> = (0..target.config.ignore.len())
            .map(|index| !is_from(target, &format!("ignore[{index}]"), file))
            .collect();
        target.retain_ignore(|index| keep[index]);
        lint::adjust(lint, &mut target.config)?;
        filter.configure(&mut target.config)?;
    }
    lint::apply_tsconfig(lint, &mut targets)?;
    let linters = Linters::build(&targets)?;
    let version_scoped = linters
        .rules()
        .into_iter()
        .filter(|meta| meta.version_scoped)
        .map(|meta| meta.id)
        .collect();
    let mut runs = roots::lint_targets(base, targets, &linters)?;
    filter.apply_runs(&mut runs)?;
    Ok(Lints {
        roots: runs
            .into_iter()
            .map(|run| Linted {
                workspace: run.workspace,
                diagnostics: run.report.diagnostics,
            })
            .collect(),
        version_scoped,
    })
}

/// What an entry is compared by: its rules, packages, dependencies and files, each sorted and without repeats, since
/// neither changes what the entry matches; not its reason.
type Key = [Vec<String>; 4];

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
        sorted(entry.files.patterns()),
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
    /// Neither a package, a dependency nor a file: its entry would turn the rule off for the whole workspace.
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
                "an `[[ignore]]` entry without a package, dependency or file would turn the rule off for the whole \
                 workspace"
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

/// The dependency pattern of a finding: `name@specifier` when its rule is about the specifier (`version_scoped`) and
/// the finding carries it as written, so only that version is silenced; else the dependency's name, so a version bump
/// does not reopen a finding about something else.
fn dependency_pattern(diagnostic: &Diagnostic, version_scoped: bool) -> Option<String> {
    let dependency = diagnostic.dependency.as_deref()?;
    Some(match diagnostic.specifier.as_deref().filter(|_| version_scoped) {
        Some(specifier) => format!("{}@{}", literal(dependency_name(dependency)), literal(specifier)),
        None => literal(dependency_name(dependency)),
    })
}

fn selector(pattern: Option<String>) -> Result<Selector, CliError> {
    Ok(match pattern {
        Some(pattern) => Selector::new([pattern])?,
        None => Selector::default(),
    })
}

/// The file pattern of a finding about no dependency that is located in a file other than a manifest (an unused file
/// or export, an import cycle): its path relative to the root, so the entry covers that file and not every file of
/// the package. A finding about a dependency, or one located in a `package.json`, has none: its package and dependency
/// narrow it.
fn file_pattern(diagnostic: &Diagnostic) -> Option<String> {
    if diagnostic.dependency.is_some() {
        return None;
    }
    let path = &diagnostic.location.path;
    let name = path.file_name()?;
    (name != "package.json").then(|| literal(&slash(path)))
}

/// The entry that records `diagnostic`, or why there is none: a finding with neither a package, a dependency nor a
/// file would need an entry of its rule alone, which turns the rule off for the whole workspace, and one in an unnamed
/// root package that names no file an entry no package pattern narrows to it.
fn entry_for(
    workspace: &Workspace,
    version_scoped: &BTreeSet<&'static str>,
    diagnostic: &Diagnostic,
    reason: &str,
) -> Result<Result<IgnoreEntry, Unrecorded>, CliError> {
    let file = file_pattern(diagnostic);
    let package = match package_pattern(workspace, diagnostic) {
        Ok(package) => package,
        // The file alone narrows the entry to findings of the root package's files.
        Err(Unrecorded::UnnamedRoot) if file.is_some() => None,
        Err(unrecorded) => return Ok(Err(unrecorded)),
    };
    let dependency = dependency_pattern(diagnostic, version_scoped.contains(diagnostic.rule.as_str()));
    if package.is_none() && dependency.is_none() && file.is_none() {
        return Ok(Err(Unrecorded::WorkspaceWide));
    }
    Ok(Ok(IgnoreEntry::default()
        .with_rules([diagnostic.rule.as_str().to_owned()])
        .with_packages(selector(package)?)
        .with_dependencies(selector(dependency)?)
        .with_files(selector(file)?)
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
        render_list(&mut out, "files", entry.files.patterns());
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
    if !entry.files.is_empty() {
        scope.push(format!("file {}", entry.files.patterns().join(", ")));
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

/// The members every `skif baseline` JSON document has for a root.
fn members(file: &str, entries: usize, added: &[&IgnoreEntry], removed: &[&IgnoreEntry]) -> Map<String, Value> {
    let mut members = Map::new();
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

/// What a command did in one root: the root, its JSON members, its human output and its exit code.
struct Outcome {
    root: PathBuf,
    members: Map<String, Value>,
    human: String,
    code: u8,
}

/// Prints what `command` did in every root of `roots` and returns the exit code: one JSON document (a multi-root run
/// lists one member per root under `roots`) or the human output of each root; `2` when a root could not be loaded.
fn emit(command: &str, roots: &Roots, outcomes: Vec<Outcome>, format: RulesFormat) -> Result<u8, CliError> {
    let code = if roots.failures.is_empty() {
        outcomes.iter().map(|outcome| outcome.code).max().unwrap_or(CLEAN)
    } else {
        COULD_NOT_RUN
    };
    match format {
        RulesFormat::Json => {
            let mut members = Map::new();
            members.insert("command".to_owned(), json!(command));
            if roots.multi {
                let listed = outcomes.into_iter().map(|mut outcome| {
                    let mut member = Map::new();
                    member.insert("root".to_owned(), json!(relative(&roots.base, &outcome.root)));
                    member.append(&mut outcome.members);
                    Value::Object(member)
                });
                members.insert("roots".to_owned(), Value::Array(listed.collect()));
                with_errors(&mut members, &roots.failures);
            } else if let Some(mut outcome) = outcomes.into_iter().next() {
                members.append(&mut outcome.members);
            }
            write_stdout(document(members)?.as_bytes())?;
        }
        RulesFormat::Human => {
            let out: String = outcomes.into_iter().map(|outcome| outcome.human).collect();
            write_stdout(out.as_bytes())?;
            for failure in &roots.failures {
                stderr_line(&format!("error: {}", escape_controls(&failure.describe())));
            }
        }
    }
    Ok(code)
}

/// A root of a `write` or `prune` run once its target went to the lint run: its directory, its baseline file, whether
/// its configuration already extends that file, and what the file holds.
struct Place {
    root: PathBuf,
    file: PathBuf,
    extended: bool,
    baseline: Option<Baseline>,
}

impl Place {
    /// The entries of the baseline file, none for a file that does not exist.
    fn entries(&self) -> &[IgnoreEntry] {
        self.baseline
            .as_ref()
            .map_or(&[][..], |baseline| baseline.entries.as_slice())
    }
}

/// Reads the baseline file of every root and splits the roots into their places and their targets.
fn read_places(roots: &mut Roots) -> Result<(Vec<Place>, Vec<Target>), CliError> {
    let mut places = Vec::with_capacity(roots.all.len());
    let mut targets = Vec::with_capacity(roots.all.len());
    for Root { target, file } in std::mem::take(&mut roots.all) {
        let baseline = read_baseline(&file)?;
        places.push(Place {
            root: target.root.clone(),
            extended: target.sources.iter().any(|source| source.path == file),
            file,
            baseline,
        });
        targets.push(target);
    }
    Ok((places, targets))
}

fn write_baseline(args: &BaselineWriteArgs) -> Result<u8, CliError> {
    let lint = args.lint.lint_args(&args.common.path, args.common.recursive);
    let mut roots = locate(&args.common, &lint)?;
    let (places, targets) = read_places(&mut roots)?;
    let files: Vec<&Path> = places.iter().map(|place| place.file.as_path()).collect();
    let lints = lint_without(targets, &files, &lint, &roots.base)?;
    let mut outcomes = Vec::with_capacity(places.len());
    for (place, linted) in places.iter().zip(&lints.roots) {
        outcomes.push(write_root(
            place,
            linted,
            &lints.version_scoped,
            (args, &lint),
            &roots.base,
        )?);
    }
    emit("write", &roots, outcomes, args.common.format)
}

/// Records the findings of one root that no entry of its baseline matches, makes its configuration extend the
/// baseline file, and checks that the configuration applies it.
fn write_root(
    place: &Place,
    linted: &Linted,
    version_scoped: &BTreeSet<&'static str>,
    (args, lint): (&BaselineWriteArgs, &LintArgs),
    base: &Path,
) -> Result<Outcome, CliError> {
    let existing = place.entries();
    let covered: BTreeSet<usize> = ignore_matches(&linted.workspace, existing, &linted.diagnostics)
        .into_iter()
        .flatten()
        .collect();
    let mut added: Vec<IgnoreEntry> = Vec::new();
    let mut unrecorded: Vec<(&Diagnostic, Unrecorded)> = Vec::new();
    for (index, diagnostic) in linted.diagnostics.iter().enumerate() {
        if covered.contains(&index) || !is_recorded(diagnostic) {
            continue;
        }
        match entry_for(&linted.workspace, version_scoped, diagnostic, &args.reason)? {
            Ok(entry) => added.push(entry),
            Err(cause) => unrecorded.push((diagnostic, cause)),
        }
    }
    let added = sorted_unique(added);
    let entries = sorted_unique(existing.iter().cloned().chain(added.iter().cloned()).collect());
    let text = render(&entries);
    let current = place.baseline.as_ref().map(|baseline| baseline.text.as_str());
    let mut written = write_if_changed(&place.file, &text, current)?;
    // After the baseline file, so the configuration never extends a file that does not exist.
    let extended = if place.extended {
        None
    } else {
        Some(add_extends(&place.root, &place.file)?)
    };
    written |= extended.is_some();
    // Resolved again, so a `reset` of the file that now extends the baseline is seen.
    if let Ok(target) = skif::find_target(&place.root, &CachedFs::new(OsFs), &lint::overrides(lint))
        && target.root == place.root
    {
        check_applied(&target, &place.file, &entries)?;
    }

    let added: Vec<&IgnoreEntry> = added.iter().collect();
    let extended = extended.map(|config| relative(&place.root, &config));
    let mut members = members(&relative(&place.root, &place.file), entries.len(), &added, &[]);
    members.insert("extended".to_owned(), json!(extended));
    members.insert(
        "unrecorded".to_owned(),
        Value::Array(unrecorded.iter().map(unrecorded_json).collect()),
    );
    members.insert("written".to_owned(), json!(written));

    let shown = relative(base, &place.file);
    let mut human = String::new();
    if args.common.format == RulesFormat::Human {
        for (diagnostic, cause) in &unrecorded {
            stderr_line(&format!(
                "warning: not recorded: {} ({}): {}",
                diagnostic.rule.as_str(),
                diagnostic.message,
                cause.note()
            ));
        }
        if added.is_empty() {
            let _ = writeln!(
                human,
                "{shown} is up to date ({})",
                counted(entries.len(), "entry", "entries")
            );
        } else {
            let _ = writeln!(
                human,
                "Recorded {} in {shown} ({} in total)",
                counted(added.len(), "new entry", "new entries"),
                entries.len()
            );
            list_entries(&mut human, '+', &added);
        }
        if let Some(config) = &extended {
            let _ = writeln!(
                human,
                "Added {} to `extends` in {config}",
                relative(&place.root, &place.file)
            );
        }
    }
    Ok(Outcome {
        root: place.root.clone(),
        members,
        human,
        code: CLEAN,
    })
}

fn prune_baseline(args: &BaselinePruneArgs) -> Result<u8, CliError> {
    let lint = args.lint.lint_args(&args.common.path, args.common.recursive);
    let mut roots = locate(&args.common, &lint)?;
    let (places, targets) = read_places(&mut roots)?;
    for (place, target) in places.iter().zip(&targets) {
        let Some(baseline) = &place.baseline else {
            return Err(CliError {
                path: Some(slash(&place.file)),
                ..CliError::new(
                    Kind::Config,
                    format!(
                        "{} does not exist; `skif baseline write` creates it",
                        place.file.display()
                    ),
                )
            });
        };
        check_applied(target, &place.file, &baseline.entries)?;
    }
    let files: Vec<&Path> = places.iter().map(|place| place.file.as_path()).collect();
    let lints = lint_without(targets, &files, &lint, &roots.base)?;
    let mut outcomes = Vec::with_capacity(places.len());
    for (place, linted) in places.iter().zip(&lints.roots) {
        outcomes.push(prune_root(place, linted, args.common.format, &roots.base)?);
    }
    emit("prune", &roots, outcomes, args.common.format)
}

/// Removes the entries of one root's baseline that match no finding of its lint run.
fn prune_root(place: &Place, linted: &Linted, format: RulesFormat, base: &Path) -> Result<Outcome, CliError> {
    let entries = place.entries();
    let text = place.baseline.as_ref().map(|baseline| baseline.text.as_str());
    let matches = ignore_matches(&linted.workspace, entries, &linted.diagnostics);
    let (kept, removed): (Matched<'_>, Matched<'_>) =
        entries.iter().zip(&matches).partition(|(_, found)| !found.is_empty());
    let kept = sorted_unique(kept.into_iter().map(|(entry, _)| entry.clone()).collect());
    let removed = sorted_unique(removed.into_iter().map(|(entry, _)| entry.clone()).collect());
    let written = write_if_changed(&place.file, &render(&kept), text)?;

    let removed: Vec<&IgnoreEntry> = removed.iter().collect();
    let mut members = members(&relative(&place.root, &place.file), kept.len(), &[], &removed);
    members.insert("written".to_owned(), json!(written));
    let shown = relative(base, &place.file);
    let mut human = String::new();
    if format == RulesFormat::Human {
        if removed.is_empty() {
            let _ = writeln!(
                human,
                "Nothing to prune in {shown} ({})",
                counted(kept.len(), "entry", "entries")
            );
        } else {
            let _ = writeln!(
                human,
                "Removed {} from {shown} ({} left)",
                counted(removed.len(), "entry", "entries"),
                kept.len()
            );
            list_entries(&mut human, '-', &removed);
        }
    }
    Ok(Outcome {
        root: place.root.clone(),
        members,
        human,
        code: CLEAN,
    })
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
    // `git -C` fails on a missing directory, which would read as a bad revision.
    if !dir.is_dir() {
        return Err(usage(format!("--file: the directory {} does not exist", dir.display())));
    }
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
    let lint = LintArgs {
        path: args.common.path.clone(),
        recursive: args.common.recursive,
        ..LintArgs::default()
    };
    let mut roots = locate(&args.common, &lint)?;
    if args.base.is_none() && roots.all.len() > 1 {
        return Err(CliError::new(
            Kind::Usage,
            format!(
                "--base-file names one baseline file, but {} holds {} roots; give --base <REF>",
                args.common.path.display(),
                roots.all.len()
            ),
        ));
    }
    let mut outcomes = Vec::with_capacity(roots.all.len());
    for root in std::mem::take(&mut roots.all) {
        outcomes.push(check_root(&root.target.root, &root.file, args, &roots.base)?);
    }
    emit("check", &roots, outcomes, args.common.format)
}

/// Compares one root's baseline file with its earlier version.
fn check_root(root: &Path, file: &Path, args: &BaselineCheckArgs, shown_from: &Path) -> Result<Outcome, CliError> {
    let current = sorted_unique(
        read_baseline(file)?
            .map(|baseline| baseline.entries)
            .unwrap_or_default(),
    );
    let (base, text) = match (&args.base, &args.base_file) {
        (Some(revision), _) => (revision.clone(), at_revision(file, revision)?),
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

    let mut members = members(&relative(root, file), current.len(), &added, &removed);
    members.insert("base".to_owned(), json!(base));
    let shown = relative(shown_from, file);
    let mut human = String::new();
    if args.common.format == RulesFormat::Human {
        if added.is_empty() {
            let _ = write!(human, "{shown} gained no entries compared with {base}");
            if !removed.is_empty() {
                let _ = write!(human, " ({} fewer)", removed.len());
            }
            human.push('\n');
        } else {
            let _ = writeln!(
                human,
                "{shown} gained {} compared with {base}",
                counted(added.len(), "entry", "entries")
            );
            list_entries(&mut human, '+', &added);
        }
    }
    Ok(Outcome {
        root: root.to_path_buf(),
        members,
        human,
        code: if added.is_empty() { CLEAN } else { GREW },
    })
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
        let file = entry(&["unused-files"], &["a"], &[], None).with_files(Selector::new(["src/old.ts"]).unwrap());
        assert_eq!(describe(&file), "unused-files: package a, file src/old.ts");
    }

    #[test]
    fn entries_with_files_render_them_and_compare_by_them() {
        let file = |path: &str| entry(&["unused-files"], &["a"], &[], None).with_files(Selector::new([path]).unwrap());
        let text = render(&[file("src/old.ts")]);
        assert!(
            text.ends_with("[[ignore]]\nrules = [\"unused-files\"]\npackages = [\"a\"]\nfiles = [\"src/old.ts\"]\n"),
            "{text}"
        );
        assert_eq!(parse_entries(&text).unwrap(), [file("src/old.ts")]);
        assert_ne!(key(&file("src/old.ts")), key(&file("src/new.ts")));
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
