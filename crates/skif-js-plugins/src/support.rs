//! Small helpers shared by the plugins: package and path classification, naming conventions and
//! typed access to [`ConfigValue`]s.

use skif_js_syntax::ConfigValue;

use crate::plugin::PluginInput;

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

/// The entry for a glob or path written in a config. Negated patterns yield nothing.
pub(crate) fn entry_glob(base: &str, pattern: &str) -> Option<String> {
    let pattern = pattern.trim();
    if pattern.is_empty() || pattern.starts_with('!') {
        return None;
    }
    let pattern = pattern.strip_prefix("<rootDir>").map_or(pattern, |r| r.trim_start_matches('/'));
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

/// The values of an object, the elements of an array or the value itself.
pub(crate) fn values_of(value: &ConfigValue) -> Vec<&ConfigValue> {
    match value {
        ConfigValue::Object(entries) => entries.iter().map(|(_, v)| v).collect(),
        ConfigValue::Array(items) => items.iter().collect(),
        other => vec![other],
    }
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
