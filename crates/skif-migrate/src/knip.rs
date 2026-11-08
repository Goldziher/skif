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

/// What `--dependencies` and its siblings include. `--dependencies` also adds `devDependencies` and
/// `optionalPeerDependencies` outside production mode, which [`Selection::new`] does.
const SHORTCUTS: &[(&str, &[&str])] = &[
    (
        "dependencies",
        &[
            "dependencies",
            "unlisted",
            "binaries",
            "unresolved",
            "catalog",
            "catalogReferences",
        ],
    ),
    (
        "exports",
        &["exports", "types", "enumMembers", "namespaceMembers", "duplicates"],
    ),
    ("files", &["files"]),
    ("cycles", &["cycles"]),
];

/// The knip issue types the shortcut flag `--<flag>` (`dependencies`, `exports`, `files` or `cycles`) selects, as
/// knip works them out: outside production mode `--dependencies` also selects `devDependencies` and
/// `optionalPeerDependencies`. `None` for any other flag.
#[must_use]
pub fn knip_shortcut_types(flag: &str, production: bool) -> Option<Vec<&'static str>> {
    let (_, types) = SHORTCUTS.iter().find(|(name, _)| *name == flag)?;
    let mut types = types.to_vec();
    if flag == "dependencies" && !production {
        types.extend(["devDependencies", "optionalPeerDependencies"]);
    }
    Some(types)
}

/// The skif rules that report the findings of the knip issue type `name` (empty when skif has none), or `None` when
/// `name` is not a knip issue type.
#[must_use]
pub fn knip_issue_type_rules(name: &str) -> Option<Vec<&'static str>> {
    tables::knip_types()
        .into_iter()
        .find(|knip_type| knip_type.name == name)
        .map(|knip_type| knip_type.rules)
}

/// Issue types that only add to the defaults when they are the only ones included.
const ADD_ON: &[&str] = &["nsExports", "nsTypes"];

/// Issue types production mode leaves out.
const PRODUCTION_EXCLUDED: &[&str] = &["devDependencies", "catalog", "catalogReferences"];

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
    "--tsConfig",
    "-t",
    "--use-tsconfig-files",
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

/// One run of knip: the flags of one command of one script.
#[derive(Default)]
struct Run {
    /// `scripts.<name>`.
    script: String,
    /// `--include` and the shortcut flags, which override the configuration.
    include: Vec<String>,
    /// `--exclude`, which overrides the configuration.
    exclude: Vec<String>,
    /// `--production`, or `--strict`, which implies it.
    production: bool,
    /// The run passed `--production` itself.
    explicit_production: bool,
}

/// What the configuration and the flags say about the issue types.
#[derive(Default)]
struct Plan {
    /// `include` of the configuration.
    include: Vec<String>,
    /// `exclude` of the configuration.
    exclude: Vec<String>,
    rules: Vec<(String, Level)>,
    tags: Vec<String>,
    /// `+tag` entries: the report is limited to exports with one of these tags.
    include_tags: Vec<String>,
    hints: Option<Level>,
    include_entry_exports: bool,
    /// `--tsConfig` of a script.
    tsconfig: Option<String>,
    use_tsconfig_files: bool,
    runs: Vec<Run>,
    /// Where the issue type settings came from, for the note.
    origins: Vec<String>,
}

/// The issue types one run reports, worked out as knip does.
struct Selection {
    included: Vec<String>,
    excluded: Vec<String>,
}

fn extend_names(list: &mut Vec<String>, names: &[&str]) {
    list.extend(names.iter().map(|name| (*name).to_owned()));
}

impl Selection {
    fn same_types(&self, other: &Self) -> bool {
        let sorted = |names: &[String]| {
            let mut names = names.to_vec();
            names.sort();
            names
        };
        sorted(&self.included) == sorted(&other.included)
    }

