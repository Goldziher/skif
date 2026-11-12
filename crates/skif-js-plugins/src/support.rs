//! Small helpers shared by the plugins: package and path classification, naming conventions and
//! typed access to [`ConfigValue`]s.

use skif_core::HashMap;
use std::path::{Path, PathBuf};

use skif_js_syntax::{ConfigValue, OWN_SPREAD, is_declaration_file};

use crate::plugin::{ConfigAst, IncompleteScope, PluginContext, PluginInput, PluginSpec};

const FILE_EXTENSIONS: &[&str] = &[
    "js", "jsx", "ts", "tsx", "mjs", "cjs", "mts", "cts", "json", "css", "scss", "html", "vue", "svelte",
];

/// The package name of a module specifier, `None` for paths, aliases, URLs and protocol specifiers.
pub(crate) fn package_name(specifier: &str) -> Option<String> {
    let specifier = specifier.split('?').next().unwrap_or_default().trim();
    if specifier.is_empty() || specifier.starts_with(['.', '/', '~', '#', '$', '<', '!']) || specifier.contains(':') {
        return None;
    }
    let mut parts = specifier.split('/');
    let first = parts.next()?;
    if first.starts_with('@') {
        let second = parts.next().filter(|s| !s.is_empty())?;
        Some(format!("{first}/{second}"))
    } else {
        Some(first.to_string())
    }
}

/// Whether the specifier is a relative path.
pub(crate) fn is_relative(specifier: &str) -> bool {
    specifier == "."
        || specifier == ".."
        || specifier.starts_with("./")
        || specifier.starts_with("../")
        || specifier.starts_with(".\\")
}

/// Whether a bare value looks like a file name rather than a package (`setup.ts`, `test/setup.js`).
pub(crate) fn looks_like_file(value: &str) -> bool {
    if value.starts_with("@/") {
        return true;
    }
    if value.starts_with('@') {
        return false;
    }
    value
        .rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && FILE_EXTENSIONS.contains(&ext))
}

/// Joins `relative` onto `base` lexically, collapsing `.` and `..`. An absolute `relative` is returned as is.
pub(crate) fn join_rel(base: &str, relative: &str) -> String {
    if relative.starts_with('/') {
        return relative.to_string();
    }
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// The path of `to` relative to `from`, both `/`-separated and relative to the same directory without `.` or `..`:
/// `relative_path("packages/b", "")` is `../..`, `relative_path("packages/b", "packages/b/src")` is `src`.
pub(crate) fn relative_path(from: &str, to: &str) -> String {
    let from: Vec<&str> = from.split('/').filter(|part| !part.is_empty()).collect();
    let to: Vec<&str> = to.split('/').filter(|part| !part.is_empty()).collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut parts = vec![".."; from.len() - common];
    parts.extend(&to[common..]);
    parts.join("/")
}

/// `path`, relative to the same directory as `dir`, relative to `dir` instead when it lies in it (`""` for `dir`
/// itself); `None` when it lies outside.
pub(crate) fn path_within(path: &str, dir: &str) -> Option<String> {
    if path == dir {
        return Some(String::new());
    }
    if dir.is_empty() {
        return Some(path.to_owned());
    }
    path.strip_prefix(dir)?.strip_prefix('/').map(str::to_owned)
}

/// The absolute path of `relative`, a `/`-separated path relative to the directory `dir`, with `.` and `..` collapsed
/// lexically: a config that extends `../base.json` is the file in the parent directory, not a path through `dir`.
pub(crate) fn join_lexically(dir: &Path, relative: &str) -> PathBuf {
    let mut path = dir.to_path_buf();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                path.pop();
            }
            other => path.push(other),
        }
    }
    path
}

/// Classifies a value a config uses to name a module or file: a relative path or `<rootDir>` path becomes an entry,
/// a package becomes a dependency. With `bare_is_file`, a bare name with a source extension is a file.
pub(crate) fn module_input(base: &str, specifier: &str, bare_is_file: bool) -> Option<PluginInput> {
    let specifier = specifier.trim();
    if specifier.is_empty() {
        return None;
    }
    if let Some(rest) = specifier.strip_prefix("<rootDir>") {
        return Some(PluginInput::Entry(join_rel(base, rest.trim_start_matches('/'))));
    }
    if is_relative(specifier) || (bare_is_file && looks_like_file(specifier)) {
        return Some(PluginInput::Entry(join_rel(base, specifier)));
    }
    package_name(specifier).map(PluginInput::Dependency)
}

/// A reference from one config file to another: a relative path is a further config file of the same tool, a bare name
/// a shared config package (`extends: ["config-standard"]`).
pub(crate) fn config_ref(base: &str, specifier: &str) -> Option<PluginInput> {
    let specifier = specifier.trim();
    if is_relative(specifier) {
        return Some(PluginInput::ConfigFile(join_rel(base, specifier)));
    }
    package_name(specifier).map(PluginInput::Dependency)
}

/// Every `script` of the scripts a config holds as a string or a list of strings.
pub(crate) fn script_inputs(value: &ConfigValue) -> Vec<PluginInput> {
    strings(value)
        .into_iter()
        .map(|script| PluginInput::Script(script.to_owned()))
        .collect()
}

