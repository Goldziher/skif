//! `skif mcp`: a Model Context Protocol server on the stdio transport (ADR 0013).
//!
//! One JSON-RPC 2.0 message per line in each direction. Each tool stands for one command run with `--format json`:
//! its arguments are turned into that command's flags and parsed by the command line's own definition, and it
//! returns the document the command prints, as structured content and as compact text. A command that exits 2 gives
//! `isError`, with its `error` document or, when roots failed in a multi-root run, its report. [`TOOLS`] maps each
//! argument to its flag; the tests check the map against the command line.
//!
//! Messages are handled one at a time, in order: a `tools/call` blocks the server until its lint finishes, the
//! next message waits for it, and `notifications/cancelled` cannot stop a call already running. A line longer than
//! [`MAX_LINE`] is refused with `-32600` without being buffered, and the server goes on with the next line.
//!
//! `lint` keeps one in-memory module cache per canonical `path` and `recursive` pair (ADR 0007), its misses read
//! from the cache store, which the server never writes, and pruned after each call to what it used, so a repeated
//! call parses only the files that changed. The other tools are uncached.

use std::collections::BTreeMap;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

use clap::{CommandFactory, Parser};
use serde_json::{Map, Value, json};

use crate::args::{CacheCleanArgs, CacheStatsArgs, ConfigArgs, ExplainUsageArgs, Format, LintArgs, RulesArgs};
use crate::cache::{CacheEnv, CacheHost, SeededHost};
use crate::error::{CliError, Kind};
use crate::explain::{self, Failure};
use crate::lint::{canonical, run_captured};
use crate::output::{error_document, stderr_line};

/// The protocol revisions this server speaks, newest first; the first is offered when the client asks for another.
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
/// The longest message line accepted; a longer one is skipped up to its newline instead of being buffered.
const MAX_LINE: usize = 64 * 1024 * 1024;
/// The exit code of a command that could not run.
const COULD_NOT_RUN: u8 = 2;

const INSTRUCTIONS: &str = "skif lints JavaScript and TypeScript monorepos: workspace and manifest rules, version \
policies, unused dependencies, files and exports. Each tool returns the JSON of the `skif` command it names. An \
argument is the flag of the same name (`max_show_issues` is `--max-show-issues`); errors name the flags; paths are \
relative to the server's working directory. No project file is written: apply fixes with `skif --fix`; \
`cache_clean` removes only skif's own cache.";

/// `skif [PATH]` as its own command, for parsing a lint tool's flags.
#[derive(Debug, Parser)]
#[command(name = "skif", no_binary_name = true)]
struct LintCommand {
    #[command(flatten)]
    args: LintArgs,
}

/// `skif explain-usage` as its own command.
#[derive(Debug, Parser)]
#[command(name = "skif explain-usage", no_binary_name = true)]
struct ExplainCommand {
    #[command(flatten)]
    args: ExplainUsageArgs,
}

/// `skif config` as its own command.
#[derive(Debug, Parser)]
#[command(name = "skif config", no_binary_name = true)]
struct ConfigCommand {
    #[command(flatten)]
    args: ConfigArgs,
}

/// `skif rules` as its own command.
#[derive(Debug, Parser)]
#[command(name = "skif rules", no_binary_name = true)]
struct RulesCommand {
    #[command(flatten)]
    args: RulesArgs,
}

/// `skif cache stats` as its own command.
#[derive(Debug, Parser)]
#[command(name = "skif cache stats", no_binary_name = true)]
struct CacheStatsCommand {
    #[command(flatten)]
    args: CacheStatsArgs,
}

/// `skif cache clean` as its own command.
#[derive(Debug, Parser)]
#[command(name = "skif cache clean", no_binary_name = true)]
struct CacheCleanCommand {
    #[command(flatten)]
    args: CacheCleanArgs,
}

/// How a tool argument is written on the command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A boolean: `true` gives the flag.
    Flag,
    /// A string: one value.
    Text,
    /// A non-negative integer: one value.
    Count,
    /// An array of strings: the flag once per item.
    List,
}

