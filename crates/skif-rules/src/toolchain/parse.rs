//! Reading one toolchain file: the commands it runs and the Node and package-manager versions it pins.
//!
//! Dockerfiles are split into instructions (continuation lines joined, comment lines skipped), workflows are read
//! line by line for their steps (`uses:` with its `with:` inputs, `run:` scalars and block scalars), shell scripts are
//! commands throughout and version files are read by their own syntax. Bazel files are read as Starlark for the
//! version arguments of the `rules_js` and `rules_nodejs` calls. Every span is a byte range of the file.

use std::ops::Range;

use skif_config::PackageManager;
use skif_core::shell::{self, Word};

use super::commands::{self, Corepack};
use super::options::ToolchainKind;
use super::suppress::{self, Directive};
use crate::starlark::{self, Kind};

/// What a pin pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tool {
    /// The Node.js runtime.
    Node,
    /// A package manager.
    Manager(PackageManager),
}

/// A version of Node or of a package manager written in a toolchain file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pin {
    pub tool: Tool,
    /// The version or range as written, without quotes.
    pub version: String,
    /// Where it is written.
    pub span: Range<usize>,
    /// What writes it, for messages: `FROM node`, `.nvmrc`, `actions/setup-node node-version`.
    pub label: String,
}

/// A simple command a file runs; word spans are byte ranges of the file.
#[derive(Debug, Clone)]
pub(crate) struct Command {
    pub words: Vec<Word>,
}

impl Command {
    /// From the first through the last word.
    pub fn span(&self) -> Range<usize> {
        let start = self.words.first().map_or(0, |word| word.span.start);
        let end = self.words.last().map_or(start, |word| word.span.end);
        start..end
    }
}

/// Everything the toolchain rules read from one file.
#[derive(Debug, Clone, Default)]
pub(crate) struct Parsed {
    pub commands: Vec<Command>,
    pub pins: Vec<Pin>,
    /// The first `corepack enable` of a file that runs no `corepack prepare`, `use` or `install`.
    pub unpinned_corepack: Option<Range<usize>>,
    pub directives: Vec<Directive>,
    /// The sources of a Dockerfile's `COPY` and `ADD` instructions that copy from the build context (no `--from`),
    /// as written, without quotes.
    pub copied: Vec<String>,
    /// The byte offsets of the `COPY` and `ADD` instructions among them that copy the whole build context (`.` or
    /// `./`).
    pub context_copies: Vec<usize>,
}

/// Reads `source`, a file of `kind` named `name` (its last path segment).
pub(crate) fn parse(kind: ToolchainKind, name: &str, source: &str) -> Parsed {
    let mut parsed = Parsed {
        directives: suppress::directives(source),
        ..Parsed::default()
    };
    match kind {
        ToolchainKind::VersionFiles => version_file(name, source, &mut parsed.pins),
        ToolchainKind::Dockerfiles => dockerfile(source, &mut parsed),
        ToolchainKind::Workflows => workflow(source, &mut parsed),
        ToolchainKind::ShellScripts => shell_commands(source, 0, 0, &mut parsed.commands),
        ToolchainKind::Bazel => bazel(source, &mut parsed.pins),
    }
    parsed.commands.sort_by_key(|command| command.span().start);
    let mut enable = None;
    let mut prepared = false;
    for command in &parsed.commands {
        parsed.pins.extend(commands::command_pins(&command.words));
        match commands::corepack(&command.words) {
            Some(Corepack::Enable) => {
                enable.get_or_insert_with(|| command.span());
            }
            Some(Corepack::Prepare) => prepared = true,
            None => {}
        }
    }
    parsed.unpinned_corepack = enable.filter(|_| !prepared);
    parsed.pins.sort_by_key(|pin| pin.span.start);
    parsed
}

/// How deep command substitutions are followed.
const MAX_SUBSTITUTION_DEPTH: usize = 4;

/// The simple commands of shell text that starts at byte `base` of the file, those inside `$( )` and backticks too.
fn shell_commands(text: &str, base: usize, depth: usize, out: &mut Vec<Command>) {
    let tokens = shell::tokenize(text);
    for segment in tokens.segments {
        if segment.words.is_empty() {
            continue;
        }
        let words = segment
            .words
            .into_iter()
            .map(|mut word| {
                word.span = word.span.start + base..word.span.end + base;
                word
            })
            .collect();
        out.push(Command { words });
    }
    if depth < MAX_SUBSTITUTION_DEPTH {
        for range in tokens.substitutions {
            if let Some(inner) = text.get(range.clone()) {
                shell_commands(inner, base + range.start, depth + 1, out);
            }
        }
    }
}

