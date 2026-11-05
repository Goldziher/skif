//! The `package.json` scripts that run a tool. Their flags change what the old tool checked, so they are translated
//! too.

use std::path::Path;

use skif_js_plugins::scripts::split_commands;
use skif_js_syntax::ConfigValue;

use crate::notes::Tool;

/// Words that may stand before the tool in a command: package-manager launchers and their subcommands.
const LAUNCHERS: &[&str] = &[
    "npx", "pnpm", "pnpx", "yarn", "npm", "bun", "bunx", "exec", "dlx", "run", "x",
];

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
                args.next_if(|next| !next.starts_with("--")).cloned()
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

/// Every command of every script of `package_json` that runs `tool`.
pub(crate) fn invocations(package_json: &ConfigValue, tool: Tool) -> Vec<Invocation> {
    let Some(scripts) = package_json.get("scripts").and_then(ConfigValue::as_object) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (name, body) in scripts {
        let Some(body) = body.as_str() else { continue };
        for words in split_commands(body) {
            let position = words.iter().position(|word| is_tool_word(word, tool));
            let Some(position) = position else { continue };
            let launched = words[..position]
                .iter()
                .all(|word| LAUNCHERS.contains(&word.as_str()) || word.starts_with('-'));
            if launched {
                out.push(Invocation {
                    script: name.clone(),
                    args: words[position + 1..].to_vec(),
                });
            }
        }
    }
    out
}

/// The `package.json` key that configures `tool`.
pub(crate) fn config_key<'v>(tool: Tool, package_json: &'v ConfigValue) -> Option<&'v ConfigValue> {
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