/// One argument of a tool.
struct Param {
    name: &'static str,
    /// The clap id of the command-line argument it stands for; `None` for an argument the tool folds into another
    /// (`export` joins `file` as `FILE#EXPORT`).
    id: Option<&'static str>,
    shape: Shape,
    description: &'static str,
}

const fn param(name: &'static str, shape: Shape, description: &'static str) -> Param {
    Param {
        name,
        id: Some(name),
        shape,
        description,
    }
}

/// One tool: the command it runs and the flags its arguments stand for.
struct Tool {
    name: &'static str,
    /// The human-readable name `tools/list` gives (MCP 2025-06-18).
    title: &'static str,
    description: &'static str,
    /// The command whose flags the arguments are parsed as.
    command: fn() -> clap::Command,
    /// The flags every call passes: JSON output, and what a long-lived host fixes (no types, ADR 0013).
    fixed: &'static [&'static str],
    params: &'static [Param],
    /// The flags the tool sets itself, depending on the call: `lint` sets `--dry-run` once the parsed flags fix
    /// ([`LintArgs::fixing`]), and `config` passes `--show` unless `explain` is given, since its command line needs
    /// one of the two before it parses.
    implied: &'static [&'static str],
    /// What a call does to the machine: every tool states it, so a tool added later cannot pass as read-only.
    effect: Effect,
}

/// What a call of a tool does outside the server, as its `annotations` tell the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Effect {
    /// Changes nothing: no project file, and the cache store is read at most.
    ReadOnly,
    /// Deletes files.
    Destructive,
}

impl Effect {
    /// The tool annotations of MCP: no tool reaches beyond the machine, and repeating a call changes nothing more.
    fn annotations(self) -> Value {
        let read_only = self == Self::ReadOnly;
        json!({
            "readOnlyHint": read_only,
            "destructiveHint": !read_only,
            "idempotentHint": true,
            "openWorldHint": false,
        })
    }
}

const PATH: Param = param("path", Shape::Text, "A directory or file in the project; default `.`.");
const RECURSIVE: Param = param("recursive", Shape::Flag, "Every root at or below `path`.");
const CONFIG: Param = param("config", Shape::Text, "This config file instead of discovered ones.");
const NO_CONFIG: Param = param("no_config", Shape::Flag, "Read no config file.");
const INCLUDE: Param = param(
    "include",
    Shape::List,
    "Only these rule IDs or knip issue types; enables off rules.",
);
const EXCLUDE: Param = param("exclude", Shape::List, "Not these rule IDs or knip issue types.");
const WORKSPACE: Param = param(
    "workspace",
    Shape::List,
    "Only findings of these packages: name, directory or glob; `!` negates.",
);
const PRODUCTION: Param = param("production", Shape::Flag, "Production code only.");
const STRICT: Param = param("strict", Shape::Flag, "knip's strict mode; implies `production`.");
const NO_JS: Param = param("no_js", Shape::Flag, "Skip the JavaScript analysis.");

const CACHE_LOCATION: Param = param(
    "cache_location",
    Shape::Text,
    "This cache directory instead of the one a lint of `path` uses.",
);