/// A script that runs in `dir`, a directory relative to the workspace: a script of the workspace itself when `dir` is
/// its root, a [`PluginInput::ScriptAt`] otherwise. A directory that is computed, absolute or outside the workspace
/// drops the script.
pub(crate) fn script_in(dir: &str, script: String) -> Option<PluginInput> {
    let dir = dir.trim();
    if dir.starts_with('/') || dir.contains(['$', '`', '{']) {
        return None;
    }
    let dir = join_rel("", dir);
    if dir.is_empty() {
        return Some(PluginInput::Script(script));
    }
    (dir != ".." && !dir.starts_with("../")).then_some(PluginInput::ScriptAt { dir, script })
}

/// The path below the first `node_modules` directory of `path`: `@scope/pkg/cli.js` for
/// `./node_modules/@scope/pkg/cli.js`. `None` for a path outside one.
pub(crate) fn below_node_modules(path: &str) -> Option<&str> {
    path.strip_prefix("node_modules/").or_else(|| {
        let at = path.find("/node_modules/")?;
        Some(&path[at + "/node_modules/".len()..])
    })
}

/// The files, relative to the workspace, a `projects` string of a test runner names: a config file or a glob of them,
/// or a directory (or glob of directories) holding a file called like one of `names` (a brace group of file names).
/// `<rootDir>` stands for `base`. Negated strings name nothing.
pub(crate) fn project_files(cx: &PluginContext<'_>, base: &str, project: &str, names: &str) -> Vec<String> {
    let project = project.trim();
    if project.is_empty() || project.starts_with('!') {
        return Vec::new();
    }
    let rest = project
        .strip_prefix("<rootDir>")
        .map_or(project, |rest| rest.trim_start_matches('/'));
    let relative = join_rel(base, &extglob_to_braces(rest));
    let pattern = if looks_like_file(&relative) {
        relative
    } else {
        join_rel(&relative, names)
    };
    crate::files::find_files(cx.fs, cx.dir, &[pattern])
}

/// The entry for a glob or path written in a config. Negated patterns yield nothing.
pub(crate) fn entry_glob(base: &str, pattern: &str) -> Option<String> {
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern.starts_with('!') {
        return None;
    }
    let pattern = pattern
        .strip_prefix("<rootDir>")
        .map_or(pattern, |r| r.trim_start_matches('/'));
    Some(join_rel(base, &extglob_to_braces(pattern)))
}

/// Like [`entry_glob`], but a negated pattern stays negated: `!x` becomes `!<base>/x`, which removes files from the
/// entries as knip's `!` entries do.
pub(crate) fn entry_pattern(base: &str, pattern: &str) -> Option<String> {
    match pattern.trim().strip_prefix('!') {
        Some(rest) => entry_glob(base, rest).map(|glob| format!("!{glob}")),
        None => entry_glob(base, pattern),
    }
}

/// Rewrites the extglob groups of picomatch, the dialect knip's globs use, to the brace syntax `globset` knows.
///
/// `@(a|b)` and a bare `(a|b)` become `{a,b}`, `?(a|b)` and `*(a|b)` become `{a,b,}`, `+(a|b)` becomes `{a,b}`
/// (matching the repetitions is not possible; one occurrence is) and `!(a|b)` becomes `*` (a superset: negation
/// cannot be expressed). A bare group without `|` stays literal, so `app/(group)/page.tsx` still names a directory
/// called `(group)`. Escaped characters and character classes are copied as they are. A pattern with an unclosed
/// group is returned unchanged.
pub(crate) fn extglob_to_braces(pattern: &str) -> String {
    #[derive(Clone, Copy, PartialEq)]
    enum Kind {
        One,
        Optional,
        Plus,
        Not,
        Plain,
    }
    struct Frame {
        kind: Kind,
        body: String,
        alternatives: bool,
    }
    fn push(stack: &mut [Frame], out: &mut String, text: &str) {
        match stack.last_mut() {
            Some(frame) => frame.body.push_str(text),
            None => out.push_str(text),
        }
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len());
    let mut stack: Vec<Frame> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        let operator = match (c, next) {
            ('@', Some('(')) => Some(Kind::One),
            ('?' | '*', Some('(')) => Some(Kind::Optional),
            ('+', Some('(')) => Some(Kind::Plus),
            ('!', Some('(')) => Some(Kind::Not),
            _ => None,
        };
        if let Some(kind) = operator {
            stack.push(Frame {
                kind,
                body: String::new(),
                alternatives: false,
            });
            i += 2;
            continue;
        }
        match c {
            '\\' => {
                let end = (i + 2).min(chars.len());
                push(&mut stack, &mut out, &chars[i..end].iter().collect::<String>());
                i = end;
                continue;
            }
            '[' => {
                if let Some(close) = class_close(&chars, i) {
                    push(&mut stack, &mut out, &chars[i..=close].iter().collect::<String>());
                    i = close + 1;
                    continue;
                }
                push(&mut stack, &mut out, "[");
            }
            '(' => stack.push(Frame {
                kind: Kind::Plain,
                body: String::new(),
                alternatives: false,
            }),
            ')' => match stack.pop() {
                Some(frame) => {
                    let text = match frame.kind {
                        Kind::One | Kind::Plus => format!("{{{}}}", frame.body),
                        Kind::Optional => format!("{{{},}}", frame.body),
                        Kind::Not => "*".to_owned(),
                        Kind::Plain if frame.alternatives => format!("{{{}}}", frame.body),
                        Kind::Plain => format!("({})", frame.body),
                    };
                    push(&mut stack, &mut out, &text);
                }
                None => out.push(')'),
            },
            '|' if !stack.is_empty() => {
                if let Some(frame) = stack.last_mut() {
                    frame.alternatives = true;
                    frame.body.push(',');
                }
            }
            other => push(&mut stack, &mut out, other.encode_utf8(&mut [0; 4])),
        }
        i += 1;
    }
    if stack.is_empty() { out } else { pattern.to_owned() }
}

