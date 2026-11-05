//! knip: `knip.json(c)`, `knip.{js,ts,...}`, the `knip` key of `package.json`, and the flags of the scripts that run
//! it.

use skif_js_plugins::registry;
use skif_js_syntax::ConfigValue;

use crate::doc::{Doc, Section, Val};
use crate::keys;
use crate::notes::{NoteClass, Tool};
use crate::scripts::{self, Invocation};
use crate::state::{Level, Src, State};
use crate::tables::{self, KnipType};
use crate::value::{boolean, entries, join, pattern_list, read, string, string_list};
use crate::{MigrateError, ToolInput};

/// Rules that are not issue types but belong to a knip run: a file that cannot be parsed ends it.
const ALWAYS: &[&str] = &["unparseable-files"];

/// What `--dependencies` and its siblings include.
const SHORTCUTS: &[(&str, &[&str])] = &[
    (
        "dependencies",
        &[
            "dependencies",
            "devDependencies",
            "optionalPeerDependencies",
            "unlisted",
            "binaries",
            "unresolved",
        ],
    ),
    (
        "exports",
        &[
            "exports",
            "nsExports",
            "types",
            "nsTypes",
            "enumMembers",
            "namespaceMembers",
            "duplicates",
        ],
    ),
    ("files", &["files"]),
    ("cycles", &["cycles"]),
];

/// Flags that change nothing about what is reported.
const INERT_FLAGS: &[&str] = &[
    "--help",
    "--version",
    "--config",
    "-c",
    "--reporter",
    "--reporter-options",
    "--directory",
    "-D",
    "--tsConfig",
    "-t",
    "--cache",
    "--cache-location",
    "--debug",
    "--no-progress",
    "-n",
    "--watch",
    "-w",
    "--performance",
    "--memory",
    "--memory-realtime",
    "--trace",
    "--trace-file",
    "--trace-export",
    "--trace-dependency",
    "--output-file",
    "--isolate-workspaces",
    "--use-tsconfig-files",
    "--include-libs",
    "--format",
    "-F",
    "--fix-type",
    "--allow-remove-files",
];

/// The flags the translation handles itself.
const HANDLED_FLAGS: &[&str] = &[
    "--dependencies",
    "--exports",
    "--files",
    "--cycles",
    "--include",
    "--exclude",
    "--production",
    "-p",
    "--strict",
    "-s",
    "--include-entry-exports",
    "--treat-config-hints-as-errors",
    "--treat-tag-hints-as-errors",
    "--no-config-hints",
    "--workspace",
    "-W",
    "--tags",
    "--max-issues",
    "--max-show-issues",
    "--no-exit-code",
    "--fix",
    "--no-gitignore",
    "--preprocessor",
    "--preprocessor-options",
];

/// Keys of the knip configuration that are not settings of one workspace, handled where they appear.
const ROOT_ONLY: &[&str] = &[
    "workspaces",
    "ignoreWorkspaces",
    "include",
    "exclude",
    "rules",
    "tags",
    "treatConfigHintsAsErrors",
    "treatTagHintsAsErrors",
    "cycles",
    "$schema",
];

/// Where a key is written: the root workspace or a workspace directory.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    Root,
    Dir(String),
}

impl Target {
    /// The table of the workspace's settings.
    fn js(&self) -> Vec<String> {
        match self {
            Self::Root => vec!["js".to_owned()],
            Self::Dir(dir) => vec!["js".to_owned(), "workspaces".to_owned(), dir.clone()],
        }
    }

    /// The table of the keys that exist per workspace only; the root is `"."`.
    fn workspace(&self) -> Vec<String> {
        match self {
            Self::Root => vec![
                "js".to_owned(),
                "workspaces".to_owned(),
                keys::ROOT_WORKSPACE.to_owned(),
            ],
            Self::Dir(_) => self.js(),
        }
    }