/// The tools, in the order `tools/list` gives them.
const TOOLS: &[Tool] = &[
    Tool {
        name: "lint",
        title: "Lint a monorepo",
        description: "`skif --format json`: the lint report. Findings are not an error.",
        command: LintCommand::command,
        fixed: &["--format", "json"],
        params: &[
            PATH,
            RECURSIVE,
            CONFIG,
            NO_CONFIG,
            INCLUDE,
            EXCLUDE,
            WORKSPACE,
            PRODUCTION,
            STRICT,
            NO_JS,
            param(
                "max_show_issues",
                Shape::Count,
                "At most N findings per rule; `summary` counts all.",
            ),
            param(
                "fix",
                Shape::Flag,
                "Plan safe fixes (`--dry-run`): adds `fix.changes`, unified diffs.",
            ),
            param("fix_unsafe", Shape::Flag, "Plan unsafe fixes too."),
            param(
                "fix_type",
                Shape::List,
                "Only the fixes of these rule IDs or issue types.",
            ),
            param("select", Shape::Text, "The version alignment fixes target."),
            param("allow_remove_files", Shape::Flag, "Plan file deletions."),
        ],
        implied: &["--dry-run"],
        effect: Effect::ReadOnly,
    },
    Tool {
        name: "explain_usage",
        title: "Explain why a file, export or dependency is used",
        description: "`skif explain-usage --format json`: why a file, export or dependency counts as used.",
        command: ExplainCommand::command,
        fixed: &["--format", "json"],
        params: &[
            Param {
                name: "file",
                id: Some("target"),
                shape: Shape::Text,
                description: "The source file.",
            },
            Param {
                name: "export",
                id: None,
                shape: Shape::Text,
                description: "One export of `file`.",
            },
            param("dependency", Shape::Text, "Explain this dependency instead of a file."),
            param(
                "package",
                Shape::Text,
                "With `dependency`: this package only (name or directory).",
            ),
            param("path", Shape::Text, "A directory in the project; default `.`."),
            param("production", Shape::Flag, "The production-only analysis."),
            CONFIG,
            NO_CONFIG,
        ],
        implied: &[],
        effect: Effect::ReadOnly,
    },
    Tool {
        name: "config",
        title: "Show the effective configuration",
        description: "`skif config --format json`: the config files of each root and the merged config.",
        command: ConfigCommand::command,
        fixed: &["--format", "json"],
        params: &[
            PATH,
            RECURSIVE,
            param(
                "explain",
                Shape::Text,
                "Explain this key (`rules.ID.level`): its assignments and final value.",
            ),
            param("package", Shape::Text, "With `explain`: this package's overrides too."),
            CONFIG,
            NO_CONFIG,
        ],
        implied: &["--show"],
        effect: Effect::ReadOnly,
    },
    Tool {
        name: "list_rules",
        title: "List skif's rules",
        description: "`skif rules --format json`: every rule with its ID, default level and fixability.",
        command: RulesCommand::command,
        fixed: &["--format", "json"],
        params: &[],
        implied: &[],
        effect: Effect::ReadOnly,
    },
    Tool {
        name: "cache_stats",
        title: "Show skif's cache",
        description: "`skif cache stats --format json`: the cache location and the stores in it. Writes nothing.",
        command: CacheStatsCommand::command,
        fixed: &["--format", "json"],
        params: &[PATH, CACHE_LOCATION],
        implied: &[],
        effect: Effect::ReadOnly,
    },
    Tool {
        name: "cache_clean",
        title: "Remove skif's cache",
        description: "`skif cache clean --format json`: removes the cache store of the project; the next lint \
                      rebuilds it. Touches no project file.",
        command: CacheCleanCommand::command,
        fixed: &["--format", "json"],
        params: &[
            PATH,
            CACHE_LOCATION,
            param("all", Shape::Flag, "Every project's store in the cache location."),
        ],
        implied: &[],
        effect: Effect::Destructive,
    },
];

/// Serves stdin and stdout until stdin closes; returns the exit code.
pub fn run() -> u8 {
    match serve(io::stdin().lock(), io::stdout().lock()) {
        Ok(()) => 0,
        Err(error) => {
            stderr_line(&format!("error: skif mcp: {error}"));
            2
        }
    }
}

/// Answers every message read from `input` on `output`, one line each, until `input` ends. A reader that went
/// away ends the loop without an error.
///
/// # Errors
/// An error reading `input` or writing `output`.
pub fn serve(input: impl BufRead, output: impl Write) -> io::Result<()> {
    serve_limited(input, output, MAX_LINE)
}