/// The whitespace-separated words of `text` with their byte offsets.
fn words_at(text: &str) -> Vec<(usize, &str)> {
    let mut words = Vec::new();
    let mut start = None;
    for (index, c) in text.char_indices() {
        match (c.is_whitespace(), start) {
            (true, Some(from)) => {
                words.push((from, &text[from..index]));
                start = None;
            }
            (false, None) => start = Some(index),
            _ => {}
        }
    }
    if let Some(from) = start {
        words.push((from, &text[from..]));
    }
    words
}

/// `value` without one pair of surrounding quotes, with the offset of the unquoted text.
fn unquote(value: &str) -> (&str, usize) {
    for quote in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(quote) {
            let end = inner.find(quote).unwrap_or(inner.len());
            return (&inner[..end], 1);
        }
    }
    (value, 0)
}

/// A pin of `version` at byte `at` of the file, unless the version is computed or empty.
fn pin(tool: Tool, version: &str, at: usize, label: impl Into<String>) -> Option<Pin> {
    (!version.is_empty() && !version.contains('$')).then(|| Pin {
        tool,
        version: version.to_owned(),
        span: at..at + version.len(),
        label: label.into(),
    })
}

/// The tool an asdf or mise tool name stands for.
fn tool_named(name: &str) -> Option<Tool> {
    match name {
        "node" | "nodejs" => Some(Tool::Node),
        other => commands::manager_named(other.strip_prefix("npm:").unwrap_or(other)).map(Tool::Manager),
    }
}

/// Lines of `source` with the byte offset each starts at, line endings removed.
fn lines_at(source: &str) -> Vec<(usize, &str)> {
    let mut offset = 0;
    source
        .split_inclusive('\n')
        .map(|line| {
            let start = offset;
            offset += line.len();
            (start, line.trim_end_matches(['\n', '\r']))
        })
        .collect()
}

fn version_file(name: &str, source: &str, pins: &mut Vec<Pin>) {
    // `.bun-version` is the file `oven-sh/setup-bun` reads with `bun-version-file`.
    let single = match name {
        ".nvmrc" | ".node-version" => Some(Tool::Node),
        ".bun-version" => Some(Tool::Manager(PackageManager::Bun)),
        _ => None,
    };
    if let Some(tool) = single {
        let first = lines_at(source).into_iter().find_map(|(start, line)| {
            let code = line.split('#').next().unwrap_or_default();
            words_at(code).first().map(|&(offset, word)| (start + offset, word))
        });
        if let Some((at, word)) = first {
            let version = word.strip_prefix(['v', 'V']).unwrap_or(word);
            pins.extend(pin(tool, version, at + word.len() - version.len(), name));
        }
        return;
    }
    match name {
        ".tool-versions" => {
            for (start, line) in lines_at(source) {
                let code = line.split('#').next().unwrap_or_default();
                let words = words_at(code);
                if let [(_, tool), (offset, version), ..] = words.as_slice()
                    && let Some(tool) = tool_named(tool)
                {
                    let label = format!(".tool-versions {}", words[0].1);
                    pins.extend(pin(tool, version, start + offset, label));
                }
            }
        }
        _ => mise(name, source, pins),
    }
}

/// The `[tools]` table of `mise.toml`: `node = "20"`, `node = ["20", "18"]` or `node = { version = "20" }`; the
/// first string is the version.
fn mise(name: &str, source: &str, pins: &mut Vec<Pin>) {
    let mut in_tools = false;
    for (start, line) in lines_at(source) {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_tools = trimmed.split('#').next().unwrap_or_default().trim() == "[tools]";
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|_| in_tools) else {
            continue;
        };
        let key = key.trim().trim_matches(['"', '\'']);
        let Some(tool) = tool_named(key) else { continue };
        let value_start = start + key_end(line) + 1;
        let Some(open) = value.find(['"', '\'']) else { continue };
        let (version, _) = unquote(&value[open..]);
        pins.extend(pin(
            tool,
            version,
            value_start + open + 1,
            format!("{name} [tools] {key}"),
        ));
    }
}

/// The byte offset of the `=` of a `key = value` line.
fn key_end(line: &str) -> usize {
    line.find('=').unwrap_or_default()
}