    fn plugins(&self) -> Vec<String> {
        let mut path = match self {
            Self::Root => vec!["js".to_owned()],
            Self::Dir(_) => self.js(),
        };
        path.push(keys::WS_PLUGINS.to_owned());
        path
    }

    /// How a note names the table: `[js]`, `[js.workspaces."apps/web"]`.
    fn describe(path: &[String]) -> String {
        let parts: Vec<String> = path
            .iter()
            .map(|part| {
                if part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                    part.clone()
                } else {
                    format!("\"{part}\"")
                }
            })
            .collect();
        format!("[{}]", parts.join("."))
    }

    /// `file` as the root of the project sees it.
    fn file_glob(&self, file: &str) -> String {
        match self {
            Self::Root => file.to_owned(),
            Self::Dir(dir) => format!("{}/{}", dir.trim_end_matches('/'), file.trim_start_matches("./")),
        }
    }
}

fn table<'d>(doc: &'d mut Doc, path: &[String]) -> &'d mut Section {
    let parts: Vec<&str> = path.iter().map(String::as_str).collect();
    doc.table(&parts)
}

/// What the configuration and the flags say about the issue types.
#[derive(Default)]
struct Plan {
    include: Vec<String>,
    exclude: Vec<String>,
    rules: Vec<(String, Level)>,
    tags: Vec<String>,
    hints: Option<Level>,
    production: bool,
    include_entry_exports: bool,
    /// Where the issue type settings came from, for the note.
    origins: Vec<String>,
}

impl Plan {
    fn origin(&mut self, origin: String) {
        if !self.origins.contains(&origin) {
            self.origins.push(origin);
        }
    }
}

pub(crate) fn translate(input: &ToolInput, state: &mut State<'_>) -> Result<(), MigrateError> {
    state.keep(ALWAYS.iter().copied());
    let package = input
        .package_json
        .as_ref()
        .map(|file| read(file, Tool::Knip))
        .transpose()?;
    let package_name = input
        .package_json
        .as_ref()
        .map_or("package.json", |file| file.path.as_str());
    let mut plan = Plan::default();
    let mut src = Src::new(Tool::Knip, package_name);
    let found = match (&input.config, &package) {
        (Some(file), _) => Some((read(file, Tool::Knip)?, Src::new(Tool::Knip, file.path.clone()), "")),
        (None, Some(package)) => {
            scripts::config_key(Tool::Knip, package).map(|config| (config.clone(), src.clone(), "knip"))
        }
        (None, None) => None,
    };
    if let Some((config, config_src, root)) = &found {
        src = config_src.clone();
        read_root(state, config_src, root, config, &mut plan);
    }
    if let Some(package) = &package {
        for invocation in scripts::invocations(package, Tool::Knip) {
            read_flags(state, &src, &invocation, &mut plan);
        }
    }
    apply(state, &src, &plan);
    Ok(())
}

fn level(state: &mut State<'_>, src: &Src, path: &str, text: &str) -> Option<Level> {
    match text {
        "error" => Some(Level::Error),
        "warn" => Some(Level::Warn),
        "off" => Some(Level::Off),
        other => {
            state.note(
                src,
                NoteClass::Human,
                path,
                format!("`{other}` is not `error`, `warn` or `off`"),
            );
            None
        }
    }
}