/// [`serve`] with lines longer than `limit` bytes refused, so a test can exercise the cap with a small one.
fn serve_limited(mut input: impl BufRead, mut output: impl Write, limit: usize) -> io::Result<()> {
    let mut caches = Caches::new();
    loop {
        let mut bytes = Vec::new();
        let cap = u64::try_from(limit).unwrap_or(u64::MAX).saturating_add(1);
        if io::Read::take(&mut input, cap).read_until(b'\n', &mut bytes)? == 0 {
            return Ok(());
        }
        let reply = if bytes.len() > limit && bytes.last() != Some(&b'\n') {
            skip_line(&mut input)?;
            Some(error_reply(
                &Value::Null,
                RpcError(INVALID_REQUEST, format!("a message longer than {limit} bytes")),
            ))
        } else {
            let line = String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            if line.trim().is_empty() {
                continue;
            }
            let reply = handle_line(&line, &mut caches);
            // The server runs until its client closes stdin, so the timings of each request are written once it is
            // answered.
            skif_core::timing::flush_stderr();
            reply
        };
        let Some(reply) = reply else {
            continue;
        };
        let mut bytes = serde_json::to_vec(&reply).map_err(io::Error::other)?;
        bytes.push(b'\n');
        match output.write_all(&bytes).and_then(|()| output.flush()) {
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            result => result?,
        }
    }
}

/// Discards `input` up to and including the next newline, or to its end, without buffering it.
fn skip_line(input: &mut impl BufRead) -> io::Result<()> {
    loop {
        let buffer = input.fill_buf()?;
        if buffer.is_empty() {
            return Ok(());
        }
        if let Some(newline) = buffer.iter().position(|&byte| byte == b'\n') {
            input.consume(newline + 1);
            return Ok(());
        }
        let length = buffer.len();
        input.consume(length);
    }
}

/// A JSON-RPC error: code and message.
struct RpcError(i64, String);

impl RpcError {
    fn params(message: impl Into<String>) -> Self {
        Self(INVALID_PARAMS, message.into())
    }
}

fn error_reply(id: &Value, RpcError(code, message): RpcError) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// The module caches of the `lint` tool, by canonical `path` and `recursive`: in memory, their misses read from the
/// cache store, which the server never writes.
type Caches = BTreeMap<(PathBuf, bool), Arc<SeededHost>>;

/// The reply to one line, or `None` for a notification or a response.
fn handle_line(line: &str, caches: &mut Caches) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(message) => message,
        Err(error) => {
            return Some(error_reply(
                &Value::Null,
                RpcError(PARSE_ERROR, format!("parse error: {error}")),
            ));
        }
    };
    let Value::Object(message) = message else {
        return Some(error_reply(
            &Value::Null,
            RpcError(
                INVALID_REQUEST,
                "expected one JSON-RPC message object; batches are not supported".to_owned(),
            ),
        ));
    };
    let id = message.get("id").cloned();
    let response = message.contains_key("result") || message.contains_key("error");
    if !response
        && id
            .as_ref()
            .is_some_and(|id| !(id.is_string() || id.is_number() || id.is_null()))
    {
        return Some(error_reply(
            &Value::Null,
            RpcError(INVALID_REQUEST, "`id` must be a string, a number or null".to_owned()),
        ));
    }
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(error_reply(
            id.as_ref().unwrap_or(&Value::Null),
            RpcError(INVALID_REQUEST, "`jsonrpc` must be \"2.0\"".to_owned()),
        ));
    }
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // A response to a request this server never sends, or a malformed message: a request without a method
        // still gets an answer.
        return match id {
            Some(id) if !response => Some(error_reply(
                &id,
                RpcError(INVALID_REQUEST, "a request needs a `method`".to_owned()),
            )),
            _ => None,
        };
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    // Notifications (`notifications/initialized`, `notifications/cancelled`, ...) need no answer.
    let id = id?;
    Some(match dispatch(method, &params, caches) {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => error_reply(&id, error),
    })
}