/// The tool a keyword argument of a call in a Bazel file pins, by the last segment of the call's name and the
/// argument's name: `pnpm_version` of `pnpm.pnpm(...)` and `npm_translate_lock(...)` (`rules_js`), `node_version` of
/// `node.toolchain(...)` (`rules_nodejs` under bzlmod) and of `nodejs_register_toolchains(...)` (its `WORKSPACE`
/// macro).
fn bazel_argument(call: &str, argument: &str) -> Option<Tool> {
    let last = call.rsplit('.').next().unwrap_or(call);
    match (last, argument) {
        ("pnpm" | "npm_translate_lock", "pnpm_version") => Some(Tool::Manager(PackageManager::Pnpm)),
        ("toolchain" | "nodejs_register_toolchains", "node_version") => Some(Tool::Node),
        _ => None,
    }
}

/// The versions `MODULE.bazel`, `WORKSPACE` or a `.bzl` file passes as a string literal to a call that installs pnpm
/// or Node (see [`bazel_argument`]); `#` comments are skipped and a computed value pins nothing.
fn bazel(source: &str, pins: &mut Vec<Pin>) {
    let tokens = starlark::tokens(source);
    // The calls whose parentheses are open, innermost last, with the depth of brackets inside each.
    let mut calls: Vec<(Option<String>, usize)> = Vec::new();
    for (index, token) in tokens.iter().enumerate() {
        match token.kind {
            Kind::Punct(b'(') => calls.push((starlark::callee(source, &tokens, index), 0)),
            Kind::Punct(b'[' | b'{') => {
                if let Some((_, depth)) = calls.last_mut() {
                    *depth += 1;
                }
            }
            Kind::Punct(b']' | b'}') => {
                if let Some((_, depth)) = calls.last_mut() {
                    *depth = depth.saturating_sub(1);
                }
            }
            Kind::Punct(b')') => {
                calls.pop();
            }
            Kind::Name => {
                let Some((Some(call), 0)) = calls.last() else { continue };
                let argument = token.text(source);
                let Some(tool) = bazel_argument(call, argument) else {
                    continue;
                };
                let (Some(equals), Some(value)) = (tokens.get(index + 1), tokens.get(index + 2)) else {
                    continue;
                };
                if equals.is(b'=') && value.kind == Kind::Str {
                    let version = value.text(source);
                    pins.extend(pin(tool, version, value.span.start, format!("{call} {argument}")));
                }
            }
            _ => {}
        }
    }
}

/// The escape character of a Dockerfile: `\`, or the one an `# escape=` parser directive at the top sets.
fn escape_char(source: &str) -> char {
    for (_, line) in lines_at(source) {
        let Some(directive) = line.trim().strip_prefix('#') else {
            break;
        };
        if let Some((key, value)) = directive.split_once('=')
            && key.trim().eq_ignore_ascii_case("escape")
            && let Some(c) = value.trim().chars().next()
        {
            return c;
        }
    }
    '\\'
}

fn dockerfile(source: &str, parsed: &mut Parsed) {
    let escape = escape_char(source);
    let lines = lines_at(source);
    let continues = |line: &str| line.trim_end().ends_with(escape);
    let is_code = |line: &str| {
        let trimmed = line.trim_start();
        !trimmed.is_empty() && !trimmed.starts_with('#')
    };
    let mut index = 0;
    while index < lines.len() {
        let (start, line) = lines[index];
        if !is_code(line) {
            index += 1;
            continue;
        }
        let mut last = index;
        let mut open = continues(line);
        while open && last + 1 < lines.len() {
            last += 1;
            if is_code(lines[last].1) {
                open = continues(lines[last].1);
            }
        }
        let begin = start + (line.len() - line.trim_start().len());
        let end = lines[last].0 + lines[last].1.len();
        instruction(source, begin..end, parsed);
        index = last + 1;
    }
}

/// The variables of `ARG` and `ENV` instructions that name a tool version.
fn version_variable(name: &str) -> Option<Tool> {
    match name {
        "NODE_VERSION" => Some(Tool::Node),
        "NPM_VERSION" => Some(Tool::Manager(PackageManager::Npm)),
        "PNPM_VERSION" => Some(Tool::Manager(PackageManager::Pnpm)),
        "YARN_VERSION" => Some(Tool::Manager(PackageManager::Yarn)),
        "BUN_VERSION" => Some(Tool::Manager(PackageManager::Bun)),
        _ => None,
    }
}