/// The index of the `]` closing the character class opened at `open`.
fn class_close(chars: &[char], open: usize) -> Option<usize> {
    let mut index = open + 1;
    if matches!(chars.get(index), Some('!' | '^')) {
        index += 1;
    }
    if chars.get(index) == Some(&']') {
        index += 1;
    }
    while index < chars.len() {
        match chars[index] {
            ']' => return Some(index),
            '\\' => index += 2,
            _ => index += 1,
        }
    }
    None
}

/// The package a tool names by convention: `foo` becomes `<ns>-foo`, `@scope` becomes `@scope/<ns>`,
/// `@scope/foo` becomes `@scope/<ns>-foo`. Names that already carry the prefix are kept.
pub(crate) fn conventional(id: &str, namespace: &str) -> String {
    let prefixed = |name: &str| {
        if name == namespace || name.starts_with(&format!("{namespace}-")) {
            name.to_string()
        } else {
            format!("{namespace}-{name}")
        }
    };
    let parts: Vec<&str> = id.split('/').collect();
    match parts.as_slice() {
        [scope, rest @ ..] if scope.starts_with('@') => match rest.first() {
            None | Some(&"") => format!("{scope}/{namespace}"),
            Some(name) => format!("{scope}/{}", prefixed(name)),
        },
        [name, ..] => prefixed(name),
        [] => id.to_string(),
    }
}

/// The strings of a string or an array of strings.
pub(crate) fn strings(value: &ConfigValue) -> Vec<&str> {
    match value {
        ConfigValue::String(s) => vec![s.as_str()],
        ConfigValue::Array(items) => items.iter().filter_map(ConfigValue::as_str).collect(),
        _ => Vec::new(),
    }
}

/// The files or patterns a string, or an array of strings, names: each alternative of the value, or of an element,
/// that is one of several ([`ConfigValue::alternatives`]) counts once, as every file a branch of a condition names is
/// a candidate. A file only another branch names is missing on disk, which leaves it out of the entries.
pub(crate) fn file_strings(value: &ConfigValue) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    for listed in value.alternatives() {
        let items = match listed {
            ConfigValue::Array(items) => items.as_slice(),
            single => std::slice::from_ref(single),
        };
        for alternative in items.iter().flat_map(ConfigValue::alternatives) {
            if let ConfigValue::String(text) = alternative
                && !out.contains(&text.as_str())
            {
                out.push(text);
            }
        }
    }
    out
}

/// Whether the files or patterns a value names are not all known: it has an unknown part
/// ([`ConfigValue::has_unknown`]) other than a value, an element or an object value that is one of several known
/// strings or lists of them, whose files [`file_strings`] reads.
pub(crate) fn files_unknown(value: &ConfigValue) -> bool {
    match value {
        ConfigValue::OneOf(alternatives) => alternatives.iter().any(|alternative| match alternative {
            ConfigValue::String(_) => false,
            ConfigValue::Array(items) => items.iter().any(file_element_unknown),
            _ => true,
        }),
        ConfigValue::Array(items) => items.iter().any(file_element_unknown),
        ConfigValue::Object(entries) => entries.iter().any(|(_, value)| files_unknown(value)),
        other => other.has_unknown(),
    }
}

/// Whether an element of a list of files or patterns is not known: one of several values that are not all strings, or
/// a value with an unknown part.
fn file_element_unknown(item: &ConfigValue) -> bool {
    match item {
        ConfigValue::OneOf(alternatives) => !alternatives.iter().all(|alternative| alternative.as_str().is_some()),
        other => other.has_unknown(),
    }
}

/// The `resolve.extensions` list of a bundler config: extensions with a leading dot, webpack's `'...'` placeholder and
/// entries that are not extensions left out.
pub(crate) fn extension_inputs(value: &ConfigValue) -> Vec<PluginInput> {
    strings(value)
        .into_iter()
        .filter(|extension| extension.len() > 1 && extension.starts_with('.') && !extension.starts_with(".."))
        .map(|extension| PluginInput::Extension(extension.to_owned()))
        .collect()
}

/// The value at a key path.
pub(crate) fn get_in<'a>(value: &'a ConfigValue, path: &[&str]) -> Option<&'a ConfigValue> {
    path.iter().try_fold(value, |current, key| current.get(key))
}

/// A name written either as a string or as `[name, options]`.
pub(crate) fn name_of(value: &ConfigValue) -> Option<&str> {
    match value {
        ConfigValue::String(s) => Some(s),
        ConfigValue::Array(items) => items.first().and_then(ConfigValue::as_str),
        _ => None,
    }
}

/// The objects of a value that is an object or a (nested) array of objects.
pub(crate) fn objects(value: &ConfigValue) -> Vec<&ConfigValue> {
    match value {
        ConfigValue::Object(_) => vec![value],
        ConfigValue::Array(items) => items.iter().flat_map(objects).collect(),
        _ => Vec::new(),
    }
}

/// Whether a config is unknown, or is a list (of builds) with an unknown element.
pub(crate) fn config_unknown(config: &ConfigValue) -> bool {
    match config {
        ConfigValue::Array(items) => items.iter().any(ConfigValue::is_unknown),
        other => other.is_unknown(),
    }
}

/// Which unknown spreads and merges on the way to a key [`path_unknown`] counts as possibly setting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Spreads {
    /// Every one: the key may come from any unknown value the config spreads or merges.
    Any,
    /// Only those of values the config computes itself ([`OWN_SPREAD`]), leaving aside what an imported base config
    /// may set: whether the config computes the key itself.
    Own,
}