fn dispatch(method: &str, params: &Value, caches: &mut Caches) -> Result<Value, RpcError> {
    match method {
        "initialize" => Ok(initialize(params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => call(params, caches),
        other => Err(RpcError(METHOD_NOT_FOUND, format!("method not found: {other}"))),
    }
}

/// The client's protocol version when this server speaks it, else the newest one it does.
fn negotiate(requested: Option<&str>) -> &'static str {
    PROTOCOL_VERSIONS
        .iter()
        .find(|version| Some(**version) == requested)
        .copied()
        .unwrap_or(PROTOCOL_VERSIONS[0])
}

fn initialize(params: &Value) -> Value {
    json!({
        "protocolVersion": negotiate(params.get("protocolVersion").and_then(Value::as_str)),
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "skif", "title": "skif", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

/// The `tools/list` entries. Each argument's type comes from its shape, and its allowed values from the command
/// line's definition.
fn tools() -> Value {
    Value::Array(
        TOOLS
            .iter()
            .map(|tool| {
                let command = (tool.command)();
                let properties: Map<String, Value> = tool
                    .params
                    .iter()
                    .map(|param| (param.name.to_owned(), param_schema(&command, param)))
                    .collect();
                json!({
                    "name": tool.name,
                    "title": tool.title,
                    "description": tool.description,
                    "inputSchema": { "type": "object", "properties": properties, "additionalProperties": false },
                    "annotations": tool.effect.annotations(),
                })
            })
            .collect(),
    )
}

fn param_schema(command: &clap::Command, param: &Param) -> Value {
    let mut schema = match param.shape {
        Shape::Flag => json!({ "type": "boolean" }),
        Shape::Text => json!({ "type": "string" }),
        Shape::Count => json!({ "type": "integer", "minimum": 0 }),
        Shape::List => json!({ "type": "array", "items": { "type": "string" } }),
    };
    let choices: Vec<String> = param
        .id
        .and_then(|id| command.get_arguments().find(|arg| arg.get_id() == id))
        .map_or_else(Vec::new, |arg| {
            arg.get_possible_values()
                .iter()
                .filter(|value| !value.is_hide_set())
                .map(|value| value.get_name().to_owned())
                .collect()
        });
    if !choices.is_empty() && param.shape != Shape::Flag {
        schema["enum"] = json!(choices);
    }
    schema["description"] = json!(param.description);
    schema
}

fn call(params: &Value, caches: &mut Caches) -> Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::params("`tools/call` needs a tool `name`"))?;
    let empty = Map::new();
    let arguments = match params.get("arguments") {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(arguments)) => arguments,
        Some(_) => return Err(RpcError::params("`arguments` must be an object")),
    };
    let tool = TOOLS
        .iter()
        .find(|tool| tool.name == name)
        .ok_or_else(|| RpcError::params(format!("unknown tool: {name}")))?;
    if let Some(key) = arguments
        .keys()
        .find(|key| !tool.params.iter().any(|param| param.name == key.as_str()))
    {
        return Err(RpcError::params(format!("unknown argument `{key}`")));
    }
    let outcome = match tool.name {
        "lint" => lint(tool, arguments, caches),
        "explain_usage" => explain_usage(tool, arguments),
        "config" => config(tool, arguments),
        "cache_stats" => cache_stats(tool, arguments),
        "cache_clean" => cache_clean(tool, arguments),
        _ => list_rules(tool, arguments),
    };
    Ok(tool_result(outcome))
}

/// The command line `arguments` stand for: the tool's fixed flags and `extra`, then one flag per argument given, then
/// the positional arguments after `--`, in their order. Values are attached with `=`, so one that starts with `-` is
/// not read as a flag.
fn command_line(tool: &Tool, arguments: &Map<String, Value>, extra: &[&str]) -> Result<Vec<String>, Failure> {
    let mut command = (tool.command)();
    // Building numbers the positional arguments.
    command.build();
    let mut argv: Vec<String> = tool.fixed.iter().chain(extra).map(|flag| (*flag).to_owned()).collect();
    let mut positionals: Vec<(usize, String)> = Vec::new();
    for param in tool.params {
        let Some(id) = param.id else {
            continue;
        };
        let Some(value) = arguments.get(param.name).filter(|value| !value.is_null()) else {
            continue;
        };
        let arg = command
            .get_arguments()
            .find(|arg| arg.get_id() == id)
            .ok_or_else(|| plain(CliError::new(Kind::Internal, format!("`{}` has no flag", param.name))))?;
        let values = match (param.shape, value) {
            (Shape::Flag, Value::Bool(false)) => Vec::new(),
            (Shape::Flag, Value::Bool(true)) => {
                argv.push(format!("--{}", arg.get_long().unwrap_or(id)));
                continue;
            }
            (Shape::Text, Value::String(text)) => vec![text.clone()],
            (Shape::Count, Value::Number(number)) if number.is_u64() => vec![number.to_string()],
            (Shape::List, Value::Array(items)) if items.iter().all(Value::is_string) => {
                items.iter().filter_map(Value::as_str).map(str::to_owned).collect()
            }
            (shape, _) => {
                let expected = match shape {
                    Shape::Flag => "a boolean",
                    Shape::Text => "a string",
                    Shape::Count => "a non-negative integer",
                    Shape::List => "an array of strings",
                };
                return Err(usage(format!("`{}` must be {expected}", param.name)));
            }
        };
        match (arg.get_long(), arg.get_index()) {
            (Some(long), _) => argv.extend(values.into_iter().map(|value| format!("--{long}={value}"))),
            (None, index) => positionals.extend(values.into_iter().map(|value| (index.unwrap_or(usize::MAX), value))),
        }
    }
    if !positionals.is_empty() {
        positionals.sort_by_key(|(index, _)| *index);
        argv.push("--".to_owned());
        argv.extend(positionals.into_iter().map(|(_, value)| value));
    }
    Ok(argv)
}

/// `arguments` with `export` joined to `file` as the command line's one `FILE#EXPORT`.
fn folded(arguments: &Map<String, Value>) -> Result<Map<String, Value>, Failure> {
    let mut arguments = arguments.clone();
    match arguments.remove("export") {
        None | Some(Value::Null) => {}
        Some(Value::String(export)) => match arguments.get_mut("file") {
            Some(Value::String(file)) => {
                file.push('#');
                file.push_str(&export);
            }
            _ => return Err(usage("`export` needs `file`")),
        },
        Some(_) => return Err(usage("`export` must be a string")),
    }
    Ok(arguments)
}

/// The arguments of `tool` parsed as its command's flags, with `extra` flags: a flag the command line rejects is a
/// usage error with clap's message.
fn parse<C: Parser>(tool: &Tool, arguments: &Map<String, Value>, extra: &[&str]) -> Result<C, Failure> {
    let argv = command_line(tool, &folded(arguments)?, extra)?;
    let mut command = (tool.command)();
    command
        .try_get_matches_from_mut(argv)
        .and_then(|mut matches| C::from_arg_matches_mut(&mut matches))
        .map_err(|error| usage(clap_message(&error)))
}

/// The first line of a clap error, without its `error: ` prefix.
fn clap_message(error: &clap::Error) -> String {
    let rendered = error.render().to_string();
    let first = rendered.lines().next().unwrap_or_default();
    first.strip_prefix("error: ").unwrap_or(first).to_owned()
}

fn plain(error: CliError) -> Failure {
    Box::new((error, Map::new()))
}

/// A malformed argument found while running a tool: reported in the result, so the agent can correct the call.
fn usage(message: impl Into<String>) -> Failure {
    plain(CliError::new(Kind::Usage, message))
}

/// The document a command printed, parsed.
fn parsed(text: &[u8]) -> Result<Value, Failure> {
    serde_json::from_slice(text).map_err(|error| plain(CliError::new(Kind::Internal, error.to_string())))
}

/// A tool's document, and whether its command exited 2.
type Answer = (Value, bool);

/// A `tools/call` result: the document as structured content and as compact text, flagged when the command could not
/// run.
fn tool_result(outcome: Result<Answer, Failure>) -> Value {
    let (document, is_error) = outcome.unwrap_or_else(|failure| {
        let (error, extra) = *failure;
        let text = error_document(&error, extra).unwrap_or_default();
        let document = serde_json::from_str(&text).unwrap_or_else(|_| json!({ "error": { "message": error.message } }));
        (document, true)
    });
    let text = serde_json::to_string(&document).unwrap_or_default();
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": document,
        "isError": is_error,
    })
}