fn read_root(state: &mut State<'_>, src: &Src, root: &str, config: &ConfigValue, plan: &mut Plan) {
    let target = Target::Root;
    table(&mut state.doc, &target.js());
    for (key, item) in entries(state, src, root, config) {
        let path = join(root, key);
        match key {
            "$schema" => {}
            "workspaces" => workspaces(state, src, &path, item),
            "ignoreWorkspaces" => {
                let list = string_list(state, src, &path, item);
                write_list(state, src, &target.js(), keys::JS_IGNORE_WORKSPACES, &path, list, "");
            }
            "include" | "exclude" => {
                let list = string_list(state, src, &path, item);
                plan.origin(path);
                if key == "include" {
                    plan.include.extend(list);
                } else {
                    plan.exclude.extend(list);
                }
            }
            "rules" => rules(state, src, &path, item, plan),
            "tags" => {
                let list = string_list(state, src, &path, item);
                tags(state, src, &path, &list, plan);
            }
            "treatConfigHintsAsErrors" => {
                if boolean(state, src, &path, item) == Some(true) {
                    plan.hints = Some(Level::Error);
                    plan.origin(path);
                }
            }
            "treatTagHintsAsErrors" => {
                if boolean(state, src, &path, item) == Some(true) {
                    state.note(
                        src,
                        NoteClass::Unsupported,
                        path,
                        "skif has no tag hints to turn into errors",
                    );
                }
            }
            "cycles" => state.note(
                src,
                NoteClass::Unsupported,
                path,
                "`circular-imports` has no `allow` or `dynamicImports` options; it is written at the level of the \
                 `cycles` issue type",
            ),
            _ => workspace_key(state, src, &target, key, &path, item),
        }
    }
}

fn workspaces(state: &mut State<'_>, src: &Src, path: &str, value: &ConfigValue) {
    for (dir, config) in entries(state, src, path, value) {
        let dir_path = format!("{path}[\"{dir}\"]");
        let normalized = dir.trim_start_matches("./").trim_end_matches('/');
        let target = if normalized.is_empty() || normalized == "." {
            Target::Root
        } else {
            Target::Dir(normalized.to_owned())
        };
        table(&mut state.doc, &target.js());
        for (key, item) in entries(state, src, &dir_path, config) {
            let key_path = join(&dir_path, key);
            if ROOT_ONLY.contains(&key) {
                state.note(
                    src,
                    NoteClass::Human,
                    key_path,
                    "knip reads this key at the top level only; it has no effect inside a workspace",
                );
            } else {
                workspace_key(state, src, &target, key, &key_path, item);
            }
        }
    }
}

fn rules(state: &mut State<'_>, src: &Src, path: &str, value: &ConfigValue, plan: &mut Plan) {
    for (issue_type, item) in entries(state, src, path, value) {
        let item_path = join(path, issue_type);
        if let Some(text) = string(state, src, &item_path, item)
            && let Some(parsed) = level(state, src, &item_path, &text)
        {
            plan.rules.push((issue_type.to_owned(), parsed));
        }
    }
    plan.origin(path.to_owned());
}

fn tags(state: &mut State<'_>, src: &Src, path: &str, list: &[String], plan: &mut Plan) {
    for (index, tag) in list.iter().enumerate() {
        if let Some(name) = tag.strip_prefix('-') {
            plan.tags.push(name.to_owned());
        } else {
            state.note_with(
                src,
                NoteClass::Unsupported,
                format!("{path}[{index}]"),
                format!("`{tag}` limits the report to exports with that tag"),
                "skif can only ignore exports by tag (`-tag`)",
            );
        }
    }
    if !plan.tags.is_empty() {
        plan.origin(path.to_owned());
    }
}

/// Writes `list` to `key` of the table `path` and notes it. Nothing happens for an empty list.
fn write_list(
    state: &mut State<'_>,
    src: &Src,
    path: &[String],
    key: &str,
    note_path: &str,
    list: Vec<String>,
    extra: &str,
) {
    if list.is_empty() {
        return;
    }
    let marked = list.iter().any(|entry| entry.ends_with('!'));
    table(&mut state.doc, path).extend_strs(key, list);
    let suffix = if marked {
        " (the `!` production markers are kept)"
    } else {
        ""
    };
    state.note(
        src,
        NoteClass::Exact,
        note_path,
        format!("written to {} {key}{extra}{suffix}", Target::describe(path)),
    );
}

