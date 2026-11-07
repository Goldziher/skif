//! Varlock: the `@plugin(…)` and `@import(…)` decorators in the root comments of the `.env` files under
//! `package.json#varlock.loadPath` (the workspace by default), and of the files a script passes with `--path`: a local
//! plugin is a production entry, a plugin package is used, an import is scanned too. The command of
//! `varlock run -- <command>` is a script.

use std::collections::BTreeSet;

use crate::files::find_files;
use crate::plugin::{Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::script_config::{commands_after_separator, flag_values, workspace_path};
use crate::support::{get_in, is_relative, join_rel, manifest, package_name, parent_dir, strings};

/// The `varlock` plugin.
pub struct Varlock;

static SPEC: PluginSpec = PluginSpec {
    id: "varlock",
    enablers: &[Enabler::Dependency("varlock")],
    binaries: &["varlock"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Varlock {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut paths: Vec<String> = manifest(cx)
            .as_ref()
            .and_then(|manifest| get_in(manifest, &["varlock", "loadPath"]))
            .map(|value| strings(value).into_iter().map(str::to_owned).collect())
            .unwrap_or_default();
        if paths.is_empty() {
            paths.push(".".to_owned());
        }
        let mut out = Vec::new();
        for script in cx.scripts.values() {
            paths.extend(flag_values(script, SPEC.binaries, &["-p", "--path"]));
            out.extend(
                commands_after_separator(script, SPEC.binaries, "run")
                    .into_iter()
                    .map(PluginInput::Script),
            );
        }
        let mut queue: Vec<String> = paths
            .iter()
            .filter_map(|path| workspace_path(path).or_else(|| (path.trim() == ".").then(String::new)))
            .flat_map(|path| env_files(cx, &path))
            .collect();
        let mut visited = BTreeSet::new();
        while let Some(file) = queue.pop() {
            if !visited.insert(file.clone()) {
                continue;
            }
            let Ok(source) = cx.fs.read_to_string(&cx.dir.join(&file)) else {
                continue;
            };
            let parsed = parse(&source);
            if parsed.disabled {
                continue;
            }
            let dir = parent_dir(&file);
            for directive in parsed.directives {
                let descriptor = directive.descriptor.as_str();
                if directive.disabled || descriptor.contains('$') || is_external_url(descriptor) {
                    continue;
                }
                let local = is_relative(descriptor) || descriptor.starts_with(['/', '~']);
                if directive.import {
                    if is_relative(descriptor) {
                        let path = join_rel(dir, descriptor);
                        if !path.starts_with("..") {
                            queue.extend(env_files(cx, &path));
                        }
                    }
                } else if is_relative(descriptor) {
                    out.push(PluginInput::ProductionEntry(join_rel(dir, descriptor)));
                } else if !local && let Some(name) = package_name(descriptor) {
                    out.push(PluginInput::Dependency(name));
                }
            }
        }
        out
    }
}

/// The `.env` files a load path names: the file itself, or the `.env` and `.env.*` files of a directory.
fn env_files(cx: &PluginContext<'_>, path: &str) -> Vec<String> {
    let absolute = cx.dir.join(path);
    if cx.fs.is_dir(&absolute) {
        find_files(cx.fs, cx.dir, &[join_rel(path, ".env"), join_rel(path, ".env.*")])
    } else if cx.fs.is_file(&absolute) {
        vec![path.to_owned()]
    } else {
        Vec::new()
    }
}

fn is_external_url(path: &str) -> bool {
    path.starts_with("//")
        || path.split_once(':').is_some_and(|(scheme, _)| {
            scheme.len() > 1
                && scheme.starts_with(|c: char| c.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        })
}

/// A `@plugin(…)` or `@import(…)` decorator.
#[derive(Debug, PartialEq, Eq)]
struct Directive {
    import: bool,
    descriptor: String,
    /// `enabled=false`.
    disabled: bool,
}

#[derive(Debug, Default)]
struct Parsed {
    directives: Vec<Directive>,
    /// `@disable` or `@disable=true`.
    disabled: bool,
}

/// The root comments of an env file: the comment blocks before the first item, without the block attached to it and
/// without divider lines (`# ---`).
fn root_comments(source: &str) -> String {
    let mut comments: Vec<&str> = Vec::new();
    let mut block: Vec<&str> = Vec::new();
    let mut has_item = false;
    for line in source.trim_start_matches('\u{feff}').lines() {
        if line.trim().is_empty() {
            comments.append(&mut block);
            continue;
        }
        let Some(content) = line.trim_start().strip_prefix('#') else {
            has_item = true;
            break;
        };
        let content = content.trim_start();
        let divider = content.len() >= 3 && content.chars().take(3).all(|c| matches!(c, '-' | '=' | '*' | '#'));
        if divider {
            comments.append(&mut block);
        } else {
            block.push(content);
        }
    }
    if !has_item {
        comments.append(&mut block);
    }
    comments.join("\n")
}

/// The text of the quoted string that starts at `start`, and the index after its closing quote.
fn read_quoted(chars: &[char], start: usize) -> Option<(String, usize)> {
    let quote = chars[start];
    let mut value = String::new();
    let mut index = start + 1;
    while index < chars.len() {
        let c = chars[index];
        if c == quote {
            return Some((value, index + 1));
        }
        if c == '\\' && chars.get(index + 1) == Some(&quote) {
            value.push(quote);
            index += 1;
        } else {
            value.push(c);
        }
        index += 1;
    }
    None
}

fn closing(c: char) -> Option<char> {
    match c {
        '(' => Some(')'),
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

/// The comma-separated arguments of the bracket group opened at `start`, and the index after it.
fn read_arguments(chars: &[char], start: usize) -> Option<(Vec<String>, usize)> {
    let mut args = Vec::new();
    let mut stack = vec![closing(chars[start])?];
    let mut argument = String::new();
    let mut index = start + 1;
    while index < chars.len() {
        let c = chars[index];
        if matches!(c, '"' | '\'' | '`') {
            let (_, end) = read_quoted(chars, index)?;
            argument.extend(&chars[index..end]);
            index = end;
            continue;
        }
        if c == '#' {
            index += chars[index..].iter().position(|&c| c == '\n')?;
            argument.push(' ');
        } else if let Some(close) = closing(c) {
            stack.push(close);
            argument.push(c);
        } else if Some(&c) == stack.last() {
            stack.pop();
            if stack.is_empty() {
                args.push(argument.trim().to_owned());
                return Some((args, index + 1));
            }
            argument.push(c);
        } else if c == ',' && stack.len() == 1 {
            args.push(argument.trim().to_owned());
            argument.clear();
        } else {
            argument.push(c);
        }
        index += 1;
    }
    None
}

/// The end of a `=value`.
fn read_value_end(chars: &[char], start: usize) -> usize {
    let mut index = start;
    while index < chars.len() && !chars[index].is_whitespace() && chars[index] != '#' {
        let c = chars[index];
        if matches!(c, '"' | '\'' | '`') {
            match read_quoted(chars, index) {
                Some((_, end)) => index = end,
                None => return chars.len(),
            }
        } else if closing(c).is_some() {
            match read_arguments(chars, index) {
                Some((_, end)) => index = end,
                None => return chars.len(),
            }
        } else {
            index += 1;
        }
    }
    index
}

/// A literal string argument: quoted, or a bare word that is not a boolean, number, reference or function call.
fn static_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let chars: Vec<char> = trimmed.chars().collect();
    if let Some(&first) = chars.first()
        && matches!(first, '"' | '\'' | '`')
    {
        let (text, end) = read_quoted(&chars, 0)?;
        return (end == chars.len()).then_some(text);
    }
    let call = trimmed
        .find('(')
        .is_some_and(|open| open > 0 && trimmed[..open].chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    if trimmed.is_empty()
        || trimmed.contains(|c: char| c.is_whitespace() || c == ',' || c == ')')
        || trimmed.contains('$')
        || call
        || ["true", "false", "undefined"]
            .iter()
            .any(|word| trimmed.eq_ignore_ascii_case(word))
        || trimmed.parse::<f64>().is_ok_and(f64::is_finite)
    {
        return None;
    }
    Some(trimmed.to_owned())
}

fn parse(source: &str) -> Parsed {
    let text = root_comments(source);
    let chars: Vec<char> = text.chars().collect();
    let mut parsed = Parsed::default();
    let line_end = |from: usize| {
        chars[from.min(chars.len())..]
            .iter()
            .position(|&c| c == '\n')
            .map(|at| from + at)
    };
    let mut index = 0;
    while index < chars.len() {
        while matches!(chars.get(index), Some(' ' | '\t')) {
            index += 1;
        }
        if chars.get(index) != Some(&'@') {
            index = line_end(index).map_or(chars.len(), |end| end + 1);
            continue;
        }
        while index < chars.len() && chars[index] != '\n' && chars[index] != '#' {
            if chars[index] != '@' || !chars.get(index + 1).is_some_and(char::is_ascii_alphabetic) {
                index += 1;
                continue;
            }
            let mut name_end = index + 2;
            while chars
                .get(name_end)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
            {
                name_end += 1;
            }
            let name: String = chars[index + 1..name_end].iter().collect();
            let mut cursor = name_end;
            while matches!(chars.get(cursor), Some(' ' | '\t')) {
                cursor += 1;
            }
            match chars.get(cursor) {
                Some('(') => {
                    let Some((args, end)) = read_arguments(&chars, cursor) else {
                        break;
                    };
                    if (name == "plugin" || name == "import")
                        && let Some(descriptor) = args.first().and_then(|arg| static_string(arg))
                    {
                        let disabled = args.iter().skip(1).any(|arg| {
                            arg.split_once('=')
                                .is_some_and(|(key, value)| key.trim() == "enabled" && value.trim() == "false")
                        });
                        parsed.directives.push(Directive {
                            import: name == "import",
                            descriptor,
                            disabled,
                        });
                    }
                    index = end;
                }
                Some('=') => {
                    let end = read_value_end(&chars, cursor + 1);
                    if name == "disable" {
                        let value: String = chars[cursor + 1..end].iter().collect();
                        parsed.disabled |= value.trim() == "true";
                    }
                    index = end;
                }
                _ => {
                    if name == "disable" {
                        parsed.disabled = true;
                    }
                    index = name_end;
                }
            }
        }
        index = line_end(index).map_or(chars.len(), |end| end + 1);
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decorators_of_the_root_comments_are_read() {
        let source = "# @plugin(@varlock/1password-plugin) @import(./.env.shared, enabled=false)\n# @plugin(\"./plugins/x.ts\")\n# ---\n\n# @plugin(not-root)\nITEM=1\n";
        let parsed = parse(source);
        assert!(!parsed.disabled);
        let descriptors: Vec<&str> = parsed.directives.iter().map(|d| d.descriptor.as_str()).collect();
        assert_eq!(
            descriptors,
            ["@varlock/1password-plugin", "./.env.shared", "./plugins/x.ts"]
        );
        assert!(parsed.directives[1].disabled);
        assert!(parse("# @disable\n").disabled);
        assert_eq!(parse("# @plugin(\"a\") # comment\n").directives.len(), 1);
    }
}
