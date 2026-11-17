//! The parity of `skif mcp` with the command line: every command has a tool or a reason, every flag of a tool's
//! command an argument or a reason, and every argument parses as the flag of the same name. Introspects clap's
//! definition, so a flag added to the command line fails here until the server covers it.

use serde_json::{Map, Value, json};

use super::{ConfigCommand, ExplainCommand, LintCommand, Param, RulesCommand, Shape, TOOLS, Tool, parse, tools};

/// What happens to each command of the command line in the server: the tool that runs it, or why none does.
/// `""` is the bare `skif`, the lint.
const COMMANDS: &[(&str, Result<&str, &str>)] = &[
    ("", Ok("lint")),
    ("rules", Ok("list_rules")),
    ("explain-usage", Ok("explain_usage")),
    ("config", Ok("config")),
    ("schema", Err("a static document, published as schema/skif.schema.json")),
    ("init", Err("writes skif.toml")),
    ("migrate", Err("writes skif.toml; a one-off decision for a human")),
    (
        "baseline",
        Err("writes or prunes the baseline file; `check` reads git history"),
    ),
    ("update", Err("uses the network and writes manifests")),
    (
        "list",
        Err("a human view of the version groups; their findings come from `lint`"),
    ),
    (
        "json",
        Err("NDJSON of every instance the version groups govern; their findings come from `lint`"),
    ),
    (
        "format",
        Err("writes package.json files; its check is `lint` with `include: [package-json-format]`"),
    ),
    ("mcp", Err("this server")),
    ("lsp", Err("an editor server")),
];

/// The flags of each tool's command that no argument stands for, and why. With [`Tool::fixed`], the implied
/// flags and the arguments, they cover every flag of the command exactly once.
const NOT_EXPOSED: &[(&str, &[(&str, &str)])] = &[
    (
        "lint",
        &[
            ("output", "writes a file; the report is the result"),
            ("no_install", "a dry run installs nothing"),
            ("no_prompt", "the server never prompts"),
            ("max_warnings", "changes only the exit code; read `summary`"),
            ("fail_on_warnings", "changes only the exit code; read `summary`"),
            ("max_issues", "changes only the exit code; read `summary`"),
            ("no_exit_code", "changes only the exit code"),
            ("rules", "a configuration override; pass `config` or edit skif.toml"),
            ("ignore_rule", "use `exclude`"),
            (
                "ignore_package",
                "a configuration override; use `workspace` or edit skif.toml",
            ),
            ("ignore_dependency", "a configuration override; edit skif.toml"),
            ("ignore_rule_for_package", "a configuration override; edit skif.toml"),
            (
                "require_ignore_reasons",
                "a policy check for CI; `[lint] require-ignore-reasons` still applies",
            ),
            (
                "type_aware",
                "long-lived hosts analyse without types (ADR 0010, ADR 0013)",
            ),
            ("no_gitignore", "a configuration override; `[js] no-gitignore`"),
            (
                "include_entry_exports",
                "a configuration override; `[js] include-entry-exports`",
            ),
            ("tsconfig", "a configuration override; `[js] tsconfig`"),
            (
                "use_tsconfig_files",
                "a configuration override; `[js] use-tsconfig-files`",
            ),
            (
                "isolate_workspaces",
                "a configuration override; `[js] isolate-workspaces`, or `strict`",
            ),
            ("color", "terminal output only"),
            ("watch", "a long-running loop; call `lint` again"),
            ("cache", "the server keeps its own module cache per path"),
            ("cache_location", "the server keeps its own module cache per path"),
            ("trace_dependency", "use `explain_usage` with `dependency`"),
            ("debug", "stderr diagnostics only"),
            ("performance", "stderr diagnostics only"),
            ("duration", "stderr diagnostics only"),
            ("memory", "stderr diagnostics only"),
            ("dependencies", "a shortcut for `include`"),
            ("exports", "a shortcut for `include`"),
            ("files", "a shortcut for `include`"),
            ("cycles", "a shortcut for `include`"),
            (
                "tags",
                "a configuration override; `[js] ignore-exports-used-in-file` and tags in skif.toml",
            ),
            ("dependency", "a version-group filter; use `include` or `workspace`"),
            (
                "dependency_types",
                "a version-group filter; use `include` or `workspace`",
            ),
            (
                "specifier_types",
                "a version-group filter; use `include` or `workspace`",
            ),
            ("source", "a version-group filter; use `include` or `workspace`"),
        ],
    ),
    (
        "explain_usage",
        &[(
            "type_aware",
            "long-lived hosts analyse without types (ADR 0010, ADR 0013)",
        )],
    ),
    ("config", &[]),
    ("list_rules", &[]),
];