fn plugin_value(state: &mut State<'_>, src: &Src, path: &str, value: &ConfigValue) -> Val {
    match value {
        ConfigValue::Bool(enabled) => Val::Bool(*enabled),
        ConfigValue::String(_) | ConfigValue::Array(_) => {
            let config = string_list(state, src, path, value);
            Val::Inline(vec![("config".to_owned(), Val::strs(config))])
        }
        ConfigValue::Object(_) => {
            let mut options = Vec::new();
            for (key, item) in entries(state, src, path, value) {
                let key_path = join(path, key);
                if matches!(key, "config" | "entry" | "project") {
                    let list = string_list(state, src, &key_path, item);
                    if !list.is_empty() {
                        options.push((key.to_owned(), Val::strs(list)));
                    }
                } else {
                    state.note(
                        src,
                        NoteClass::Unsupported,
                        key_path,
                        format!("`{key}` is not a plugin option skif has"),
                    );
                }
            }
            if options.is_empty() {
                Val::Bool(true)
            } else {
                Val::Inline(options)
            }
        }
        _ => {
            state.note(
                src,
                NoteClass::Human,
                path,
                "the plugin setting is only known at run time",
            );
            Val::Bool(true)
        }
    }
}

fn workspace_key(state: &mut State<'_>, src: &Src, target: &Target, key: &str, path: &str, item: &ConfigValue) {
    let js = target.js();
    match key {
        "entry" | "project" | "ignore" => {
            let list = string_list(state, src, path, item);
            write_list(state, src, &js, key, path, list, "");
        }
        "ignoreFiles" => {
            let list = string_list(state, src, path, item);
            write_list(state, src, &js, "ignore", path, list, "");
            state.note_with(
                src,
                NoteClass::Approximate,
                path,
                "knip hid these files from the unused files report only; skif's `ignore` hides them from every rule",
                "none needed unless another rule should still look at them",
            );
        }
        "ignoreDependencies" | "ignoreBinaries" => {
            let list = pattern_list(state, src, path, item, true);
            let name = if key == "ignoreDependencies" {
                "ignore-dependencies"
            } else {
                "ignore-binaries"
            };
            write_list(state, src, &js, name, path, list, "");
        }
        "ignoreUnresolved" => {
            let list = pattern_list(state, src, path, item, false);
            let name = if *target == Target::Root {
                keys::JS_IGNORE_UNRESOLVED
            } else {
                keys::WS_IGNORE_UNRESOLVED
            };
            write_list(state, src, &js, name, path, list, "");
        }
        "includeEntryExports" => {
            if let Some(enabled) = boolean(state, src, path, item) {
                let name = if *target == Target::Root {
                    keys::JS_INCLUDE_ENTRY_EXPORTS
                } else {
                    keys::WS_INCLUDE_ENTRY_EXPORTS
                };
                table(&mut state.doc, &js).set(name, Val::Bool(enabled));
                state.note(
                    src,
                    NoteClass::Exact,
                    path,
                    format!("written to {} {name}", Target::describe(&js)),
                );
            }
        }
        "paths" => paths(state, src, target, path, item),
        "ignoreMembers" => {
            let list = pattern_list(state, src, path, item, false);
            let global = Target::Root.js();
            write_list(state, src, &global, keys::JS_IGNORE_MEMBERS, path, list, "");
            widened(state, src, target, path);
        }
        "ignoreExportsUsedInFile" => ignore_exports_used_in_file(state, src, target, path, item),
        "ignoreIssues" => ignore_issues(state, src, target, path, item),
        "ignoreGlobalBinaries" => state.note(
            src,
            NoteClass::Unsupported,
            path,
            "skif has no list of binaries that are assumed to be installed globally",
        ),
        "compilers" | "asyncCompilers" | "preprocessor" | "preprocessorOptions" => state.note_with(
            src,
            NoteClass::Unsupported,
            path,
            "compilers and preprocessors are functions, which skif never runs",
            "skif reads Vue, Svelte, Astro and CSS files itself; other file types need a plugin or an entry pattern",
        ),
        other if registry::plugin(other).is_some() => {
            let value = plugin_value(state, src, path, item);
            let plugins = target.plugins();
            table(&mut state.doc, &plugins).set(other, value);
            state.note(
                src,
                NoteClass::Exact,
                path,
                format!("written to {} {other}", Target::describe(&plugins)),
            );
        }
        other => state.note(
            src,
            NoteClass::Unsupported,
            path,
            format!("`{other}` is not a knip option, and skif has no plugin of that name"),
        ),
    }
}