/// The flags of a `lint` call. The tool never writes: a fix is only planned (`--dry-run`).
fn lint_args(tool: &Tool, arguments: &Map<String, Value>) -> Result<LintArgs, Failure> {
    let mut args = parse::<LintCommand>(tool, arguments, &[])?.args;
    args.dry_run = args.fixing();
    Ok(args)
}

/// The flags of a `config` call: `skif config` shows the configuration (`--show`) when no key is to be explained.
fn config_args(tool: &Tool, arguments: &Map<String, Value>) -> Result<ConfigArgs, Failure> {
    let show = if arguments.get("explain").is_none_or(Value::is_null) {
        tool.implied
    } else {
        &[]
    };
    Ok(parse::<ConfigCommand>(tool, arguments, show)?.args)
}

/// `lint`: `skif` with the tool's flags.
fn lint(tool: &Tool, arguments: &Map<String, Value>, caches: &mut Caches) -> Result<Answer, Failure> {
    let args = lint_args(tool, arguments)?;
    // A path that does not resolve gets no cache; the lint reports why it cannot run.
    let cache = canonical(&args.path).ok().map(|start| {
        Arc::clone(
            caches
                .entry((start, args.recursive))
                .or_insert_with(|| Arc::new(SeededHost::new(CacheEnv::detect(false)))),
        )
    });
    let (stdout, code) =
        run_captured(&args, cache.as_ref().map(|cache| cache.as_ref() as &dyn CacheHost)).map_err(plain)?;
    Ok((parsed(&stdout)?, code == COULD_NOT_RUN))
}