/// The arguments no flag stands for, and what the tool does with them.
const FOLDED: &[(&str, &str, &str)] = &[("explain_usage", "export", "joins `file` as `FILE#EXPORT`")];

fn tool(name: &str) -> &'static Tool {
    TOOLS.iter().find(|tool| tool.name == name).unwrap()
}

/// The clap ids of `command`'s arguments, without help and version.
fn flag_ids(command: &clap::Command) -> Vec<String> {
    command
        .get_arguments()
        .map(|arg| arg.get_id().as_str().to_owned())
        .filter(|id| id != "help" && id != "version")
        .collect()
}

/// The clap id of the flag `--long` (or `long` without dashes).
fn id_of(command: &clap::Command, flag: &str) -> String {
    let long = flag.trim_start_matches('-');
    command
        .get_arguments()
        .find(|arg| arg.get_long() == Some(long))
        .unwrap_or_else(|| panic!("no flag --{long}"))
        .get_id()
        .as_str()
        .to_owned()
}

#[test]
fn every_command_has_a_tool_or_a_reason() {
    use clap::CommandFactory as _;
    let mut commands: Vec<&str> = crate::args::Cli::command()
        .get_subcommands()
        .map(clap::Command::get_name)
        .filter(|name| *name != "help")
        .map(|name| Box::leak(name.to_owned().into_boxed_str()) as &str)
        .collect();
    commands.insert(0, "");
    let mut declared: Vec<&str> = COMMANDS.iter().map(|(command, _)| *command).collect();
    commands.sort_unstable();
    declared.sort_unstable();
    assert_eq!(
        declared, commands,
        "COMMANDS must name every command of the command line once"
    );
    let mut tools: Vec<&str> = COMMANDS.iter().filter_map(|(_, tool)| tool.ok()).collect();
    let mut served: Vec<&str> = TOOLS.iter().map(|tool| tool.name).collect();
    tools.sort_unstable();
    served.sort_unstable();
    assert_eq!(tools, served, "every tool stands for one command");
}

#[test]
fn every_flag_of_a_tool_command_has_an_argument_or_a_reason() {
    for tool in TOOLS {
        let command = (tool.command)();
        let mut covered: Vec<(String, &str)> = Vec::new();
        for flag in tool.fixed.iter().filter(|flag| flag.starts_with("--")) {
            covered.push((id_of(&command, flag), "fixed"));
        }
        let implied = ["fix", "explain"]
            .iter()
            .map(|key| (*key).to_owned())
            .map(|key| Map::from_iter([(key, Value::Bool(true))]));
        for arguments in std::iter::once(Map::new()).chain(implied) {
            for flag in (tool.implied)(&arguments) {
                covered.push((id_of(&command, flag), "implied"));
            }
        }
        for param in tool.params {
            if let Some(id) = param.id {
                covered.push((id.to_owned(), "argument"));
            }
        }
        let (_, hidden) = NOT_EXPOSED.iter().find(|(name, _)| *name == tool.name).unwrap();
        for (id, reason) in *hidden {
            assert_ne!(*reason, "", "{}: `{id}` needs a reason", tool.name);
            covered.push(((*id).to_owned(), "not exposed"));
        }
        covered.sort();
        covered.dedup_by(|a, b| a == b && a.1 == "implied");
        let mut ids: Vec<String> = covered.iter().map(|(id, _)| id.clone()).collect();
        let mut flags = flag_ids(&command);
        flags.sort();
        let unique = {
            let mut unique = ids.clone();
            unique.dedup();
            unique
        };
        assert_eq!(ids, unique, "{}: a flag is covered twice: {covered:?}", tool.name);
        ids.dedup();
        assert_eq!(
            ids, flags,
            "{}: every flag needs an argument, a fixed or implied flag, or a NOT_EXPOSED reason",
            tool.name
        );
        for param in tool.params.iter().filter(|param| param.id.is_none()) {
            assert!(
                FOLDED
                    .iter()
                    .any(|(name, folded, _)| *name == tool.name && *folded == param.name),
                "{}: `{}` stands for no flag and is not in FOLDED",
                tool.name,
                param.name
            );
        }
    }
}