/// A setting that knip scopes to a workspace but that skif applies to the whole project.
fn widened(state: &mut State<'_>, src: &Src, target: &Target, path: &str) {
    if let Target::Dir(dir) = target {
        state.note(
            src,
            NoteClass::Approximate,
            path,
            format!("set for the workspace `{dir}`; skif applies it to every workspace"),
        );
    }
}

fn paths(state: &mut State<'_>, src: &Src, target: &Target, path: &str, value: &ConfigValue) {
    let mut section = target.workspace();
    section.push(keys::WS_PATHS.to_owned());
    let mut written = false;
    for (alias, item) in entries(state, src, path, value) {
        let targets = string_list(state, src, &join(path, alias), item);
        if !targets.is_empty() {
            table(&mut state.doc, &section).set(alias, Val::strs(targets));
            written = true;
        }
    }
    if written {
        state.note(
            src,
            NoteClass::Exact,
            path,
            format!("written to {}", Target::describe(&section)),
        );
    }
}

fn ignore_exports_used_in_file(state: &mut State<'_>, src: &Src, target: &Target, path: &str, value: &ConfigValue) {
    let js = Target::Root.js();
    let written = match value {
        ConfigValue::Bool(enabled) => Some((Val::Bool(*enabled), NoteClass::Exact)),
        ConfigValue::Object(_) => {
            let mut flags = Vec::new();
            for (kind, item) in entries(state, src, path, value) {
                if let Some(enabled) = boolean(state, src, &join(path, kind), item) {
                    flags.push((kind.to_owned(), Val::Bool(enabled)));
                }
            }
            Some((Val::Inline(flags), NoteClass::Approximate))
        }
        _ => {
            state.note(
                src,
                NoteClass::Human,
                path,
                "expected true, false or an object of booleans",
            );
            None
        }
    };
    if let Some((val, class)) = written {
        table(&mut state.doc, &js).set(keys::JS_IGNORE_EXPORTS_USED_IN_FILE, val);
        state.note(
            src,
            class,
            path,
            format!("written to [js] {}", keys::JS_IGNORE_EXPORTS_USED_IN_FILE),
        );
        widened(state, src, target, path);
    }
}

fn ignore_issues(state: &mut State<'_>, src: &Src, target: &Target, path: &str, value: &ConfigValue) {
    let types = tables::knip_types();
    for (file, item) in entries(state, src, path, value) {
        let item_path = join(path, file);
        let mut rule_ids: Vec<&str> = Vec::new();
        for name in string_list(state, src, &item_path, item) {
            match types.iter().find(|known| known.name == name) {
                Some(known) if !known.rules.is_empty() => {
                    for rule in &known.rules {
                        if !rule_ids.contains(rule) {
                            rule_ids.push(*rule);
                        }
                    }
                }
                _ => state.note(
                    src,
                    NoteClass::Unsupported,
                    item_path.clone(),
                    format!("`{name}` is not an issue type skif has a rule for"),
                ),
            }
        }
        if rule_ids.is_empty() {
            continue;
        }
        let element = state
            .doc
            .element(&["overrides"], vec![format!("knip ignoreIssues for {file}")]);
        element.set("files", Val::strs([target.file_glob(file)]));
        element.set(
            "rules",
            Val::Inline(rule_ids.iter().map(|id| ((*id).to_owned(), Val::str("off"))).collect()),
        );
        state.note(
            src,
            NoteClass::Exact,
            item_path,
            "written to [[overrides]] with the matching rules off",
        );
    }
}