fn instruction(source: &str, range: Range<usize>, parsed: &mut Parsed) {
    let text = &source[range.clone()];
    let keyword_len = text.find(char::is_whitespace).unwrap_or(text.len());
    let rest = &text[keyword_len..];
    let body_start = range.start + keyword_len + (rest.len() - rest.trim_start().len());
    let body = &source[body_start..range.end];
    let keyword = text[..keyword_len].to_ascii_uppercase();
    match keyword.as_str() {
        "FROM" => parsed.pins.extend(from_pin(body, body_start)),
        "ARG" | "ENV" => variable_pins(&keyword, body, body_start, &mut parsed.pins),
        "RUN" => run(body, body_start, &mut parsed.commands),
        "COPY" | "ADD" => {
            let sources = context_sources(body);
            if sources.iter().any(|source| matches!(source.as_str(), "." | "./")) {
                parsed.context_copies.push(range.start);
            }
            parsed.copied.extend(sources);
        }
        _ => {}
    }
}

/// The sources of a `COPY` or `ADD` instruction, every argument but the destination, in the exec form
/// (`["a", "b", "./"]`) or the shell form; none when `--from` copies from a stage or an image instead of the build
/// context. Continuation characters and the comment lines between continued lines are dropped.
fn context_sources(body: &str) -> Vec<String> {
    let words = words_at(body);
    let mut offset = body.len();
    for &(at, word) in &words {
        if word.starts_with("--from") {
            return Vec::new();
        }
        if !word.starts_with("--") {
            offset = at;
            break;
        }
    }
    let rest = body[offset..].trim_end();
    let mut arguments: Vec<String> = if rest.starts_with('[') {
        serde_json::from_str(&joined_lines(rest)).unwrap_or_default()
    } else {
        words_at(rest)
            .into_iter()
            .map(|(_, word)| unquote(word.trim_end_matches(['\\', '`'])).0.to_owned())
            .filter(|word| !word.is_empty())
            .collect()
    };
    arguments.pop();
    arguments
}

/// The lines of a continued instruction joined by spaces, without the continuation character at the end of each line
/// and without the comment lines between them.
fn joined_lines(text: &str) -> String {
    text.lines()
        .enumerate()
        .filter(|(index, line)| *index == 0 || !line.trim_start().starts_with('#'))
        .map(|(_, line)| line.trim_end().trim_end_matches(['\\', '`']))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The Node version of `FROM [--platform=...] node:<version>[-variant]`, or the Bun version of
/// `FROM oven/bun:<version>[-variant]`; any image whose last path segment is `node` counts (`node`,
/// `docker.io/library/node`, `registry:5000/node`), and any whose last two are `oven/bun`.
fn from_pin(body: &str, base: usize) -> Option<Pin> {
    let &(offset, image) = words_at(body).iter().find(|(_, word)| !word.starts_with("--"))?;
    let image = image.split('@').next().unwrap_or(image);
    let name_start = image.rfind('/').map_or(0, |slash| slash + 1);
    let colon = name_start + image[name_start..].find(':')?;
    let (tool, label) = match &image[name_start..colon] {
        "node" => (Tool::Node, "FROM node"),
        "bun" if image[..name_start].trim_end_matches('/').rsplit('/').next() == Some("oven") => {
            (Tool::Manager(PackageManager::Bun), "FROM oven/bun")
        }
        _ => return None,
    };
    let tag = &image[colon + 1..];
    let version = tag.split('-').next().unwrap_or(tag);
    if !version.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    pin(tool, version, base + offset + colon + 1, label)
}

/// `ARG NODE_VERSION=20.11.1`, `ENV PNPM_VERSION=9.1.0` and the legacy `ENV NODE_VERSION 20.11.1`.
fn variable_pins(keyword: &str, body: &str, base: usize, pins: &mut Vec<Pin>) {
    let words = words_at(body);
    if keyword == "ENV"
        && let [(_, name), (offset, value), ..] = words.as_slice()
        && !name.contains('=')
    {
        if let Some(tool) = version_variable(name) {
            let (version, quote) = unquote(value);
            pins.extend(pin(tool, version, base + offset + quote, format!("ENV {name}")));
        }
        return;
    }
    for (offset, word) in words {
        let Some((name, value)) = word.split_once('=') else {
            continue;
        };
        let Some(tool) = version_variable(name) else { continue };
        let (version, quote) = unquote(value);
        let at = base + offset + name.len() + 1 + quote;
        pins.extend(pin(tool, version, at, format!("{keyword} {name}")));
    }
}

/// The commands of a `RUN` instruction: its flags (`--mount=...`) skipped, then the exec form (`["npm", "ci"]`) or
/// the shell form.
fn run(body: &str, base: usize, commands: &mut Vec<Command>) {
    let mut offset = 0;
    for (at, word) in words_at(body) {
        if !word.starts_with("--") {
            offset = at;
            break;
        }
        offset = at + word.len();
    }
    let command = &body[offset..];
    let start = base + offset;
    if command.starts_with('[') {
        if let Ok(words) = serde_json::from_str::<Vec<String>>(command.trim_end()) {
            let span = start..start + command.trim_end().len();
            let words = words
                .into_iter()
                .map(|text| Word {
                    text,
                    span: span.clone(),
                    dynamic: false,
                })
                .collect();
            commands.push(Command { words });
        }
        return;
    }
    shell_commands(command, start, 0, commands);
}

/// A line of a workflow: where it starts, its text and its indentation.
struct YamlLine<'a> {
    start: usize,
    text: &'a str,
    indent: usize,
}

