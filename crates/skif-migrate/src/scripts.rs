//! The `package.json` scripts that run a tool. Their flags change what the old tool checked, so they are translated
//! too.

use std::path::Path;

use skif_js_plugins::scripts::split_commands;
use skif_js_syntax::ConfigValue;

use crate::notes::{NoteClass, Tool};
use crate::state::{Src, State};

/// Words that may stand before the tool in a command: package-manager launchers and their subcommands, and the
/// wrappers that set up an environment or a task runner around it.
const LAUNCHERS: &[&str] = &[
    "npx",
    "pnpm",
    "pnpx",
    "yarn",
    "npm",
    "bun",
    "bunx",
    "exec",
    "dlx",
    "run",
    "x",
    "corepack",
    "cross-env",
    "dotenv",
    "dotenvx",
    "env",
    "time",
    "nohup",
    "turbo",
    "nx",
];

/// Task runners that name the task as a word: `turbo run knip -- --production` passes only what follows `--` on.
const TASK_RUNNERS: &[&str] = &["turbo", "nx"];

/// Flags of launchers and wrappers that take a value, which then is not a word of the command.
const VALUE_FLAGS: &[&str] = &[
    "--filter",
    "-F",
    "--workspace",
    "--cwd",
    "-C",
    "--prefix",
    "--dir",
    "--package",
    "-p",
    "-e",
    "-f",
];

/// Programs that take whole commands as arguments: the commands inside are the ones that run the tool.
const COMMAND_RUNNERS: &[&str] = &["concurrently", "conc"];
const SHELLS: &[&str] = &["sh", "bash", "zsh"];

/// Programs whose arguments only mention the tool.
const MENTIONERS: &[&str] = &["echo", "printf", "cat", "grep", "rg", "which", "type", "man"];

/// One command of one script that runs the tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Invocation {
    /// The script name.
    pub script: String,
    /// The words after the tool.
    pub args: Vec<String>,
}

impl Invocation {
    /// `scripts.<name>`, the path notes about the invocation use.
    pub(crate) fn path(&self) -> String {
        format!("scripts.{}", self.script)
    }

    /// The first word that is not a flag, the subcommand of `syncpack lint`.
    pub(crate) fn subcommand(&self) -> Option<&str> {
        self.args
            .first()
            .map(String::as_str)
            .filter(|word| !word.starts_with('-'))
    }

    /// Whether `--long` (or the short form `-s`) is present.
    pub(crate) fn has(&self, long: &str, short: Option<&str>) -> bool {
        self.args.iter().any(|arg| {
            arg == &format!("--{long}")
                || arg.starts_with(&format!("--{long}="))
                || short.is_some_and(|short| arg == &format!("-{short}"))
        })
    }

    /// The values of every `--long value`, `--long=value`, `-s value` pair, split at commas.
    pub(crate) fn values(&self, long: &str, short: Option<&str>) -> Vec<String> {
        let mut out = Vec::new();
        let mut args = self.args.iter().peekable();
        while let Some(arg) = args.next() {
            let long_flag = format!("--{long}");
            let value = if *arg == long_flag || short.is_some_and(|short| *arg == format!("-{short}")) {
                args.next_if(|next| !next.starts_with('-')).cloned()
            } else {
                arg.strip_prefix(&format!("{long_flag}=")).map(str::to_owned)
            };
            if let Some(value) = value {
                out.extend(
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|v| !v.is_empty())
                        .map(str::to_owned),
                );
            }
        }
        out
    }

    /// The flags that are present, as `--long` or `-s`, whatever their values.
    pub(crate) fn flags(&self) -> Vec<String> {
        self.args
            .iter()
            .filter(|arg| arg.starts_with('-') && arg.len() > 1)
            .map(|arg| arg.split_once('=').map_or(arg.as_str(), |(flag, _)| flag).to_owned())
            .collect()
    }
}