/// Whether the value at `path` is not fully known statically: it contains an unknown part (an unknown spread of either
/// kind inside it included), or an object on the way is unknown or has an unknown spread `spreads` counts after the
/// key (or without it) that could provide the key.
pub(crate) fn path_unknown(value: &ConfigValue, path: &[&str], spreads: Spreads) -> bool {
    path_unknown_by(value, path, spreads, ConfigValue::has_unknown)
}

/// Like [`path_unknown`], for a key that names files or patterns: a value at `path` that is one of several known ones is
/// known ([`files_unknown`]), as [`file_strings`] reads each.
pub(crate) fn files_path_unknown(value: &ConfigValue, path: &[&str], spreads: Spreads) -> bool {
    path_unknown_by(value, path, spreads, files_unknown)
}

/// [`path_unknown`] with `leaf` telling whether the value at the end of the path is unknown.
fn path_unknown_by(value: &ConfigValue, path: &[&str], spreads: Spreads, leaf: fn(&ConfigValue) -> bool) -> bool {
    let Some((key, rest)) = path.split_first() else {
        return leaf(value);
    };
    match value {
        ConfigValue::Object(_) => match last_entry(value, key, spreads) {
            Some(Entry::Value(child)) => path_unknown_by(child, rest, spreads, leaf),
            Some(Entry::Spread) => true,
            None => false,
        },
        other => other.is_unknown(),
    }
}

/// What decides `key` of an object, as [`last_entry`] finds it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Entry<'v> {
    /// The value of the key's last entry.
    Value(&'v ConfigValue),
    /// An unknown spread after the last entry of the key, or without one, which may provide or replace it.
    Spread,
}

/// What decides `key` of `object`: its last entry for the key, or an unknown spread that `spreads` counts after it (or
/// without it). `None` when the object has neither, and for a value that is not an object.
pub(crate) fn last_entry<'v>(object: &'v ConfigValue, key: &str, spreads: Spreads) -> Option<Entry<'v>> {
    let counted = |name: &str| match spreads {
        Spreads::Any => ConfigValue::is_spread_key(name),
        Spreads::Own => name == OWN_SPREAD,
    };
    let (name, value) = object
        .as_object()?
        .iter()
        .rev()
        .find(|(name, _)| name == key || counted(name.as_str()))?;
    Some(if name == key {
        Entry::Value(value)
    } else {
        Entry::Spread
    })
}

/// A name a config gives a module or package, as [`reporter_names`] reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Named<'v> {
    /// A name known statically.
    Known(&'v str),
    /// A name that is not known statically.
    Unknown,
}

/// The name of each reporter a `reporter` or `reporters` value lists: a name, or the first element of a `[name,
/// options]` pair, for a list of them or a single one. Every alternative of a value that is one of several
/// ([`ConfigValue::alternatives`]) counts, whether the list, an element or a name is, so that every module a branch
/// names is credited. Anything else (`null`, an options object, a reporter instance) names nothing.
pub(crate) fn reporter_names(value: &ConfigValue) -> Vec<Named<'_>> {
    let mut out = Vec::new();
    for listed in value.alternatives() {
        match listed {
            ConfigValue::Array(items) => {
                for item in items {
                    reporter_name(item, &mut out);
                }
            }
            single => reporter_name(single, &mut out),
        }
    }
    out
}

/// Adds to `out` the names of the reporter `item` of a list names, a name or the first element of a pair.
fn reporter_name<'v>(item: &'v ConfigValue, out: &mut Vec<Named<'v>>) {
    for alternative in item.alternatives() {
        let name = match alternative {
            ConfigValue::Array(pair) => pair.first(),
            other => Some(other),
        };
        for name in name.map(ConfigValue::alternatives).unwrap_or_default() {
            match name {
                ConfigValue::String(name) => out.push(Named::Known(name)),
                unknown if unknown.is_unknown() => out.push(Named::Unknown),
                _ => {}
            }
        }
    }
}

/// Whether the reporters `key` of `object` lists are not all known: a name is not ([`reporter_names`]), or an
/// unknown spread `spreads` counts may set the key.
pub(crate) fn reporters_unknown(object: &ConfigValue, key: &str, spreads: Spreads) -> bool {
    match last_entry(object, key, spreads) {
        Some(Entry::Value(reporters)) => reporter_names(reporters).contains(&Named::Unknown),
        Some(Entry::Spread) => true,
        None => false,
    }
}

/// Whether `key` of `object` may hold anything: it is unknown, an unknown spread may provide or replace it, or it is a
/// list with an unknown element. Unlike [`key_unknown`], what a known element holds is not looked at, so a list of
/// project objects is known when each object is, whatever their other options are.
pub(crate) fn shallow_key_unknown(object: &ConfigValue, key: &str) -> bool {
    match object {
        ConfigValue::Object(entries) => match entries
            .iter()
            .rev()
            .find(|(name, _)| name == key || ConfigValue::is_spread_key(name))
        {
            Some((name, _)) if ConfigValue::is_spread_key(name) => true,
            Some((_, ConfigValue::Array(items))) => items.iter().any(ConfigValue::is_unknown),
            Some((_, value)) => value.is_unknown(),
            None => false,
        },
        other => other.is_unknown(),
    }
}