    /// knip's `getIncludedIssueTypes`: the flags override the configuration, production mode drops the types that
    /// need dev dependencies, and `--dependencies` brings in the dev dependencies outside production mode.
    fn new(plan: &Plan, run: &Run, defaults: &[&str]) -> Self {
        let mut config_exclude: Vec<String> = plan
            .rules
            .iter()
            .map(|(name, _)| name)
            .filter(|name| knip_level(plan, name) == Level::Off)
            .cloned()
            .collect();
        config_exclude.extend(plan.exclude.iter().cloned());
        let mut inc: Vec<String> = run.include.clone();
        inc.extend(plan.include.iter().filter(|name| !run.exclude.contains(name)).cloned());
        let mut exc: Vec<String> = run.exclude.clone();
        exc.extend(config_exclude.into_iter().filter(|name| !run.include.contains(name)));
        if run.production {
            extend_names(&mut exc, PRODUCTION_EXCLUDED);
        } else {
            if inc.iter().any(|name| name == "dependencies") {
                extend_names(&mut inc, &["devDependencies", "optionalPeerDependencies"]);
            }
            if exc.iter().any(|name| name == "dependencies") {
                extend_names(&mut exc, &["devDependencies", "optionalPeerDependencies"]);
            }
        }
        let mut reported: Vec<String> = if inc.is_empty() {
            defaults.iter().map(|name| (*name).to_owned()).collect()
        } else if inc.iter().any(|name| !ADD_ON.contains(&name.as_str())) {
            inc
        } else {
            extend_names(&mut inc, defaults);
            inc
        };
        reported.retain(|name| !exc.contains(name));
        Self {
            included: reported,
            excluded: exc,
        }
    }
}

impl Plan {
    fn origin(&mut self, origin: String) {
        if !self.origins.contains(&origin) {
            self.origins.push(origin);
        }
    }
}

pub(crate) fn translate(input: &ToolInput, state: &mut State<'_>) -> Result<(), MigrateError> {
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
        let invocations = scripts::invocations(package, Tool::Knip);
        for invocation in &invocations {
            read_flags(state, &src, invocation, &mut plan);
        }
        scripts::note_unrecognised(state, &src, package, Tool::Knip);
        note_differing_flags(state, &src, &invocations);
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
                    state.note_with(
                        src,
                        NoteClass::Approximate,
                        path,
                        "skif reports tag hints through `stale-js-config`, together with the configuration hints",
                        "run `skif migrate --strict-hints` to make `stale-js-config` an error",
                    );
                }
            }
            "cycles" => cycles(state, src, &path, item),
            _ => workspace_key(state, src, &target, key, &path, item),
        }
    }
}