/// The instructions promise that an argument is the flag of the same name; the positional `TARGET` of
/// `explain-usage` is the one exception, as `file`.
#[test]
fn each_argument_is_named_after_its_flag() {
    for tool in TOOLS {
        for param in tool.params {
            match (tool.name, param.name, param.id) {
                ("explain_usage", "file", Some("target")) | (_, _, None) => {}
                (_, name, Some(id)) => assert_eq!(name, id, "{}: `{name}` stands for `{id}`", tool.name),
            }
        }
    }
}

#[test]
fn each_argument_has_the_type_of_its_flag() {
    for tool in TOOLS {
        let command = (tool.command)();
        for param in tool.params {
            let Some(id) = param.id else {
                continue;
            };
            let arg = command.get_arguments().find(|arg| arg.get_id() == id).unwrap();
            let expected = match arg.get_action() {
                clap::ArgAction::SetTrue => vec![Shape::Flag],
                clap::ArgAction::Append => vec![Shape::List],
                clap::ArgAction::Set => vec![Shape::Text, Shape::Count],
                other => panic!("{}: `{}` has the action {other:?}", tool.name, param.name),
            };
            assert!(
                expected.contains(&param.shape),
                "{}: `{}` is {:?}, its flag takes {expected:?}",
                tool.name,
                param.name,
                param.shape
            );
        }
    }
}

#[test]
fn the_input_schemas_list_exactly_the_arguments() {
    let listed = tools();
    for (entry, tool) in listed.as_array().unwrap().iter().zip(TOOLS) {
        assert_eq!(entry["name"], tool.name);
        let properties = entry["inputSchema"]["properties"].as_object().unwrap();
        let names: Vec<&str> = properties.keys().map(String::as_str).collect();
        let params: Vec<&str> = tool.params.iter().map(|param| param.name).collect();
        assert_eq!(names, params, "{}", tool.name);
        for param in tool.params {
            let kind = match param.shape {
                Shape::Flag => "boolean",
                Shape::Text => "string",
                Shape::Count => "integer",
                Shape::List => "array",
            };
            assert_eq!(properties[param.name]["type"], kind, "{}.{}", tool.name, param.name);
        }
    }
}

/// A value of `param`'s shape that its flag accepts.
fn sample(command: &clap::Command, param: &Param) -> Value {
    let choice = param
        .id
        .and_then(|id| command.get_arguments().find(|arg| arg.get_id() == id))
        .and_then(|arg| {
            arg.get_possible_values()
                .first()
                .map(|value| value.get_name().to_owned())
        });
    match param.shape {
        Shape::Flag => Value::Bool(true),
        Shape::Text => json!(choice.unwrap_or_else(|| format!("{}-value", param.name))),
        Shape::Count => json!(3),
        Shape::List => json!([format!("{}-a", param.name), format!("{}-b", param.name)]),
    }
}

/// The arguments a call needs besides `param` for its command to accept it.
fn context(tool: &str, param: &str) -> Map<String, Value> {
    let needs = match (tool, param) {
        ("lint", "allow_remove_files" | "fix_type" | "select") => json!({ "fix": true }),
        ("explain_usage", "package") => json!({ "dependency": "react" }),
        ("explain_usage", "dependency") => json!({}),
        ("explain_usage", _) => json!({ "file": "src/a.ts" }),
        ("config", "package") => json!({ "explain": "lint" }),
        _ => json!({}),
    };
    needs.as_object().unwrap().clone()
}