impl YamlLine<'_> {
    fn is_code(&self) -> bool {
        let trimmed = self.text.trim_start();
        !trimmed.is_empty() && !trimmed.starts_with('#')
    }

    /// The column the keys of the line start at and the text from there: past a `- ` sequence marker.
    fn mapping(&self) -> (usize, &str, bool) {
        let content = &self.text[self.indent..];
        match content.strip_prefix("- ") {
            Some(after) => {
                let column = self.indent + 2 + (after.len() - after.trim_start().len());
                (column, after.trim_start(), true)
            }
            None => (self.indent, content, false),
        }
    }
}

/// `key: value` at the start of `text`, with the offset of the value in `text`; the value is trimmed and loses a
/// trailing comment unless it is quoted.
fn key_value(text: &str) -> Option<(&str, &str, usize)> {
    let colon = text
        .match_indices(':')
        .map(|(index, _)| index)
        .find(|&index| text[index + 1..].chars().next().is_none_or(char::is_whitespace))?;
    let key = text[..colon].trim().trim_matches(['"', '\'']);
    if key.is_empty() || key.contains(char::is_whitespace) {
        return None;
    }
    let after = &text[colon + 1..];
    let offset = colon + 1 + (after.len() - after.trim_start().len());
    let mut value = after.trim();
    if !value.starts_with(['"', '\'']) {
        value = value.split(" #").next().unwrap_or(value).trim_end();
    }
    Some((key, value, offset))
}

/// The input of a setup action that pins a version, with the tool it pins.
fn setup_input(action: &str) -> Option<(&'static str, Tool)> {
    match action {
        "actions/setup-node" => Some(("node-version", Tool::Node)),
        "pnpm/action-setup" => Some(("version", Tool::Manager(PackageManager::Pnpm))),
        "oven-sh/setup-bun" => Some(("bun-version", Tool::Manager(PackageManager::Bun))),
        _ => None,
    }
}

fn workflow(source: &str, parsed: &mut Parsed) {
    let lines: Vec<YamlLine<'_>> = lines_at(source)
        .into_iter()
        .map(|(start, text)| YamlLine {
            start,
            text,
            indent: text.len() - text.trim_start_matches(' ').len(),
        })
        .collect();
    let mut index = 0;
    while index < lines.len() {
        let line = &lines[index];
        if !line.is_code() {
            index += 1;
            continue;
        }
        let (column, rest, item) = line.mapping();
        if item {
            parsed.pins.extend(setup_pin(&lines, index, column));
        }
        if let Some(("run", value, offset)) = key_value(rest) {
            let value_start = line.start + column + offset;
            if value.starts_with(['|', '>']) {
                let mut last = index;
                for (next, candidate) in lines.iter().enumerate().skip(index + 1) {
                    if candidate.text.trim().is_empty() {
                        continue;
                    }
                    if candidate.indent <= column {
                        break;
                    }
                    last = next;
                }
                if last > index {
                    let body_start = lines[index + 1].start;
                    let body_end = lines[last].start + lines[last].text.len();
                    shell_commands(&source[body_start..body_end], body_start, 0, &mut parsed.commands);
                }
                index = last + 1;
                continue;
            }
            let (command, quote) = unquote(value);
            shell_commands(command, value_start + quote, 0, &mut parsed.commands);
        }
        index += 1;
    }
}