/// The strings of `key` in `object`, the files or patterns a plugin takes entries from, every alternative of a value
/// that is one of several included ([`file_strings`]). When the key is not fully known (see [`files_key_unknown`]),
/// the files it names are not all known, so [`PluginInput::EntriesIncomplete`] is added to `out`.
pub(crate) fn entry_strings<'v>(object: &'v ConfigValue, key: &str, out: &mut Vec<PluginInput>) -> Vec<&'v str> {
    if files_key_unknown(object, key) {
        out.push(PluginInput::EntriesIncomplete);
    }
    object.get(key).map(file_strings).unwrap_or_default()
}

/// Whether the files or patterns `key` of a config object names are not all known; see [`files_path_unknown`].
pub(crate) fn files_key_unknown(object: &ConfigValue, key: &str) -> bool {
    files_path_unknown(object, &[key], Spreads::Any)
}

/// Whether the value of `key` in a config object is not fully known statically; see [`path_unknown`].
pub(crate) fn key_unknown(object: &ConfigValue, key: &str) -> bool {
    path_unknown(object, &[key], Spreads::Any)
}

/// The extensions of the source files a convention scope matches.
pub(crate) const SCOPE_EXTENSIONS: &str = "{js,jsx,ts,tsx,mjs,cjs,mts,cts,vue,svelte,astro,mdx}";

/// The scope of an incomplete config that can only hide entries where the tool looks by convention: the plugin's
/// default entry globs and `conventions` (relative to the config's directory, `{ext}` standing for
/// [`SCOPE_EXTENSIONS`]).
pub(crate) fn convention_scope(spec: &PluginSpec, config: &ConfigAst<'_>, conventions: &[&str]) -> IncompleteScope {
    let mut globs: Vec<String> = Vec::new();
    let spec_globs = spec.entry.iter().chain(spec.production).map(|glob| (*glob).to_owned());
    let conventions = conventions
        .iter()
        .map(|glob| join_rel(config.base, &glob.replace("{ext}", SCOPE_EXTENSIONS)));
    for glob in spec_globs.chain(conventions) {
        if !globs.contains(&glob) {
            globs.push(glob);
        }
    }
    IncompleteScope::Within(globs)
}

/// The values of an object, the elements of an array or the value itself.
pub(crate) fn values_of(value: &ConfigValue) -> Vec<&ConfigValue> {
    match value {
        ConfigValue::Object(entries) => entries.iter().map(|(_, v)| v).collect(),
        ConfigValue::Array(items) => items.iter().collect(),
        other => vec![other],
    }
}

/// The scripts an HTML file loads, as production entries.
pub(crate) fn html_entries(cx: &PluginContext<'_>, base: &str, file: &str) -> Vec<PluginInput> {
    html_entries_at(cx, base, &join_rel(base, file))
}

/// The scripts the HTML file at `html_path` (relative to the workspace) loads, with `/`-rooted sources resolved
/// against `base`, the directory the server serves from (Vite's `root`).
pub(crate) fn html_entries_at(cx: &PluginContext<'_>, base: &str, html_path: &str) -> Vec<PluginInput> {
    let Ok(html) = cx.fs.read_to_string(&cx.dir.join(html_path)) else {
        return Vec::new();
    };
    let html_dir = html_path.rsplit_once('/').map_or("", |(dir, _)| dir);
    html_script_sources(&html)
        .into_iter()
        .filter_map(|src| local_script_path(&src, base, html_dir))
        .map(PluginInput::ProductionEntry)
        .collect()
}

/// The path, relative to the workspace, of the local script `src` loaded by the HTML file in `html_dir`. `base` is the
/// directory a root-relative `src` starts at. `None` for remote URLs and template placeholders (`<% … %>`);
/// `%PUBLIC_URL%` is the `public` directory.
pub(crate) fn local_script_path(src: &str, base: &str, html_dir: &str) -> Option<String> {
    if src.contains("://") || src.starts_with("//") || src.contains("<%") {
        return None;
    }
    if let Some(rest) = src.strip_prefix("%PUBLIC_URL%") {
        return Some(join_rel(base, &format!("public/{}", rest.trim_start_matches('/'))));
    }
    Some(match src.strip_prefix('/') {
        Some(root_relative) => join_rel(base, root_relative),
        None => join_rel(html_dir, src),
    })
}

/// The `src` of every `<script>` tag of an HTML document.
pub(crate) fn html_script_sources(html: &str) -> Vec<String> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(found) = lower[pos..].find("<script") {
        let start = pos + found;
        let Some(len) = lower[start..].find('>') else { break };
        let tag = &html[start..start + len];
        if let Some(src) = attribute(tag, &lower[start..start + len], "src") {
            out.push(src);
        }
        pos = start + len + 1;
    }
    out
}

fn attribute(tag: &str, lower_tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    let mut from = 0;
    while let Some(found) = lower_tag[from..].find(&needle) {
        let at = from + found;
        let preceded_by_space = lower_tag[..at].chars().next_back().is_some_and(char::is_whitespace);
        if preceded_by_space {
            let rest = &tag[at + needle.len()..];
            let quote = rest.chars().next().filter(|c| matches!(c, '"' | '\''))?;
            let inner = &rest[1..];
            return inner.find(quote).map(|end| inner[..end].to_string());
        }
        from = at + needle.len();
    }
    None
}