fn read_flags(state: &mut State<'_>, src: &Src, invocation: &Invocation, plan: &mut Plan) {
    let path = invocation.path();
    let flag_path = |flag: &str| format!("{path}: {flag}");
    for (flag, types) in SHORTCUTS {
        if invocation.has(flag, None) {
            plan.include.extend(types.iter().map(|name| (*name).to_owned()));
            plan.origin(flag_path(&format!("--{flag}")));
        }
    }
    let include = invocation.values("include", None);
    if !include.is_empty() {
        plan.include.extend(include);
        plan.origin(flag_path("--include"));
    }
    let exclude = invocation.values("exclude", None);
    if !exclude.is_empty() {
        plan.exclude.extend(exclude);
        plan.origin(flag_path("--exclude"));
    }
    let production = invocation.has("production", Some("p"));
    let strict = invocation.has("strict", Some("s"));
    if production || strict {
        plan.production = true;
        state.note(
            src,
            NoteClass::Exact,
            flag_path(if strict { "--strict" } else { "--production" }),
            "written to [js] production",
        );
    }
    if strict {
        state.note_with(
            src,
            NoteClass::Unsupported,
            flag_path("--strict"),
            "knip's strict mode also reports only direct dependencies and checks no other workspace",
            "skif only has the production half of it",
        );
    }
    if invocation.has("include-entry-exports", None) {
        plan.include_entry_exports = true;
        state.note(
            src,
            NoteClass::Exact,
            flag_path("--include-entry-exports"),
            "written to [js] include-entry-exports",
        );
    }
    if invocation.has("treat-config-hints-as-errors", None) {
        plan.hints = Some(Level::Error);
        plan.origin(flag_path("--treat-config-hints-as-errors"));
    }
    if invocation.has("no-config-hints", None) {
        plan.hints = Some(Level::Off);
        plan.origin(flag_path("--no-config-hints"));
    }
    let tag_flags = invocation.values("tags", None);
    if !tag_flags.is_empty() {
        tags(state, src, &flag_path("--tags"), &tag_flags, plan);
    }
    other_flags(state, src, invocation);
}

/// The flags that need a note and nothing else.
fn other_flags(state: &mut State<'_>, src: &Src, invocation: &Invocation) {
    let path = invocation.path();
    let notes: [(&str, NoteClass, &str, &str); 7] = [
        (
            "--treat-tag-hints-as-errors",
            NoteClass::Unsupported,
            "skif has no tag hints to turn into errors",
            "",
        ),
        (
            "--workspace",
            NoteClass::Human,
            "knip ran for some workspaces only; skif analyses every workspace of the root",
            "run skif in the workspace directory to narrow it",
        ),
        (
            "--max-issues",
            NoteClass::Approximate,
            "knip tolerated this many issues",
            "use `skif --max-warnings N` with the rules at `warn`",
        ),
        (
            "--no-exit-code",
            NoteClass::Human,
            "knip always exited 0",
            "skif exits 1 when it reports an error; lower the levels in [rules] or [lint] max-level",
        ),
        (
            "--fix",
            NoteClass::Default,
            "knip removed unused code and dependencies",
            "run `skif --fix` for the fixes skif has",
        ),
        (
            "--no-gitignore",
            NoteClass::Human,
            "knip read files that .gitignore hides",
            "skif always honours .gitignore",
        ),
        (
            "--preprocessor",
            NoteClass::Unsupported,
            "preprocessors are modules that skif never runs",
            "",
        ),
    ];
    for (flag, class, message, suggestion) in notes {
        let short = if flag == "--workspace" { Some("W") } else { None };
        if invocation.has(&flag[2..], short) {
            let note_path = format!("{path}: {flag}");
            if suggestion.is_empty() {
                state.note(src, class, note_path, message);
            } else {
                state.note_with(src, class, note_path, message, suggestion);
            }
        }
    }
    for flag in invocation.flags() {
        let known = HANDLED_FLAGS.contains(&flag.as_str()) || INERT_FLAGS.contains(&flag.as_str());
        if !known {
            state.note(
                src,
                NoteClass::Human,
                format!("{path}: {flag}"),
                "this knip flag is not translated",
            );
        }
    }
}