/// `arguments` as a user types them after `skif` (and the subcommand): `--flag value`, positionals last.
fn typed(tool: &Tool, arguments: &Map<String, Value>) -> Vec<String> {
    let mut command = (tool.command)();
    command.build();
    let mut argv: Vec<String> = tool
        .fixed
        .iter()
        .chain((tool.implied)(arguments))
        .map(|flag| (*flag).to_owned())
        .collect();
    let mut positionals = Vec::new();
    for param in tool.params {
        let (Some(id), Some(value)) = (param.id, arguments.get(param.name)) else {
            continue;
        };
        let arg = command.get_arguments().find(|arg| arg.get_id() == id).unwrap();
        let values: Vec<String> = match value {
            Value::Bool(true) => vec![],
            Value::Array(items) => items.iter().map(|item| item.as_str().unwrap().to_owned()).collect(),
            Value::String(text) => vec![text.clone()],
            other => vec![other.to_string()],
        };
        match arg.get_long() {
            Some(long) if values.is_empty() => argv.push(format!("--{long}")),
            Some(long) => {
                for value in values {
                    argv.push(format!("--{long}"));
                    argv.push(value);
                }
            }
            None => positionals.extend(values.into_iter().map(|value| (arg.get_index().unwrap(), value))),
        }
    }
    positionals.sort();
    argv.extend(positionals.into_iter().map(|(_, value)| value));
    argv
}

/// The parsed arguments of the real command line, `skif SUBCOMMAND ARGV`, as the tool's command type prints them.
fn cli(tool: &Tool, typed: &[String]) -> String {
    use clap::Parser as _;
    let subcommand = COMMANDS.iter().find(|(_, served)| *served == Ok(tool.name)).unwrap().0;
    let full = std::iter::once("skif")
        .chain((!subcommand.is_empty()).then_some(subcommand))
        .map(str::to_owned)
        .chain(typed.iter().cloned());
    let parsed = crate::args::Cli::try_parse_from(full).unwrap_or_else(|error| panic!("{typed:?}: {error}"));
    match parsed.command {
        None => format!("{:?}", parsed.lint),
        Some(crate::args::Command::ExplainUsage(args)) => format!("{args:?}"),
        Some(crate::args::Command::Config(args)) => format!("{args:?}"),
        Some(crate::args::Command::Rules(args)) => format!("{args:?}"),
        Some(other) => panic!("unexpected {other:?}"),
    }
}

/// The arguments as the tool parses them, printed like [`cli`].
fn server(tool: &Tool, arguments: &Map<String, Value>) -> String {
    let parsed = match tool.name {
        "lint" => parse::<LintCommand>(tool, arguments).map(|command| format!("{:?}", command.args)),
        "explain_usage" => parse::<ExplainCommand>(tool, arguments).map(|command| format!("{:?}", command.args)),
        "config" => parse::<ConfigCommand>(tool, arguments).map(|command| format!("{:?}", command.args)),
        _ => parse::<RulesCommand>(tool, arguments).map(|command| format!("{:?}", command.args)),
    };
    parsed.unwrap_or_else(|failure| panic!("{}: {arguments:?}: {}", tool.name, failure.0))
}

#[test]
fn each_argument_means_what_its_flag_means() {
    for tool in TOOLS {
        let command = (tool.command)();
        let mut calls = vec![Map::new()];
        for param in tool.params.iter().filter(|param| param.id.is_some()) {
            let mut arguments = context(tool.name, param.name);
            arguments.insert(param.name.to_owned(), sample(&command, param));
            calls.push(arguments);
        }
        for arguments in calls {
            if tool.name == "explain_usage" && arguments.is_empty() {
                continue;
            }
            assert_eq!(
                server(tool, &arguments),
                cli(tool, &typed(tool, &arguments)),
                "{}: {arguments:?}",
                tool.name
            );
        }
    }
}

#[test]
fn export_joins_the_file() {
    let tool = tool("explain_usage");
    let arguments = json!({ "file": "src/a.ts", "export": "run" });
    assert_eq!(
        server(tool, arguments.as_object().unwrap()),
        cli(
            tool,
            &typed(tool, json!({ "file": "src/a.ts#run" }).as_object().unwrap())
        )
    );
}

#[test]
fn a_value_that_looks_like_a_flag_stays_a_value() {
    let tool = tool("lint");
    let arguments = json!({ "path": "-weird", "include": ["--fix"] });
    let printed = server(tool, arguments.as_object().unwrap());
    assert!(printed.contains("path: \"-weird\""), "{printed}");
    assert!(printed.contains("include: [\"--fix\"]"), "{printed}");
    assert!(printed.contains("fix: false"), "{printed}");
}