fn is_tool_word(word: &str, tool: Tool) -> bool {
    let name = tool.name();
    if word == name || word.strip_prefix(name).is_some_and(|rest| rest.starts_with('@')) {
        return true;
    }
    Path::new(word).file_name().is_some_and(|file| file == name) && word.contains('/')
}

/// Whether `word` is a `NAME=value` environment assignment.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        !name.is_empty()
            && !name.starts_with(|c: char| c.is_ascii_digit())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// Whether the words before the tool are launchers, wrappers, their flags and environment assignments only.
fn launched(prefix: &[String]) -> bool {
    let mut skip = false;
    prefix.iter().all(|word| {
        if std::mem::take(&mut skip) {
            return true;
        }
        if LAUNCHERS.contains(&word.as_str()) || is_assignment(word) {
            return true;
        }
        if word.starts_with('-') {
            skip = VALUE_FLAGS.contains(&word.as_str());
            return true;
        }
        false
    })
}

/// The commands a command carries as arguments: the quoted commands of `concurrently`, the script of `sh -c`.
fn nested(words: &[String]) -> Vec<String> {
    let Some(program) = words
        .iter()
        .position(|word| COMMAND_RUNNERS.contains(&word.as_str()) || SHELLS.contains(&word.as_str()))
    else {
        return Vec::new();
    };
    let rest = &words[program + 1..];
    if SHELLS.contains(&words[program].as_str()) {
        let script = rest
            .iter()
            .position(|word| word == "-c")
            .and_then(|at| rest.get(at + 1));
        return script.cloned().into_iter().collect();
    }
    rest.iter().filter(|word| !word.starts_with('-')).cloned().collect()
}