fn knip_level(plan: &Plan, name: &str) -> Level {
    plan.rules
        .iter()
        .rev()
        .find(|(known, _)| known == name)
        .map_or(Level::Error, |(_, level)| *level)
}

/// Turns the issue types into rule levels and `[js]` settings.
fn apply(state: &mut State<'_>, src: &Src, plan: &Plan) {
    let types = tables::knip_types();
    let find = |name: &str| types.iter().find(|known| known.name == name);
    let mut mentioned = plan
        .include
        .iter()
        .chain(&plan.exclude)
        .cloned()
        .collect::<Vec<String>>();
    mentioned.extend(plan.rules.iter().map(|(name, _)| name.clone()));
    for name in &mentioned {
        match find(name) {
            None => state.note(
                src,
                NoteClass::Unsupported,
                plan.origins.join(", "),
                format!("`{name}` is not a knip issue type"),
            ),
            Some(known) if known.rules.is_empty() => state.note(
                src,
                NoteClass::Unsupported,
                plan.origins.join(", "),
                format!("skif has no rule for the `{name}` issue type"),
            ),
            Some(_) => {}
        }
    }
    let base: Vec<&KnipType> = if plan.include.is_empty() {
        types.iter().filter(|known| known.default).collect()
    } else {
        plan.include.iter().filter_map(|name| find(name)).collect()
    };
    let mut enabled: Vec<&KnipType> = Vec::new();
    for known in base {
        let removed = plan.exclude.iter().any(|name| name == known.name) || knip_level(plan, known.name) == Level::Off;
        if !removed && !enabled.contains(&known) {
            enabled.push(known);
        }
    }
    for known in &types {
        let removed = plan.exclude.iter().any(|name| name == known.name) || knip_level(plan, known.name) == Level::Off;
        if removed {
            for rule in &known.rules {
                state.set_level(rule, Level::Off);
            }
        }
    }
    let mut written = Vec::new();
    for known in &enabled {
        for rule in &known.rules {
            state.keep([*rule]);
            state.set_level(rule, knip_level(plan, known.name));
            if !written.contains(rule) {
                written.push(*rule);
            }
        }
    }
    state.note_with(
        src,
        NoteClass::Exact,
        if plan.origins.is_empty() {
            "issue types".to_owned()
        } else {
            plan.origins.join(", ")
        },
        format!(
            "{} rules are written at knip's own severity (error unless `rules` says otherwise): {}",
            written.len(),
            written.join(", ")
        ),
        "the `exact` profile turns every other rule off; `full` leaves skif's defaults for them",
    );
    js_settings(state, src, plan);
}

fn js_settings(state: &mut State<'_>, src: &Src, plan: &Plan) {
    if !plan.tags.is_empty() {
        state.add_ignore_tags("unused-exports", plan.tags.iter().cloned());
        state.note(
            src,
            NoteClass::Exact,
            "tags",
            format!(
                "exports tagged {} are ignored by `unused-exports`",
                plan.tags.join(", ")
            ),
        );
    }
    if let Some(hints) = plan.hints {
        state.set_level(keys::RULE_STALE_JS_CONFIG, hints);
        state.note(
            src,
            NoteClass::Exact,
            "configuration hints",
            format!(
                "[rules] {} is {}",
                keys::RULE_STALE_JS_CONFIG,
                if hints == Level::Error { "error" } else { "off" }
            ),
        );
    }
    let js = Target::Root.js();
    if plan.production {
        table(&mut state.doc, &js).set(keys::JS_PRODUCTION, Val::Bool(true));
    }
    if plan.include_entry_exports {
        table(&mut state.doc, &js).set(keys::JS_INCLUDE_ENTRY_EXPORTS, Val::Bool(true));
    }
}