fn explain_usage(tool: &Tool, arguments: &Map<String, Value>) -> Result<Answer, Failure> {
    let args = parse::<ExplainCommand>(tool, arguments, &[])?.args;
    let cwd = std::env::current_dir().map_err(|error| {
        plain(CliError::new(
            Kind::Io,
            format!("cannot read the current directory: {error}"),
        ))
    })?;
    let text = explain::render(&args, &cwd, Format::Json)?;
    Ok((parsed(text.as_bytes())?, false))
}

fn config(tool: &Tool, arguments: &Map<String, Value>) -> Result<Answer, Failure> {
    let args = config_args(tool, arguments)?;
    let (text, failures) = crate::config::render(&args).map_err(plain)?;
    Ok((parsed(text.as_bytes())?, !failures.is_empty()))
}

fn list_rules(tool: &Tool, arguments: &Map<String, Value>) -> Result<Answer, Failure> {
    let args = parse::<RulesCommand>(tool, arguments, &[])?.args;
    let text = crate::commands::render(&args).map_err(plain)?;
    Ok((parsed(text.as_bytes())?, false))
}

fn cache_stats(tool: &Tool, arguments: &Map<String, Value>) -> Result<Answer, Failure> {
    let args = parse::<CacheStatsCommand>(tool, arguments, &[])?.args;
    let text = crate::cache_command::stats(&args).map_err(plain)?;
    Ok((parsed(text.as_bytes())?, false))
}

fn cache_clean(tool: &Tool, arguments: &Map<String, Value>) -> Result<Answer, Failure> {
    let args = parse::<CacheCleanCommand>(tool, arguments, &[])?.args;
    let text = crate::cache_command::clean(&args).map_err(plain)?;
    Ok((parsed(text.as_bytes())?, false))
}

#[cfg(test)]
mod parity;