/// The simple commands of a script, with the commands nested in `concurrently` and `sh -c` flattened out.
fn commands(body: &str) -> Vec<Vec<String>> {
    fn collect(words: Vec<String>, depth: usize, out: &mut Vec<Vec<String>>) {
        let inner = if depth < 3 { nested(&words) } else { Vec::new() };
        if inner.is_empty() {
            out.push(words);
            return;
        }
        for script in inner {
            for command in split_commands(&script) {
                collect(command, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    for words in split_commands(body) {
        collect(words, 0, &mut out);
    }
    out
}

/// The words after the tool that are its own arguments. Task runners (`turbo run knip -- --production`) hand on only
/// what follows `--`, and `--` itself is never an argument of the tool.
fn tool_args(prefix: &[String], rest: &[String]) -> Vec<String> {
    let task_form = prefix
        .iter()
        .rposition(|word| TASK_RUNNERS.contains(&word.as_str()))
        .is_some_and(|runner| !prefix[runner..].iter().any(|word| word == "--"));
    let rest = if task_form {
        rest.iter()
            .position(|word| word == "--")
            .map_or(&rest[rest.len()..], |at| &rest[at..])
    } else {
        rest
    };
    rest.iter().filter(|word| *word != "--").cloned().collect()
}

/// Every command of every script of `package_json` that runs `tool`.
pub(crate) fn invocations(package_json: &ConfigValue, tool: Tool) -> Vec<Invocation> {
    let mut out = Vec::new();
    for (name, words) in script_commands(package_json) {
        let Some(position) = words.iter().position(|word| is_tool_word(word, tool)) else {
            continue;
        };
        if launched(&words[..position]) {
            out.push(Invocation {
                script: name,
                args: tool_args(&words[..position], &words[position + 1..]),
            });
        }
    }
    out
}

fn script_commands(package_json: &ConfigValue) -> Vec<(String, Vec<String>)> {
    let Some(scripts) = package_json.get("scripts").and_then(ConfigValue::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, body) in scripts {
        let Some(body) = body.as_str() else { continue };
        out.extend(commands(body).into_iter().map(|words| (name.clone(), words)));
    }
    out
}

/// The scripts that name `tool` in a command that is not recognised as running it, so their flags were not read.
fn unrecognised(package_json: &ConfigValue, tool: Tool) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (name, words) in script_commands(package_json) {
        let Some(position) = words.iter().position(|word| is_tool_word(word, tool)) else {
            continue;
        };
        let mentions = words
            .first()
            .is_some_and(|program| MENTIONERS.contains(&program.as_str()));
        if !launched(&words[..position]) && !mentions && !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

/// A note for every script that mentions `tool` in a way the translation does not understand.
pub(crate) fn note_unrecognised(state: &mut State<'_>, src: &Src, package_json: &ConfigValue, tool: Tool) {
    for name in unrecognised(package_json, tool) {
        state.note(
            src,
            NoteClass::Human,
            format!("scripts.{name}"),
            format!("this script mentions {tool} in a command that was not recognised, so its flags were not read"),
        );
    }
}

/// The names of the scripts of `invocations`, each once.
fn script_names(invocations: &[Invocation]) -> String {
    let mut names: Vec<String> = Vec::new();
    for invocation in invocations {
        let path = invocation.path();
        if !names.contains(&path) {
            names.push(path);
        }
    }
    names.join(", ")
}

/// When the runs of one tool do not all pass the same `--long` values: the scripts that ran it. Each run was a
/// separate invocation of the old tool, but skif has one configuration, so the caller notes the merge.
pub(crate) fn differing_values(invocations: &[Invocation], long: &str, short: Option<&str>) -> Option<String> {
    let sets: Vec<Vec<String>> = invocations
        .iter()
        .map(|invocation| {
            let mut values = invocation.values(long, short);
            values.sort();
            values.dedup();
            values
        })
        .collect();
    sets.windows(2)
        .any(|pair| pair[0] != pair[1])
        .then(|| script_names(invocations))
}

/// When `--long` is present in some runs of one tool and absent from others: the scripts that ran it.
pub(crate) fn differing_switch(invocations: &[Invocation], long: &str, short: Option<&str>) -> Option<String> {
    let present: Vec<bool> = invocations
        .iter()
        .map(|invocation| invocation.has(long, short))
        .collect();
    present
        .windows(2)
        .any(|pair| pair[0] != pair[1])
        .then(|| script_names(invocations))
}

/// The `package.json` key that configures `tool`.
pub(crate) fn config_key(tool: Tool, package_json: &ConfigValue) -> Option<&ConfigValue> {
    match tool {
        Tool::Sherif | Tool::Knip => package_json.get(tool.name()),
        Tool::Syncpack => package_json
            .get("syncpack")
            .or_else(|| package_json.get("config").and_then(|config| config.get("syncpack"))),
    }
}

/// Whether `package_json` has a key that configures `tool`.
pub(crate) fn has_key(tool: Tool, package_json: &ConfigValue) -> bool {
    config_key(tool, package_json).is_some()
}

/// The configuration file a script passes to `tool` with `--config` or `-c`, so a caller can look there first.
/// Only syncpack and knip take a configuration file.
#[must_use]
pub fn script_config_path(tool: Tool, package_json: &str) -> Option<String> {
    if tool == Tool::Sherif {
        return None;
    }
    let value = skif_js_plugins::load_config(Path::new("package.json"), package_json)?;
    invocations(&value, tool)
        .iter()
        .find_map(|invocation| invocation.values("config", Some("c")).into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(scripts: &str) -> ConfigValue {
        skif_js_plugins::load_config(Path::new("package.json"), &format!("{{ \"scripts\": {scripts} }}")).unwrap()
    }

    #[test]
    fn finds_the_tool_behind_launchers_and_separators() {
        let value = package(
            r#"{
                "a": "pnpm exec knip --production && echo ok",
                "b": "echo knip",
                "c": "npx knip@5 --strict",
                "d": "./node_modules/.bin/knip -c conf.json"
            }"#,
        );
        let found = invocations(&value, Tool::Knip);
        let scripts: Vec<&str> = found.iter().map(|i| i.script.as_str()).collect();
        assert_eq!(scripts, ["a", "c", "d"]);
        assert_eq!(found[0].args, ["--production"]);
    }

    #[test]
    fn finds_the_tool_behind_wrappers_and_environment() {
        let value = package(
            r#"{
                "a": "CI=1 knip --production",
                "b": "cross-env NODE_ENV=production knip --strict",
                "c": "dotenv -e .env -- knip --include files",
                "d": "pnpm --filter web exec knip -p",
                "e": "concurrently \"knip --exports\" \"syncpack lint\"",
                "f": "npm run knip -- --include files",
                "g": "turbo run knip -- --production",
                "h": "turbo run knip --filter=web",
                "i": "bash -c 'knip --files'"
            }"#,
        );
        let found = invocations(&value, Tool::Knip);
        let seen: Vec<(&str, Vec<&str>)> = found
            .iter()
            .map(|invocation| {
                (
                    invocation.script.as_str(),
                    invocation.args.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                ("a", vec!["--production"]),
                ("b", vec!["--strict"]),
                ("c", vec!["--include", "files"]),
                ("d", vec!["-p"]),
                ("e", vec!["--exports"]),
                ("f", vec!["--include", "files"]),
                ("g", vec!["--production"]),
                ("h", vec![]),
                ("i", vec!["--files"]),
            ]
        );
        assert_eq!(found[5].flags(), ["--include"]);
        let syncpack = invocations(&value, Tool::Syncpack);
        assert_eq!(syncpack.len(), 1);
        assert_eq!(syncpack[0].subcommand(), Some("lint"));
    }

    #[test]
    fn a_value_flag_does_not_swallow_the_next_flag() {
        let value = package(r#"{ "a": "sherif -i --fail-on-warnings" }"#);
        let found = &invocations(&value, Tool::Sherif)[0];
        assert_eq!(found.values("ignore-dependency", Some("i")), Vec::<String>::new());
        assert!(found.has("fail-on-warnings", None));
    }

    #[test]
    fn an_unrecognised_command_that_names_the_tool_is_reported() {
        let value = package(
            r#"{
                "a": "tsx scripts/run.ts knip --production",
                "b": "echo knip",
                "c": "knip",
                "d": "node scripts/other.js"
            }"#,
        );
        assert_eq!(unrecognised(&value, Tool::Knip), ["a"]);
    }

    #[test]
    fn runs_that_differ_are_detected() {
        let value =
            package(r#"{ "a": "knip --production --include files", "b": "knip --include files", "c": "knip" }"#);
        let found = invocations(&value, Tool::Knip);
        assert_eq!(
            differing_switch(&found, "production", Some("p")).as_deref(),
            Some("scripts.a, scripts.b, scripts.c")
        );
        assert_eq!(differing_values(&found[..2], "include", None), None);
        assert!(differing_values(&found, "include", None).is_some());
        assert_eq!(differing_switch(&found[..1], "production", Some("p")), None);
    }

    #[test]
    fn reads_flag_values_in_both_spellings() {
        let value = package(
            r#"{ "a": "sherif -i react --ignore-dependency=x --ignore-dependency @scope/pkg,other --fail-on-warnings" }"#,
        );
        let found = &invocations(&value, Tool::Sherif)[0];
        assert!(found.has("fail-on-warnings", None));
        assert_eq!(
            found.values("ignore-dependency", Some("i")),
            ["react", "x", "@scope/pkg", "other"]
        );
    }

    #[test]
    fn a_config_flag_names_the_file() {
        let text = r#"{ "scripts": { "knip": "knip --config tooling/knip.json" } }"#;
        assert_eq!(
            script_config_path(Tool::Knip, text).as_deref(),
            Some("tooling/knip.json")
        );
        assert_eq!(script_config_path(Tool::Sherif, text), None);
    }
}