/// The version a step that starts at line `first` (keys at `column`) passes to a setup action.
fn setup_pin(lines: &[YamlLine<'_>], first: usize, column: usize) -> Option<Pin> {
    let mut uses = None;
    let mut with = None;
    let mut end = lines.len();
    for (index, line) in lines.iter().enumerate().skip(first) {
        if index > first {
            if !line.is_code() || line.indent > column {
                continue;
            }
            if line.indent < column || line.text[line.indent..].starts_with("- ") {
                end = index;
                break;
            }
        }
        let rest = if index == first {
            line.mapping().1
        } else {
            &line.text[column..]
        };
        match key_value(rest) {
            Some(("uses", value, _)) => uses = Some(unquote(value).0),
            Some(("with", _, _)) => with = Some(index),
            _ => {}
        }
    }
    let action = uses?.split('@').next()?;
    let (input, tool) = setup_input(action)?;
    for line in &lines[with? + 1..end] {
        if !line.is_code() {
            continue;
        }
        if line.indent <= column {
            break;
        }
        let rest = &line.text[line.indent..];
        if let Some((key, value, offset)) = key_value(rest)
            && key == input
            && !value.contains("${{")
        {
            let (version, quote) = unquote(value);
            return pin(
                tool,
                version,
                line.start + line.indent + offset + quote,
                format!("{action} {input}"),
            );
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pins(kind: ToolchainKind, name: &str, source: &str) -> Vec<(Tool, String, String)> {
        parse(kind, name, source)
            .pins
            .into_iter()
            .map(|pin| {
                // A version file or setup input spans the version; a command spans its `pnpm@<version>` argument.
                assert!(source[pin.span.clone()].ends_with(&pin.version), "{}", pin.label);
                (pin.tool, pin.version, pin.label)
            })
            .collect()
    }

    fn programs(kind: ToolchainKind, source: &str) -> Vec<String> {
        parse(kind, "file", source)
            .commands
            .iter()
            .map(|command| source[command.span()].to_owned())
            .collect()
    }

    const PNPM: Tool = Tool::Manager(PackageManager::Pnpm);

    fn row(tool: Tool, version: &str, label: &str) -> (Tool, String, String) {
        (tool, version.to_owned(), label.to_owned())
    }

    #[test]
    fn version_files_pin_node_and_managers() {
        assert_eq!(
            pins(ToolchainKind::VersionFiles, ".nvmrc", "# comment\nv20.11.1\n"),
            [row(Tool::Node, "20.11.1", ".nvmrc")]
        );
        assert_eq!(
            pins(ToolchainKind::VersionFiles, ".node-version", "22\r\n"),
            [row(Tool::Node, "22", ".node-version")]
        );
        assert_eq!(
            pins(
                ToolchainKind::VersionFiles,
                ".tool-versions",
                "python 3.12.1\nnodejs 20.11.1 18.19.0 # fallback\npnpm 9.1.0\n"
            ),
            [
                row(Tool::Node, "20.11.1", ".tool-versions nodejs"),
                row(PNPM, "9.1.0", ".tool-versions pnpm")
            ]
        );
        assert_eq!(
            pins(
                ToolchainKind::VersionFiles,
                "mise.toml",
                "[env]\nnode = \"not a tool\"\n[tools]\nnode = [\"20.11\", \"18\"]\n\"npm:pnpm\" = { version = '9' }\n[tasks]\nnode = \"x\"\n"
            ),
            [
                row(Tool::Node, "20.11", "mise.toml [tools] node"),
                row(PNPM, "9", "mise.toml [tools] npm:pnpm")
            ]
        );
        assert_eq!(pins(ToolchainKind::VersionFiles, ".nvmrc", "\n"), []);
        assert_eq!(
            pins(ToolchainKind::VersionFiles, ".bun-version", "# pinned for CI\n1.3.0\n"),
            [row(Tool::Manager(PackageManager::Bun), "1.3.0", ".bun-version")]
        );
    }

    #[test]
    fn bazel_files_pin_through_rules_js_and_rules_nodejs_calls() {
        let source = "\
pnpm.pnpm(name = \"pnpm\", pnpm_version = \"8.15.0\")
npm_translate_lock(name = \"npm\", data = [\"//:package.json\"], pnpm_version = '9.1.0')
node.toolchain(node_version = \"20.11.1\", node_urls = [\"https://example.com/{filename}\"])
nodejs_register_toolchains(name = \"nodejs\", node_version = r\"18.19.0\")
# pnpm.pnpm(pnpm_version = \"7.0.0\")
other(pnpm_version = \"6.0.0\", inner = pnpm.pnpm(pnpm_version = PNPM))
pnpm.pnpm(pnpm_version = \"${VERSION}\", extra = {\"pnpm_version\": \"5.0.0\"})
node.toolchain(versions = [dict(node_version = \"16.0.0\")])
if pnpm_version == \"4.0.0\":
    pass
";
        assert_eq!(
            pins(ToolchainKind::Bazel, "MODULE.bazel", source),
            [
                row(PNPM, "8.15.0", "pnpm.pnpm pnpm_version"),
                row(PNPM, "9.1.0", "npm_translate_lock pnpm_version"),
                row(Tool::Node, "20.11.1", "node.toolchain node_version"),
                row(Tool::Node, "18.19.0", "nodejs_register_toolchains node_version"),
            ]
        );
    }

    #[test]
    fn dockerfiles_pin_bun_through_the_official_image() {
        let source = "\
FROM oven/bun:1.3.3-alpine AS deps
FROM docker.io/oven/bun:1.2 AS build
FROM oven/bun:latest
FROM example/bun:1.0.0
FROM bun:1.0.0
";
        let bun = Tool::Manager(PackageManager::Bun);
        assert_eq!(
            pins(ToolchainKind::Dockerfiles, "Dockerfile", source),
            [row(bun, "1.3.3", "FROM oven/bun"), row(bun, "1.2", "FROM oven/bun")]
        );
    }

    #[test]
    fn dockerfiles_pin_through_from_arg_env_and_commands() {
        let source = "\
# syntax=docker/dockerfile:1
ARG NODE_VERSION=\"20.11.1\" OTHER=1
FROM --platform=linux/amd64 docker.io/library/node:18.19.0-alpine3.19 AS build
FROM node:${NODE_VERSION}-slim
FROM node:lts
ENV PNPM_VERSION 8.15.0
RUN --mount=type=cache,target=/root/.npm \\
    corepack enable && \\
    # the version the lockfile wants
    corepack prepare pnpm@9.1.0 --activate
";
        assert_eq!(
            pins(ToolchainKind::Dockerfiles, "Dockerfile", source),
            [
                row(Tool::Node, "20.11.1", "ARG NODE_VERSION"),
                row(Tool::Node, "18.19.0", "FROM node"),
                row(PNPM, "8.15.0", "ENV PNPM_VERSION"),
                row(PNPM, "9.1.0", "corepack prepare"),
            ]
        );
        assert_eq!(
            parse(ToolchainKind::Dockerfiles, "Dockerfile", source).unpinned_corepack,
            None
        );
    }

    #[test]
    fn command_pins_span_their_argument() {
        let source = "RUN npm install -g pnpm@8.15.0\n";
        let parsed = parse(ToolchainKind::Dockerfiles, "Dockerfile", source);
        assert_eq!(parsed.pins.len(), 1);
        assert_eq!(&source[parsed.pins[0].span.clone()], "pnpm@8.15.0");
        assert_eq!(parsed.pins[0].version, "8.15.0");
    }

    #[test]
    fn dockerfile_commands_join_continuation_lines() {
        let source = "\
FROM node:20
RUN apt-get update \\
 && npm ci
RUN [\"yarn\", \"install\"]
CMD npm start
RUN --network=none pnpm build $(npm bin)
";
        assert_eq!(
            programs(ToolchainKind::Dockerfiles, source),
            [
                "apt-get update",
                "npm ci",
                "[\"yarn\", \"install\"]",
                "pnpm build $(npm bin)",
                "npm bin"
            ]
        );
        let exec = &parse(ToolchainKind::Dockerfiles, "Dockerfile", source).commands[2];
        assert_eq!(
            exec.words.iter().map(|word| word.text.as_str()).collect::<Vec<_>>(),
            ["yarn", "install"]
        );
    }

    #[test]
    fn dockerfile_copies_from_the_build_context_are_recorded() {
        let source = "\
FROM node:20 AS build
COPY package.json pnpm-lock.yaml ./
COPY --chown=node:node [\"tsconfig.json\", \"src\", \"/app/\"]
ADD \"bun.lock\" \\
    .npmrc /app/
COPY --from=build /app/dist ./dist
copy yarn.lock* .
COPY ./only-destination
";
        assert_eq!(
            parse(ToolchainKind::Dockerfiles, "Dockerfile", source).copied,
            [
                "package.json",
                "pnpm-lock.yaml",
                "tsconfig.json",
                "src",
                "bun.lock",
                ".npmrc",
                "yarn.lock*"
            ]
        );
        assert_eq!(
            parse(ToolchainKind::ShellScripts, "x.sh", "COPY a b\n").copied,
            Vec::<String>::new()
        );
    }

    #[test]
    fn an_exec_form_copy_continued_over_lines_is_recorded() {
        let source = "\
FROM node:20
COPY [\"package.json\", \\
# the lockfile
      \"pnpm-lock.yaml\", \\
      \"./\"]
";
        assert_eq!(
            parse(ToolchainKind::Dockerfiles, "Dockerfile", source).copied,
            ["package.json", "pnpm-lock.yaml"]
        );
    }

    #[test]
    fn copies_of_the_whole_build_context_are_recorded_where_they_start() {
        let source = "FROM node:20\nCOPY package.json ./\nCOPY . .\nADD ./ /app\nCOPY --from=build . .\n";
        let parsed = parse(ToolchainKind::Dockerfiles, "Dockerfile", source);
        let at = |text: &str| source.find(text).unwrap();
        assert_eq!(parsed.context_copies, [at("COPY . ."), at("ADD ./")]);
    }

    #[test]
    fn corepack_enable_without_prepare_is_recorded() {
        let enabled = "RUN corepack enable\nRUN pnpm install\n";
        let parsed = parse(ToolchainKind::Dockerfiles, "Dockerfile", enabled);
        assert_eq!(
            parsed.unpinned_corepack.map(|span| &enabled[span]),
            Some("corepack enable")
        );
        let prepared = "RUN corepack enable\nRUN corepack prepare --activate\n";
        assert_eq!(
            parse(ToolchainKind::Dockerfiles, "Dockerfile", prepared).unpinned_corepack,
            None
        );
    }

    #[test]
    fn workflows_pin_through_setup_actions_and_run_commands() {
        let source = "\
name: ci
on: push
defaults:
  run:
    shell: bash
jobs:
  test:
    strategy:
      matrix:
        node: [18, 20]
    steps:
      - uses: actions/checkout@v4
      - uses: pnpm/action-setup@v4
        with:
          version: 8.15.0 # older
      - name: node
        uses: actions/setup-node@v4
        with:
          cache: pnpm
          node-version: '20.11.1'
      - uses: actions/setup-node@v4
        with:
          node-version: ${{ matrix.node }}
      - run: npm ci
      - name: build
        run: |
          # skif-ignore-next-line foreign-package-manager-command -- old branch
          yarn install

          npx tsc --noEmit
      - run: \"pnpm test\"
";
        assert_eq!(
            pins(ToolchainKind::Workflows, "ci.yml", source),
            [
                row(PNPM, "8.15.0", "pnpm/action-setup version"),
                row(Tool::Node, "20.11.1", "actions/setup-node node-version"),
            ]
        );
        assert_eq!(
            programs(ToolchainKind::Workflows, source),
            ["npm ci", "yarn install", "npx tsc --noEmit", "pnpm test"]
        );
        let parsed = parse(ToolchainKind::Workflows, "ci.yml", source);
        assert_eq!(parsed.directives.len(), 1);
        let yarn_line = source[..parsed.commands[1].span().start].matches('\n').count() + 1;
        assert_eq!(parsed.directives[0].covers, Some((28, 28)));
        assert_eq!(yarn_line, 28);
    }

    #[test]
    fn shell_scripts_are_commands_throughout() {
        let source =
            "#!/usr/bin/env bash\nset -euo pipefail\nif [ -n \"$CI\" ]; then npm ci; fi\nV=$(yarn --version)\n";
        assert_eq!(
            programs(ToolchainKind::ShellScripts, source),
            [
                "set -euo pipefail",
                "if [ -n \"$CI\" ]",
                "then npm ci",
                "fi",
                "yarn --version"
            ]
        );
    }
}
