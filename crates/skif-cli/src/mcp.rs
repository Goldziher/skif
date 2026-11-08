//! `skif mcp`: a Model Context Protocol server on the stdio transport (ADR 0056).
//!
//! One JSON-RPC 2.0 message per line in each direction. The tools return the same documents the commands print
//! with `--format json`; a tool that cannot run returns the `error` document with `isError`, as the protocol asks.

use std::io::{self, BufRead, Write};
use std::path::Path;

use serde_json::{Map, Value, json};
use skif::ConfigOverrides;

use crate::commands::rules_document;
use crate::error::{CliError, Kind};
use crate::explain::{self, Failure};
use crate::lint::{canonical, lint_to_json};
use crate::output::{error_document, stderr_line};

/// The protocol revisions this server speaks, newest first; the first is offered when the client asks for another.
const PROTOCOL_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

const INSTRUCTIONS: &str = "skif lints JavaScript and TypeScript monorepos: workspace and manifest rules, version \
policies and unused or unlisted dependencies, files and exports. Call `lint` with the project directory to get the \
report, `explain_usage` to learn why a file or export counts as used, and `list_rules` for the rule IDs. Nothing is \
written; run `skif --fix` to apply fixes.";

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
pub fn serve(input: impl BufRead, mut output: impl Write) -> io::Result<()> {
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Some(reply) = handle_line(&line) else {
            continue;
        };
        let mut bytes = serde_json::to_vec(&reply).map_err(io::Error::other)?;
        bytes.push(b'\n');
        match output.write_all(&bytes).and_then(|()| output.flush()) {
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            result => result?,
        }
    }
    Ok(())
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

/// The reply to one line, or `None` for a notification or a response.
fn handle_line(line: &str) -> Option<Value> {
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
            Some(id) if !message.contains_key("result") && !message.contains_key("error") => Some(error_reply(
                &id,
                RpcError(INVALID_REQUEST, "a request needs a `method`".to_owned()),
            )),
            _ => None,
        };
    };
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    // Notifications (`notifications/initialized`, `notifications/cancelled`, ...) need no answer.
    let id = id?;
    Some(match dispatch(method, &params) {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(error) => error_reply(&id, error),
    })
}

fn dispatch(method: &str, params: &Value) -> Result<Value, RpcError> {
    match method {
        "initialize" => Ok(initialize(params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools() })),
        "tools/call" => call(params),
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

fn tools() -> Value {
    let read_only =
        json!({ "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false });
    json!([
        {
            "name": "lint",
            "title": "Lint a monorepo",
            "description": "Lint the workspace containing `path` and return skif's JSON report: the summary and every \
    diagnostic with its rule, severity, message, help, file location and available fixes. Problems in the project are \
    the report, not an error.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "A directory or file inside the project; relative to the server's working directory. Defaults to `.`." },
                    "recursive": { "type": "boolean", "description": "Lint every workspace and standalone package at or below `path`." },
                    "rules": { "type": "array", "items": { "type": "string" }, "description": "Run only these rule IDs (see `list_rules`)." },
                    "production": { "type": "boolean", "description": "Analyse production code only." },
                    "no_js": { "type": "boolean", "description": "Skip the JavaScript analysis." }
                },
                "additionalProperties": false
            },
            "annotations": read_only,
        },
        {
            "name": "explain_usage",
            "title": "Explain why a file or export is used",
            "description": "Explain why the JavaScript analysis counts a file, or one export of it, as used or unused: \
    the entry that reaches it, its importers and the verdict.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "The source file; relative to `path` unless absolute." },
                    "export": { "type": "string", "description": "One export of the file to explain." },
                    "path": { "type": "string", "description": "A directory inside the project; relative to the server's working directory. Defaults to `.`." },
                    "production": { "type": "boolean", "description": "Explain the production-only analysis." }
                },
                "required": ["file"],
                "additionalProperties": false
            },
            "annotations": read_only,
        },
        {
            "name": "list_rules",
            "title": "List skif's rules",
            "description": "List every rule with its ID, category, default level, fixability and summary.",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false },
            "annotations": read_only,
        }
    ])
}

fn call(params: &Value) -> Result<Value, RpcError> {
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
    let outcome = match name {
        "lint" => lint(&Arguments::new(
            arguments,
            &["path", "recursive", "rules", "production", "no_js"],
        )?),
        "explain_usage" => explain_usage(&Arguments::new(arguments, &["file", "export", "path", "production"])?),
        "list_rules" => {
            Arguments::new(arguments, &[])?;
            rules_document().map_err(plain)
        }
        other => return Err(RpcError::params(format!("unknown tool: {other}"))),
    };
    Ok(tool_result(outcome))
}