/// `cycles`: `allow` and `dynamicImports` go to `[js.cycles]`.
fn cycles(state: &mut State<'_>, src: &Src, path: &str, value: &ConfigValue) {
    let section = ["js".to_owned(), "cycles".to_owned()];
    for (key, item) in entries(state, src, path, value) {
        let key_path = join(path, key);
        match key {
            "dynamicImports" => {
                if let Some(enabled) = boolean(state, src, &key_path, item) {
                    table(&mut state.doc, &section).set("dynamic-imports", Val::Bool(enabled));
                    state.note(
                        src,
                        NoteClass::Exact,
                        key_path,
                        "written to [js.cycles] dynamic-imports",
                    );
                }
            }
            "allow" => {
                let ConfigValue::Array(items) = item else {
                    state.note(src, NoteClass::Human, key_path, "expected an array of file lists");
                    continue;
                };
                let mut allowed = Vec::new();
                for (index, files) in items.iter().enumerate() {
                    let files = string_list(state, src, &format!("{key_path}[{index}]"), files);
                    if !files.is_empty() {
                        allowed.push(Val::strs(files));
                    }
                }
                if !allowed.is_empty() {
                    table(&mut state.doc, &section).set("allow", Val::List(allowed));
                    state.note(
                        src,
                        NoteClass::Approximate,
                        key_path,
                        "written to [js.cycles] allow; skif reports a cycle as the set of its files, so an entry \
                         allows the cycle with exactly those files in any order",
                    );
                }
            }
            _ => state.note(
                src,
                NoteClass::Human,
                key_path,
                "not a knip `cycles` option (`allow`, `dynamicImports`)",
            ),
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
        let name = tag.trim_start_matches(['-', '+']).trim_start_matches('@');
        if name.is_empty() {
            state.note_with(
                src,
                NoteClass::Unsupported,
                format!("{path}[{index}]"),
                format!("`{tag}` names no tag"),
                "write the tag name, prefixed with `-` to ignore or `+` to include",
            );
        } else if tag.starts_with('-') {
            plan.tags.push(name.to_owned());
        } else {
            plan.include_tags.push(name.to_owned());
        }
    }
    if !plan.tags.is_empty() || !plan.include_tags.is_empty() {
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
            write_list(state, src, &js, "ignore-files", path, list, "");
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
            write_list(state, src, &js, keys::JS_IGNORE_MEMBERS, path, list, "");
        }
        "ignoreExportsUsedInFile" => ignore_exports_used_in_file(state, src, target, path, item),
        "ignoreIssues" => ignore_issues(state, src, target, path, item),
        "ignoreGlobalBinaries" => match (boolean(state, src, path, item), target) {
            (Some(true), _) => state.note(
                src,
                NoteClass::Exact,
                path,
                "skif ignores the binaries that are assumed to be installed globally by default, as knip does",
            ),
            (Some(false), Target::Root) => {
                table(&mut state.doc, &js).set(keys::JS_IGNORE_GLOBAL_BINARIES, Val::Bool(false));
                state.note(
                    src,
                    NoteClass::Exact,
                    path,
                    format!(
                        "written to {} {}",
                        Target::describe(&js),
                        keys::JS_IGNORE_GLOBAL_BINARIES
                    ),
                );
            }
            (Some(false), Target::Dir(_)) => state.note(
                src,
                NoteClass::Unsupported,
                path,
                "skif sets `ignore-global-binaries` for the whole run only, at the top level of `[js]`",
            ),
            (None, _) => {}
        },
        "compilers" => compilers(state, src, path, item),
        "asyncCompilers" | "preprocessor" | "preprocessorOptions" => state.note_with(
            src,
            NoteClass::Unsupported,
            path,
            "compilers and preprocessors are functions, which skif never runs",
            BUILT_IN_EXTRACTORS_HINT,
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

/// The extensions whose imports skif extracts itself, without running a compiler (ADR 0001).
const BUILT_IN_EXTRACTORS: &[&str] = &[
    "astro", "css", "less", "mdx", "pcss", "sass", "scss", "styl", "stylus", "svelte", "tsrx", "vue",
];

const BUILT_IN_EXTRACTORS_HINT: &str = "skif reads Vue, Svelte, Astro, MDX, TSRX, CSS, Sass, Less and Stylus files itself; \
                                         other file types need a plugin or an entry pattern";

/// Extensions whose files are outside skif's default `project`: knip reports unused ones once a compiler reads them.
const OUTSIDE_DEFAULT_PROJECT: &[&str] = &["mdx", "tsrx"];

/// `compilers`: `true` for an extension (or for all of them) asks knip for its built-in compiler, which skif's own
/// extractor stands in for. A function is a custom compiler, which skif never runs.
fn compilers(state: &mut State<'_>, src: &Src, path: &str, value: &ConfigValue) {
    if matches!(value, ConfigValue::Bool(true)) {
        state.note(
            src,
            NoteClass::Default,
            path,
            "skif extracts the imports of Vue, Svelte, Astro, MDX, TSRX and stylesheet files itself, without a compiler",
        );
        return;
    }
    for (extension, compiler) in entries(state, src, path, value) {
        // `.styl` is written as a quoted key, so it is not joined into `compilers..styl`.
        let at = if extension
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'$')
        {
            join(path, extension)
        } else {
            format!("{path}[\"{extension}\"]")
        };
        let name = extension.trim_start_matches('.').to_ascii_lowercase();
        let built_in = BUILT_IN_EXTRACTORS.contains(&name.as_str());
        match compiler {
            ConfigValue::Bool(true) if built_in && OUTSIDE_DEFAULT_PROJECT.contains(&name.as_str()) => state.note_with(
                src,
                NoteClass::Default,
                at,
                format!("skif reads `.{name}` files with its built-in extractor"),
                format!("add `**/*.{name}` to `project` to report unused `.{name}` files, as knip does"),
            ),
            ConfigValue::Bool(true) if built_in => state.note(
                src,
                NoteClass::Default,
                at,
                format!("skif reads `.{name}` files with its built-in extractor"),
            ),
            ConfigValue::Bool(true) => state.note_with(
                src,
                NoteClass::Unsupported,
                at,
                format!("neither knip nor skif has a built-in compiler for `.{name}` files"),
                BUILT_IN_EXTRACTORS_HINT,
            ),
            ConfigValue::Bool(false) => state.note(
                src,
                NoteClass::Default,
                at,
                "knip ignores `false`, and skif's built-in extractors are always on",
            ),
            _ if built_in => state.note_with(
                src,
                NoteClass::Approximate,
                at,
                "custom compilers are functions, which skif never runs",
                format!(
                    "skif reads `.{name}` files with its built-in extractor; check that it finds what this compiler did"
                ),
            ),
            _ => state.note_with(
                src,
                NoteClass::Unsupported,
                at,
                "custom compilers are functions, which skif never runs",
                BUILT_IN_EXTRACTORS_HINT,
            ),
        }
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
    let js = target.js();
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
            format!(
                "written to {} {}",
                Target::describe(&js),
                keys::JS_IGNORE_EXPORTS_USED_IN_FILE
            ),
        );
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
                Some(_) => state.note(
                    src,
                    NoteClass::Unsupported,
                    item_path.clone(),
                    format!("skif has no rule for the `{name}` issue type"),
                ),
                None => state.note(
                    src,
                    NoteClass::Unsupported,
                    item_path.clone(),
                    format!("`{name}` is not a knip issue type"),
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
    let mut run = Run {
        script: path.clone(),
        ..Run::default()
    };
    for (flag, types) in SHORTCUTS {
        if invocation.has(flag, None) {
            run.include.extend(types.iter().map(|name| (*name).to_owned()));
            plan.origin(flag_path(&format!("--{flag}")));
        }
    }
    let include = invocation.values("include", None);
    if !include.is_empty() {
        run.include.extend(include);
        plan.origin(flag_path("--include"));
    }
    let exclude = invocation.values("exclude", None);
    if !exclude.is_empty() {
        run.exclude.extend(exclude);
        plan.origin(flag_path("--exclude"));
    }
    run.explicit_production = invocation.has("production", Some("p"));
    let strict = invocation.has("strict", Some("s"));
    run.production = run.explicit_production || strict;
    if strict {
        state.note_with(
            src,
            NoteClass::Unsupported,
            flag_path("--strict"),
            "knip's strict mode also reports only direct dependencies and checks no other workspace",
            "skif only has the production half of it, which is written to [js] production",
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
    if let Some(path) = invocation.values("tsConfig", Some("t")).pop() {
        plan.tsconfig = Some(path);
        state.note(
            src,
            NoteClass::Exact,
            flag_path("--tsConfig"),
            "written to [js] tsconfig",
        );
    }
    if invocation.has("use-tsconfig-files", None) {
        plan.use_tsconfig_files = true;
        state.note(
            src,
            NoteClass::Exact,
            flag_path("--use-tsconfig-files"),
            "written to [js] use-tsconfig-files",
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
    plan.runs.push(run);
    other_flags(state, src, invocation);
}

/// Flags that change what skif reports for the whole project but that only some scripts passed, or passed with
/// different values.
fn note_differing_flags(state: &mut State<'_>, src: &Src, invocations: &[Invocation]) {
    if let Some(scripts) = scripts::differing_values(invocations, "tsConfig", Some("t")) {
        state.note_with(
            src,
            NoteClass::Approximate,
            format!("{scripts}: --tsConfig"),
            "these scripts do not all pass the same --tsConfig; skif has one [js] tsconfig for the project, so the \
             last one is written and applies to all",
            "check that every check should use it",
        );
    }
    for flag in [
        "include-entry-exports",
        "use-tsconfig-files",
        "treat-config-hints-as-errors",
        "no-config-hints",
    ] {
        if let Some(scripts) = scripts::differing_switch(invocations, flag, None) {
            state.note_with(
                src,
                NoteClass::Approximate,
                format!("{scripts}: --{flag}"),
                format!("only some of these scripts pass --{flag}; skif has one setting for the project, so it applies to all"),
                "check that every check should use it",
            );
        }
    }
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
            "pass the same `--workspace` to skif",
        ),
        (
            "--max-issues",
            NoteClass::Approximate,
            "knip tolerated this many issues",
            "pass the same `--max-issues` to skif",
        ),
        (
            "--no-exit-code",
            NoteClass::Human,
            "knip always exited 0",
            "pass `--no-exit-code` to skif, or lower the levels in [rules]",
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
            "pass `--no-gitignore` to skif",
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

/// The names of `names` once each, in order.
fn unique(names: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for name in names {
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Turns the issue types into rule levels and `[js]` settings.
fn apply(state: &mut State<'_>, src: &Src, plan: &Plan) {
    let types = tables::knip_types();
    let find = |name: &str| types.iter().find(|known| known.name == name);
    let mut mentioned: Vec<String> = plan.include.iter().chain(&plan.exclude).cloned().collect();
    for run in &plan.runs {
        mentioned.extend(run.include.iter().chain(&run.exclude).cloned());
    }
    mentioned.extend(plan.rules.iter().map(|(name, _)| name.clone()));
    for name in unique(mentioned) {
        match find(&name) {
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
    let defaults: Vec<&str> = types
        .iter()
        .filter(|known| known.default)
        .map(|known| known.name)
        .collect();
    let silent = Run::default();
    let runs: Vec<&Run> = if plan.runs.is_empty() {
        vec![&silent]
    } else {
        plan.runs.iter().collect()
    };
    let selections: Vec<Selection> = runs.iter().map(|run| Selection::new(plan, run, &defaults)).collect();
    if selections.windows(2).any(|pair| !pair[0].same_types(&pair[1])) {
        let scripts: Vec<&str> = runs.iter().map(|run| run.script.as_str()).collect();
        state.note_with(
            src,
            NoteClass::Approximate,
            format!("{}: issue types", scripts.join(", ")),
            "these scripts report different issue types; skif has one rule set, so the rules of every type any of them \
             reports are enabled",
            "narrow single rules with [[overrides]] if a check should not run everywhere",
        );
    }
    let removed = |known: &KnipType| {
        knip_level(plan, known.name) == Level::Off
            || selections
                .iter()
                .all(|selection| selection.excluded.iter().any(|name| name == known.name))
    };
    for known in &types {
        if removed(known) {
            for rule in &known.rules {
                state.set_level(rule, Level::Off);
            }
        }
    }
    let mut written = Vec::new();
    for known in &types {
        let reported = selections
            .iter()
            .any(|selection| selection.included.iter().any(|name| name == known.name));
        if removed(known) || !reported {
            continue;
        }
        for rule in &known.rules {
            state.keep(Tool::Knip, [*rule]);
            state.set_level(rule, knip_level(plan, known.name));
            if !written.contains(rule) {
                written.push(*rule);
            }
        }
    }
    if written.contains(&keys::RULE_UNUSED_CATALOG_ENTRIES) {
        report_unused_catalog_entries(state, src);
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

/// knip reports unused catalog entries by default; `unused-catalog-entries` only reports them when
/// `[catalog] report-unused-entries` is `true`, so the rule level alone would not keep knip's findings.
fn report_unused_catalog_entries(state: &mut State<'_>, src: &Src) {
    state
        .doc
        .table(&[keys::CATALOG])
        .set(keys::CATALOG_REPORT_UNUSED_ENTRIES, Val::Bool(true));
    state.note(
        src,
        NoteClass::Exact,
        "catalog",
        format!(
            "[catalog] {} = true: knip reports unused catalog entries, and `{}` needs it to report them",
            keys::CATALOG_REPORT_UNUSED_ENTRIES,
            keys::RULE_UNUSED_CATALOG_ENTRIES
        ),
    );
}

/// knip's configuration hints are not skif's: `stale-js-config` also flags entries skif itself never needed, so an
/// `error` could fail a project that knip passed. It is written as `warn` unless `--strict-hints` asks for `error`.
fn config_hints(state: &mut State<'_>, src: &Src, hints: Level) {
    let rule = keys::RULE_STALE_JS_CONFIG;
    if hints == Level::Error && !state.options.strict_hints {
        state.set_level(rule, Level::Warn);
        state.note_with(
            src,
            NoteClass::Approximate,
            "configuration hints",
            format!(
                "[rules] {rule} is warn: skif's hints differ from knip's (they also flag entries skif never needed), so \
                 error could fail a project that knip passed"
            ),
            "run `skif migrate --strict-hints` to write error",
        );
        return;
    }
    state.set_level(rule, hints);
    state.note(
        src,
        NoteClass::Exact,
        "configuration hints",
        format!(
            "[rules] {rule} is {}",
            if hints == Level::Error { "error" } else { "off" }
        ),
    );
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
    if !plan.include_tags.is_empty() {
        state.add_include_tags("unused-exports", plan.include_tags.iter().cloned());
        let exempting: Vec<&str> = plan
            .include_tags
            .iter()
            .map(String::as_str)
            .filter(|tag| matches!(*tag, "public" | "beta" | "alpha" | "alias" | "internal"))
            .collect();
        let listed = format!(
            "only exports tagged {} are reported by `unused-exports`",
            plan.include_tags.join(", ")
        );
        if exempting.is_empty() {
            state.note(src, NoteClass::Exact, "tags", listed);
        } else {
            // knip keeps exempting these tags even when included, so it reports none of their exports; skif judges them.
            state.note(
                src,
                NoteClass::Approximate,
                "tags",
                format!(
                    "{listed}; knip never reports exports tagged {}, skif reports them when they are unused",
                    exempting.join(", ")
                ),
            );
        }
    }
    if let Some(hints) = plan.hints {
        config_hints(state, src, hints);
    }
    let js = Target::Root.js();
    production(state, src, plan);
    if plan.include_entry_exports {
        table(&mut state.doc, &js).set(keys::JS_INCLUDE_ENTRY_EXPORTS, Val::Bool(true));
    }
    if let Some(path) = &plan.tsconfig {
        table(&mut state.doc, &js).set(keys::JS_TSCONFIG, Val::str(path.as_str()));
    }
    if plan.use_tsconfig_files {
        table(&mut state.doc, &js).set(keys::JS_USE_TSCONFIG_FILES, Val::Bool(true));
    }
}

/// `[js] production` is one setting for the project: it is written when every run of knip used production mode, and
/// left to a human when only some did.
fn production(state: &mut State<'_>, src: &Src, plan: &Plan) {
    let production: Vec<&Run> = plan.runs.iter().filter(|run| run.production).collect();
    if production.is_empty() {
        return;
    }
    if production.len() == plan.runs.len() {
        table(&mut state.doc, &Target::Root.js()).set(keys::JS_PRODUCTION, Val::Bool(true));
        let explicit: Vec<String> = production
            .iter()
            .filter(|run| run.explicit_production)
            .map(|run| format!("{}: --production", run.script))
            .collect();
        if !explicit.is_empty() {
            state.note(src, NoteClass::Exact, explicit.join(", "), "written to [js] production");
        }
    } else {
        let with: Vec<&str> = production.iter().map(|run| run.script.as_str()).collect();
        let without: Vec<&str> = plan
            .runs
            .iter()
            .filter(|run| !run.production)
            .map(|run| run.script.as_str())
            .collect();
        state.note_with(
            src,
            NoteClass::Human,
            format!("{}: --production", with.join(", ")),
            format!(
                "{} ran knip in production mode and {} did not; skif has one [js] production setting, so it was not written",
                with.join(", "),
                without.join(", ")
            ),
            "set [js] production = true if every check should skip dev dependencies and test files",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{knip_issue_type_rules, knip_shortcut_types};

    #[test]
    fn issue_types_and_shortcuts_name_rules() {
        assert_eq!(knip_issue_type_rules("files"), Some(vec!["unused-files"]));
        assert_eq!(knip_issue_type_rules("unused-files"), None);
        assert_eq!(knip_shortcut_types("files", false), Some(vec!["files"]));
        let dependencies = knip_shortcut_types("dependencies", false).unwrap();
        assert!(dependencies.contains(&"devDependencies"));
        let production = knip_shortcut_types("dependencies", true).unwrap();
        assert!(!production.contains(&"devDependencies"));
        assert_eq!(knip_shortcut_types("tags", false), None);
    }
}