/// The module aliases of a bundler's `resolve.alias`: an object of `find: target` or an array of
/// `{ find, replacement }`. A target that is a package rather than a path (`react: 'preact/compat'`, or the
/// `import.meta.resolve('pkg')` the evaluator reads as `'pkg'`) is an alias without a target, as the imports it matches
/// reach that package and not the one they name, and the package is a dependency of the config.
pub(crate) fn alias_inputs(alias: &ConfigValue, config: &ConfigAst<'_>) -> Vec<PluginInput> {
    let pairs: Vec<(AliasFind<'_>, &ConfigValue)> = match alias {
        ConfigValue::Object(entries) => entries
            .iter()
            .filter(|(key, _)| !ConfigValue::is_spread_key(key))
            .map(|(key, value)| (AliasFind::Text(key.as_str()), value))
            .collect(),
        ConfigValue::Array(items) => items
            .iter()
            .filter_map(|item| {
                let find = match item.get("find")? {
                    ConfigValue::String(text) => AliasFind::Text(text),
                    ConfigValue::Regex { pattern, .. } => AliasFind::Pattern(pattern),
                    _ => return None,
                };
                Some((find, item.get("replacement")?))
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut out = Vec::new();
    for (find, replacement) in pairs {
        let find = match find {
            AliasFind::Text(text) => text.trim_end_matches('$').trim_end_matches('/').to_owned(),
            AliasFind::Pattern(pattern) => {
                out.extend(pattern_alias(pattern, replacement, config));
                continue;
            }
        };
        if find.is_empty() {
            continue;
        }
        let target = replacement.as_str().and_then(|path| alias_target(path, config));
        if target.is_none()
            && let Some(package) = replacement.as_str().and_then(package_name)
        {
            out.push(PluginInput::Dependency(package));
        }
        out.push(PluginInput::Alias { find, target });
    }
    out
}

/// The `find` of an alias entry.
enum AliasFind<'a> {
    /// A prefix or exact name.
    Text(&'a str),
    /// A regular expression, without the slashes and flags.
    Pattern(&'a str),
}

/// An alias from a regular expression `find`. `^@/(.*)$` with the replacement `/src/$1` is the prefix alias `@` to
/// `src`; any other pattern with a literal prefix is an alias whose target is computed, so the imports it matches are
/// not reported. A pattern without a literal prefix cannot be named and yields nothing.
pub(crate) fn pattern_alias(pattern: &str, replacement: &ConfigValue, config: &ConfigAst<'_>) -> Option<PluginInput> {
    let (prefix, rest) = regex_literal_prefix(pattern.strip_prefix('^')?);
    let find = prefix.trim_end_matches('/');
    if find.is_empty() {
        return None;
    }
    let forwards_rest = matches!(rest, "(.*)$" | "(.+)$" | "(.*)" | "(.+)");
    let target = match replacement {
        ConfigValue::String(path) if forwards_rest => {
            let path = path.strip_suffix("$1")?;
            alias_target(path.trim_end_matches('/'), config)
        }
        ConfigValue::String(path) if rest == "$" => alias_target(path, config),
        _ => None,
    };
    Some(PluginInput::Alias {
        find: find.to_owned(),
        target,
    })
}

/// The literal text a regular expression starts with, with escaped punctuation unescaped, and what follows it.
fn regex_literal_prefix(pattern: &str) -> (String, &str) {
    let mut literal = String::new();
    let mut chars = pattern.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => match chars.peek().copied() {
                Some((_, escaped)) if escaped.is_ascii_punctuation() => {
                    literal.push(escaped);
                    chars.next();
                }
                _ => return (literal, &pattern[at..]),
            },
            c if c.is_ascii_alphanumeric() || matches!(c, '@' | '_' | '-' | '/' | '~' | '#') => literal.push(c),
            _ => return (literal, &pattern[at..]),
        }
    }
    (literal, "")
}

/// A path written as an alias target, relative to the workspace. `/src` is relative to the project root.
fn alias_target(target: &str, config: &ConfigAst<'_>) -> Option<String> {
    let target = target.replace('\\', "/");
    let target = target.trim_end_matches('/');
    if is_relative(target) {
        Some(config.join(target))
    } else {
        target.strip_prefix('/').map(|rooted| config.join(rooted))
    }
}

/// `@app/*` and `src/lib/*` without the wildcard.
pub(crate) fn strip_wildcard(pattern: &str) -> &str {
    pattern
        .strip_suffix("/*")
        .or_else(|| pattern.strip_suffix('*'))
        .unwrap_or(pattern)
        .trim_end_matches('/')
}

/// Every string that `key` holds in any object of the value, however deep: a string, the strings of an array or the
/// strings of an object's values.
pub(crate) fn property_values<'a>(value: &'a ConfigValue, key: &str) -> Vec<&'a str> {
    let mut found = Vec::new();
    collect_property(value, key, &mut found);
    found
}

fn collect_property<'a>(value: &'a ConfigValue, key: &str, found: &mut Vec<&'a str>) {
    match value {
        ConfigValue::Object(entries) => {
            for (name, child) in entries {
                if name == key {
                    found.extend(leaf_strings(child));
                }
                collect_property(child, key, found);
            }
        }
        ConfigValue::Array(items) => {
            for item in items {
                collect_property(item, key, found);
            }
        }
        _ => {}
    }
}

/// Every string of a value that is a string, an array or an object of them.
pub(crate) fn leaf_strings(value: &ConfigValue) -> Vec<&str> {
    match value {
        ConfigValue::String(s) => vec![s.as_str()],
        ConfigValue::Array(items) => items.iter().flat_map(leaf_strings).collect(),
        ConfigValue::Object(entries) => entries.iter().flat_map(|(_, v)| leaf_strings(v)).collect(),
        _ => Vec::new(),
    }
}