#[cfg(test)]
mod tests {
    use super::*;

    fn exchange(lines: &[Value]) -> Vec<Value> {
        let input: String = lines.iter().map(|line| line.to_string() + "\n").collect();
        let mut output = Vec::new();
        serve(input.as_bytes(), &mut output).unwrap();
        String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn the_version_is_negotiated() {
        assert_eq!(negotiate(Some("2025-06-18")), "2025-06-18");
        assert_eq!(negotiate(Some("2024-11-05")), "2024-11-05");
        assert_eq!(negotiate(Some("1999-01-01")), PROTOCOL_VERSIONS[0]);
        assert_eq!(negotiate(None), PROTOCOL_VERSIONS[0]);
    }

    #[test]
    fn notifications_and_responses_get_no_answer() {
        let replies = exchange(&[
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 9, "result": {} }),
            json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }),
        ]);
        assert_eq!(replies, vec![json!({ "jsonrpc": "2.0", "id": 1, "result": {} })]);
    }

    #[test]
    fn malformed_messages_get_json_rpc_errors() {
        let input =
            "{not json\n[]\n{\"jsonrpc\":\"1.0\",\"id\":2,\"method\":\"ping\"}\n{\"jsonrpc\":\"2.0\",\"id\":3}\n";
        let mut output = Vec::new();
        serve(input.as_bytes(), &mut output).unwrap();
        let codes: Vec<(Value, i64)> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| {
                let reply: Value = serde_json::from_str(line).unwrap();
                (reply["id"].clone(), reply["error"]["code"].as_i64().unwrap())
            })
            .collect();
        assert_eq!(
            codes,
            vec![
                (Value::Null, PARSE_ERROR),
                (Value::Null, INVALID_REQUEST),
                (json!(2), INVALID_REQUEST),
                (json!(3), INVALID_REQUEST),
            ]
        );
    }

    #[test]
    fn an_id_that_is_not_a_string_number_or_null_is_an_invalid_request() {
        let replies = exchange(&[
            json!({ "jsonrpc": "2.0", "id": { "a": 1 }, "method": "ping" }),
            json!({ "jsonrpc": "2.0", "id": [1], "method": "ping" }),
            json!({ "jsonrpc": "2.0", "id": true, "method": "ping" }),
            json!({ "jsonrpc": "2.0", "id": "x", "method": "ping" }),
        ]);
        let error = json!({ "jsonrpc": "2.0", "id": null, "error": {
            "code": INVALID_REQUEST, "message": "`id` must be a string, a number or null"
        } });
        assert_eq!(replies[..3], [error.clone(), error.clone(), error]);
        assert_eq!(replies[3], json!({ "jsonrpc": "2.0", "id": "x", "result": {} }));
    }

    #[test]
    fn a_line_above_the_cap_is_refused_and_the_next_line_is_served() {
        let long = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\",\"pad\":\"{}\"}}",
            "x".repeat(200)
        );
        let input = format!("{long}\n{}\n", json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" }));
        let mut output = Vec::new();
        serve_limited(input.as_bytes(), &mut output, 64).unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies.len(), 2, "{replies:?}");
        assert_eq!(replies[0]["id"], Value::Null);
        assert_eq!(replies[0]["error"]["code"], INVALID_REQUEST);
        assert_eq!(replies[1], json!({ "jsonrpc": "2.0", "id": 2, "result": {} }));
    }

    #[test]
    fn bad_arguments_are_invalid_params_or_tool_errors() {
        let replies = exchange(&[
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "fix" } }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": "lint", "arguments": { "bogus": 1 } } }),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "lint", "arguments": { "recursive": "yes" } } }),
        ]);
        assert_eq!(replies[0]["error"]["code"], INVALID_PARAMS);
        assert_eq!(replies[1]["error"]["code"], INVALID_PARAMS);
        assert_eq!(replies[2]["result"]["isError"], true);
        assert_eq!(replies[2]["result"]["structuredContent"]["error"]["kind"], "usage");
    }
}
