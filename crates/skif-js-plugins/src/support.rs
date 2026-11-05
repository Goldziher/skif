//! Small helpers shared by the plugins: package and path classification, naming conventions and
//! typed access to [`ConfigValue`]s.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, PluginContext, PluginInput};

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

/// Rewrites the extglob forms `@(a|b)` and `?(a|b)` to the brace syntax `{a,b}` and `{a,b,}`.
pub(crate) fn extglob_to_braces(pattern: &str) -> String {
    #[derive(Clone, Copy, PartialEq)]
    enum Group {
        One,
        Optional,
        Plain,
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len());
    let mut stack: Vec<Group> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '@' if next == Some('(') => {
                stack.push(Group::One);
                out.push('{');
                i += 1;
            }
            '?' if next == Some('(') => {
                stack.push(Group::Optional);
                out.push('{');
                i += 1;
            }
            '(' => {
                stack.push(Group::Plain);
                out.push('(');
            }
            ')' => match stack.pop() {
                Some(Group::One) => out.push('}'),
                Some(Group::Optional) => out.push_str(",}"),
                Some(Group::Plain) | None => out.push(')'),
            },
            '|' if matches!(stack.last(), Some(Group::One | Group::Optional)) => out.push(','),
            other => out.push(other),
        }
        i += 1;
    }
    out
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
        ConfigValue::Unknown => true,
        ConfigValue::Array(items) => items.iter().any(|item| matches!(item, ConfigValue::Unknown)),
        _ => false,
    }
}

/// Whether the value at `path` is not fully known statically: it contains an unknown part, or an object on the way
/// is unknown or has an unknown spread that could provide the key.
pub(crate) fn path_unknown(value: &ConfigValue, path: &[&str]) -> bool {
    let Some((key, rest)) = path.split_first() else {
        return value.has_unknown();
    };
    match value {
        ConfigValue::Unknown => true,
        ConfigValue::Object(_) => match value.get(key) {
            Some(child) => path_unknown(child, rest),
            None => value.get("...").is_some(),
        },
        _ => false,
    }
}

/// Whether the value of `key` in a config object is not fully known statically; see [`path_unknown`].
pub(crate) fn key_unknown(object: &ConfigValue, key: &str) -> bool {
    path_unknown(object, &[key])
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
    let html_path = join_rel(base, file);
    let Ok(html) = cx.fs.read_to_string(&cx.dir.join(&html_path)) else {
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
/// `{ find, replacement }`. Targets that are not paths (a package standing in for another) are not aliases.
pub(crate) fn alias_inputs(alias: &ConfigValue, config: &ConfigAst<'_>) -> Vec<PluginInput> {
    let pairs: Vec<(AliasFind<'_>, &ConfigValue)> = match alias {
        ConfigValue::Object(entries) => entries
            .iter()
            .filter(|(key, _)| key != "...")
            .map(|(key, value)| (AliasFind::Text(key.as_str()), value))
            .collect(),
        ConfigValue::Array(items) => items
            .iter()
            .filter_map(|item| {
                let find = match item.get("find")? {
                    ConfigValue::String(text) => AliasFind::Text(text),
                    ConfigValue::Regex(pattern) => AliasFind::Pattern(pattern),
                    _ => return None,
                };
                Some((find, item.get("replacement")?))
            })
            .collect(),
        _ => Vec::new(),
    };
    pairs
        .into_iter()
        .filter_map(|(find, replacement)| {
            let (find, replacement) = match find {
                AliasFind::Text(text) => (text.trim_end_matches('$').trim_end_matches('/').to_owned(), replacement),
                AliasFind::Pattern(pattern) => return pattern_alias(pattern, replacement, config),
            };
            if find.is_empty() {
                return None;
            }
            let target = match replacement {
                ConfigValue::String(path) => Some(alias_target(path, config)?),
                _ => None,
            };
            Some(PluginInput::Alias { find, target })
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regex_prefixes_are_literal_text_up_to_the_first_metacharacter() {
        assert_eq!(regex_literal_prefix("@\\/(.*)$"), ("@/".to_owned(), "(.*)$"));
        assert_eq!(regex_literal_prefix("~/lib/(.+)"), ("~/lib/".to_owned(), "(.+)"));
        assert_eq!(regex_literal_prefix("(a|b)"), (String::new(), "(a|b)"));
        assert_eq!(regex_literal_prefix("plain"), ("plain".to_owned(), ""));
    }

    #[test]
    fn wildcards_are_stripped() {
        assert_eq!(strip_wildcard("@app/*"), "@app");
        assert_eq!(strip_wildcard("src/lib/*"), "src/lib");
        assert_eq!(strip_wildcard("exact"), "exact");
        assert_eq!(strip_wildcard("*"), "");
    }
}