/// Production entries for globs or paths written in a config, relative to the config file.
pub(crate) fn production_entries<'a>(base: &str, patterns: impl IntoIterator<Item = &'a str>) -> Vec<PluginInput> {
    patterns
        .into_iter()
        .filter_map(|pattern| entry_glob(base, pattern))
        .map(PluginInput::ProductionEntry)
        .collect()
}

/// The JavaScript and TypeScript source files of the workspace whose text contains `needle`, as their path relative to
/// the workspace and their text. Declaration files are left out.
pub(crate) fn sources_containing(cx: &PluginContext<'_>, needle: &str) -> Vec<(String, std::sync::Arc<str>)> {
    let finder = memchr::memmem::Finder::new(needle.as_bytes());
    source_files(cx)
        .filter_map(|source| {
            let text = cx.fs.read_to_string(&cx.dir.join(&source)).ok()?;
            finder.find(text.as_bytes()).is_some().then_some((source, text))
        })
        .collect()
}

/// The JavaScript and TypeScript source files of the workspace's own whose text `keep` accepts, as their path
/// relative to the workspace and their text. Declaration files are left out, and so are the files below a directory
/// with a `package.json` of its own, such as a member of a workspace root: they belong to that package.
pub(crate) fn own_sources_where(
    cx: &PluginContext<'_>,
    keep: impl Fn(&str) -> bool,
) -> Vec<(String, std::sync::Arc<str>)> {
    let mut nested = HashMap::default();
    source_files(cx)
        .filter(|source| !in_nested_package(cx, source, &mut nested))
        .filter_map(|source| {
            let text = cx.fs.read_to_string(&cx.dir.join(&source)).ok()?;
            keep(&text).then_some((source, text))
        })
        .collect()
}

/// The JavaScript and TypeScript files below the workspace directory, declaration files left out.
fn source_files(cx: &PluginContext<'_>) -> impl Iterator<Item = String> {
    let patterns = ["**/*.{ts,mts,cts,tsx,js,mjs,cjs,jsx}".to_owned()];
    crate::files::find_files(cx.fs, cx.dir, &patterns)
        .into_iter()
        .filter(|source| !is_declaration_file(source))
}

/// Whether `path`, relative to the workspace directory, lies below a directory of the workspace that holds a
/// `package.json`. `nested` remembers the answer for each directory already looked at.
fn in_nested_package(cx: &PluginContext<'_>, path: &str, nested: &mut HashMap<String, bool>) -> bool {
    let Some((parent, _)) = path.rsplit_once('/') else {
        return false;
    };
    if let Some(&known) = nested.get(parent) {
        return known;
    }
    let answer = cx.fs.is_file(&cx.dir.join(parent).join("package.json")) || in_nested_package(cx, parent, nested);
    nested.insert(parent.to_owned(), answer);
    answer
}

/// The workspace's `package.json`, parsed; `None` when it is missing or not JSON.
pub(crate) fn manifest(cx: &PluginContext<'_>) -> Option<ConfigValue> {
    let path = cx.dir.join("package.json");
    let source = cx.fs.read_to_string(&path).ok()?;
    crate::config_load::load_config(&path, &source)
}

/// The directories the `key` option of the last call of a Vite plugin imported from one of `modules` lists in the
/// config's source: a string, or an array of strings and `{ dir }` objects. `None` when no call sets it.
pub(crate) fn vite_plugin_dirs(config: &ConfigAst<'_>, modules: &[&str], key: &str) -> Option<Vec<String>> {
    let mut dirs = None;
    for call in skif_js_syntax::extract_imported_calls(config.source, config.path, modules, &[]) {
        let Some(value) = call.arguments.first().and_then(|options| options.get(key)) else {
            continue;
        };
        let collected: Vec<String> = match value {
            ConfigValue::String(dir) => vec![dir.clone()],
            ConfigValue::Array(items) => items
                .iter()
                .filter_map(|item| item.as_str().or_else(|| item.get("dir").and_then(ConfigValue::as_str)))
                .map(str::to_owned)
                .collect(),
            _ => Vec::new(),
        };
        if !collected.is_empty() {
            dirs = Some(collected);
        }
    }
    dirs
}

/// The `root` of a Vite config's exported value, relative to the workspace; the workspace itself by default.
pub(crate) fn vite_root(config: &ConfigAst<'_>) -> String {
    config
        .value
        .get("root")
        .and_then(ConfigValue::as_str)
        .map_or_else(String::new, |root| join_rel("", root))
}