/// A tool's arguments, checked against the names it accepts.
struct Arguments<'a>(&'a Map<String, Value>);

impl<'a> Arguments<'a> {
    fn new(map: &'a Map<String, Value>, known: &[&str]) -> Result<Self, RpcError> {
        match map.keys().find(|key| !known.contains(&key.as_str())) {
            Some(key) => Err(RpcError::params(format!("unknown argument `{key}`"))),
            None => Ok(Self(map)),
        }
    }

    fn string(&self, key: &str) -> Result<Option<&'a str>, RpcError> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text)),
            Some(_) => Err(RpcError::params(format!("`{key}` must be a string"))),
        }
    }

    fn flag(&self, key: &str) -> Result<bool, RpcError> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(false),
            Some(Value::Bool(value)) => Ok(*value),
            Some(_) => Err(RpcError::params(format!("`{key}` must be a boolean"))),
        }
    }

    fn strings(&self, key: &str) -> Result<Option<Vec<&'a str>>, RpcError> {
        let invalid = || RpcError::params(format!("`{key}` must be an array of strings"));
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| item.as_str().ok_or_else(invalid))
                .collect::<Result<_, _>>()
                .map(Some),
            Some(_) => Err(invalid()),
        }
    }
}

fn plain(error: CliError) -> Failure {
    Box::new((error, Map::new()))
}

/// A `tools/call` result: the document as structured content and as text, flagged when it is an `error` document.
fn tool_result(outcome: Result<Value, Failure>) -> Value {
    let (document, is_error) = match outcome {
        Ok(document) => (document, false),
        Err(failure) => {
            let (error, extra) = *failure;
            let text = error_document(&error, extra).unwrap_or_default();
            let document =
                serde_json::from_str(&text).unwrap_or_else(|_| json!({ "error": { "message": error.message } }));
            (document, true)
        }
    };
    let text = serde_json::to_string_pretty(&document).unwrap_or_default();
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": document,
        "isError": is_error,
    })
}

fn lint(arguments: &Arguments<'_>) -> Result<Value, Failure> {
    let path = arguments.string("path").map_err(usage)?.unwrap_or(".");
    let recursive = arguments.flag("recursive").map_err(usage)?;
    let only = arguments.strings("rules").map_err(usage)?;
    let mut overrides = ConfigOverrides::default()
        .with_production(arguments.flag("production").map_err(usage)?.then_some(true))
        .with_no_js(arguments.flag("no_js").map_err(usage)?);
    if let Some(only) = only {
        let rules = rules_document().map_err(plain)?;
        let known: Vec<&str> = rules["rules"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|rule| rule["id"].as_str())
            .collect();
        if let Some(unknown) = only.iter().find(|id| !known.contains(id)) {
            return Err(plain(CliError::new(
                Kind::Usage,
                format!("unknown rule `{unknown}`; `list_rules` lists the rule IDs"),
            )));
        }
        let ignored: Vec<String> = known
            .iter()
            .filter(|id| !only.contains(id))
            .map(|id| (*id).to_owned())
            .collect();
        overrides = overrides.with_ignore_rules(ignored);
    }
    let text = lint_to_json(Path::new(path), &overrides, recursive).map_err(plain)?;
    serde_json::from_str(&text).map_err(|error| plain(CliError::new(Kind::Internal, error.to_string())))
}

fn explain_usage(arguments: &Arguments<'_>) -> Result<Value, Failure> {
    let file = arguments
        .string("file")
        .map_err(usage)?
        .ok_or_else(|| plain(CliError::new(Kind::Usage, "`file` is required")))?;
    let export = arguments
        .string("export")
        .map_err(usage)?
        .filter(|export| !export.is_empty());
    let start = canonical(Path::new(arguments.string("path").map_err(usage)?.unwrap_or("."))).map_err(plain)?;
    let file = explain::resolve_file(&start, file).map_err(plain)?;
    let overrides =
        ConfigOverrides::default().with_production(arguments.flag("production").map_err(usage)?.then_some(true));
    let report = explain::explain(&start, &file, export, &overrides)?;
    let report =
        serde_json::to_value(&report).map_err(|error| plain(CliError::new(Kind::Internal, error.to_string())))?;
    Ok(json!({ "version": 1, "skif": env!("CARGO_PKG_VERSION"), "report": report }))
}

/// A malformed argument found while running a tool: reported in the result, so the agent can correct the call.
fn usage(RpcError(_, message): RpcError) -> Failure {
    plain(CliError::new(Kind::Usage, message))
}

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