/// The directory of a path relative to the workspace, `""` for a file at the top.
pub(crate) fn parent_dir(relative: &str) -> &str {
    relative.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// The end of the string literal that starts at `start` (the index of its closing quote).
fn string_end(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() && bytes[i] != quote {
        i += if bytes[i] == b'\\' { 2 } else { 1 };
    }
    i.min(bytes.len().saturating_sub(1))
}

/// The string literal value of the property `key` of the object literal that `object` starts with, when it is written
/// as a literal. Only the properties of the object itself count, not those of objects nested in it; the text is
/// scanned, not parsed.
pub(crate) fn literal_property<'t>(object: &'t str, key: &str) -> Option<&'t str> {
    let bytes = object.as_bytes();
    let mut depth = 0_i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' | b'[' | b'(' => depth += 1,
            b'}' | b']' | b')' => {
                depth -= 1;
                if depth <= 0 {
                    return None;
                }
            }
            b'\'' | b'"' | b'`' => {
                let end = string_end(bytes, i);
                if depth == 1
                    && object.get(i + 1..end) == Some(key)
                    && let Some(value) = literal_after(object, end + 1)
                {
                    return Some(value);
                }
                i = end;
            }
            c if c.is_ascii_alphabetic() || c == b'_' || c == b'$' => {
                let end = (i..bytes.len())
                    .find(|&j| !(bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'$'))
                    .unwrap_or(bytes.len());
                if depth == 1
                    && object[i..end] == *key
                    && let Some(value) = literal_after(object, end)
                {
                    return Some(value);
                }
                i = end - 1;
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The string literal that follows `:` at `from`.
fn literal_after(text: &str, from: usize) -> Option<&str> {
    let rest = text.get(from..)?.trim_start().strip_prefix(':')?.trim_start();
    let quote = rest.chars().next().filter(|c| matches!(*c, '\'' | '"' | '`'))?;
    let body = &rest[1..];
    Some(&body[..body.find(quote)?])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extglob_groups_become_brace_alternations() {
        let cases = [
            ("src/**/*.(ts|tsx)", "src/**/*.{ts,tsx}"),
            ("src/**/*.@(ts|tsx)", "src/**/*.{ts,tsx}"),
            ("src/*.?(m)js", "src/*.{m,}js"),
            ("src/*.?(a|b)", "src/*.{a,b,}"),
            ("src/*.*(a|b)", "src/*.{a,b,}"),
            ("src/*.+(a|b)", "src/*.{a,b}"),
            ("src/!(gen|tmp)/*.ts", "src/*/*.ts"),
            ("src/@(a|(b|c))/x", "src/{a,{b,c}}/x"),
            ("app/(group)/page.tsx", "app/(group)/page.tsx"),
            ("app/[(]g[)]/x", "app/[(]g[)]/x"),
            (r"app/\(a|b\)", r"app/\(a|b\)"),
            ("src/(a|b", "src/(a|b"),
        ];
        for (pattern, expected) in cases {
            assert_eq!(extglob_to_braces(pattern), expected, "{pattern}");
        }
    }

    #[test]
    fn regex_prefixes_are_literal_text_up_to_the_first_metacharacter() {
        assert_eq!(regex_literal_prefix("@\\/(.*)$"), ("@/".to_owned(), "(.*)$"));
        assert_eq!(regex_literal_prefix("~/lib/(.+)"), ("~/lib/".to_owned(), "(.+)"));
        assert_eq!(regex_literal_prefix("(a|b)"), (String::new(), "(a|b)"));
        assert_eq!(regex_literal_prefix("plain"), ("plain".to_owned(), ""));
    }

    #[test]
    fn paths_below_node_modules_lose_the_prefix() {
        assert_eq!(below_node_modules("node_modules/pkg/cli.js"), Some("pkg/cli.js"));
        assert_eq!(below_node_modules("./node_modules/@scope/pkg"), Some("@scope/pkg"));
        assert_eq!(below_node_modules("tools/node_modules/pkg"), Some("pkg"));
        assert_eq!(below_node_modules("my_node_modules/pkg"), None);
        assert_eq!(below_node_modules("src/pkg"), None);
    }

    #[test]
    fn entry_strings_take_every_alternative_of_a_value_that_is_one_of_several_known_ones() {
        let s = |text: &str| ConfigValue::String(text.to_owned());
        let one_of = ConfigValue::OneOf;
        let cases = [
            (
                ConfigValue::Array(vec![s("./a.ts"), s("./b.ts")]),
                vec!["./a.ts", "./b.ts"],
                false,
            ),
            (
                ConfigValue::Array(vec![one_of(vec![s("./ui/setup.ts"), s("./setup.ts")]), s("./c.ts")]),
                vec!["./ui/setup.ts", "./setup.ts", "./c.ts"],
                false,
            ),
            (one_of(vec![s("./a.ts"), s("./b.ts")]), vec!["./a.ts", "./b.ts"], false),
            (
                one_of(vec![
                    ConfigValue::Array(vec![s("./a.ts")]),
                    ConfigValue::Array(vec![s("./b.ts"), s("./a.ts")]),
                ]),
                vec!["./a.ts", "./b.ts"],
                false,
            ),
            (
                ConfigValue::Array(vec![one_of(vec![s("./a.ts"), ConfigValue::Unknown])]),
                vec!["./a.ts"],
                true,
            ),
            (
                one_of(vec![
                    s("./a.ts"),
                    ConfigValue::Array(vec![s("./b.ts"), ConfigValue::Unknown]),
                ]),
                vec!["./a.ts", "./b.ts"],
                true,
            ),
            (
                ConfigValue::Array(vec![one_of(vec![s("./a.ts"), ConfigValue::Null])]),
                vec!["./a.ts"],
                true,
            ),
            (
                ConfigValue::Array(vec![s("./a.ts"), ConfigValue::Unknown]),
                vec!["./a.ts"],
                true,
            ),
            (ConfigValue::Unknown, vec![], true),
        ];
        for (listed, expected, incomplete) in cases {
            let object = ConfigValue::Object(vec![("setupFiles".to_owned(), listed.clone())]);
            let mut out = Vec::new();
            assert_eq!(entry_strings(&object, "setupFiles", &mut out), expected, "{listed:?}");
            assert_eq!(out.contains(&PluginInput::EntriesIncomplete), incomplete, "{listed:?}");
        }
    }

    #[test]
    fn wildcards_are_stripped() {
        assert_eq!(strip_wildcard("@app/*"), "@app");
        assert_eq!(strip_wildcard("src/lib/*"), "src/lib");
        assert_eq!(strip_wildcard("exact"), "exact");
        assert_eq!(strip_wildcard("*"), "");
    }
}
