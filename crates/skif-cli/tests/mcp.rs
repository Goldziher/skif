//! End-to-end tests of `skif mcp`, the Model Context Protocol server of ADR 0013, over a scripted stdin.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use support::{fixture, run, skif};

/// Runs `skif mcp` in `dir` with `messages` on stdin and returns every reply, in order.
fn session(dir: &Path, messages: &[Value]) -> Vec<Value> {
    let input: String = messages.iter().map(|message| message.to_string() + "\n").collect();
    let result = run(skif().current_dir(dir).arg("mcp").write_stdin(input));
    assert_eq!(result.code, 0, "{}{}", result.stdout, result.stderr);
    result
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap_or_else(|error| panic!("{error}: {line}")))
        .collect()
}

fn initialize(version: &str) -> Value {
    json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": { "protocolVersion": version, "capabilities": {}, "clientInfo": { "name": "test", "version": "1" } }
    })
}

fn call(id: u64, name: &str, arguments: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } })
}

fn rules(document: &Value) -> Vec<&str> {
    document["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|diagnostic| diagnostic["rule"].as_str().unwrap())
        .collect()
}

#[test]
fn a_session_initializes_lists_tools_and_lints() {
    let dir = fixture("dirty");
    let replies = session(
        dir.path(),
        &[
            initialize("2025-06-18"),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
            call(2, "lint", &json!({})),
            call(3, "lint", &json!({ "path": ".", "include": ["empty-dependencies"] })),
            call(4, "list_rules", &json!({})),
        ],
    );
    assert_eq!(replies.len(), 5, "the notification gets no answer: {replies:#?}");

    let init = &replies[0];
    assert_eq!(init["id"], 0);
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "skif");
    assert!(init["result"]["capabilities"]["tools"].is_object());

    let names: Vec<&str> = replies[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["lint", "explain_usage", "config", "list_rules"]);

    let lint = &replies[2]["result"];
    assert_eq!(lint["isError"], false);
    let report = &lint["structuredContent"];
    assert_eq!(report["summary"]["errors"], 1);
    assert_eq!(rules(report), ["root-package-private-field", "empty-dependencies"]);
    let text: Value = serde_json::from_str(lint["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(&text, report, "the text content is the same document");

    let filtered = &replies[3]["result"]["structuredContent"];
    assert_eq!(rules(filtered), ["empty-dependencies"]);
    assert_eq!(filtered["summary"]["errors"], 0);

    let listed = &replies[4]["result"]["structuredContent"]["rules"];
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["id"] == "root-package-private-field")
    );
}

#[test]
fn an_unknown_version_gets_the_newest_one() {
    let dir = fixture("clean");
    let replies = session(dir.path(), &[initialize("2000-01-01")]);
    assert_eq!(replies[0]["result"]["protocolVersion"], "2025-11-25");
}

#[test]
fn a_tool_that_cannot_run_returns_the_error_document() {
    let dir = fixture("clean");
    let replies = session(
        dir.path(),
        &[
            call(1, "lint", &json!({ "path": "missing" })),
            call(2, "lint", &json!({ "include": ["no-such-rule"] })),
        ],
    );
    for reply in &replies {
        assert_eq!(reply["result"]["isError"], true, "{reply}");
        assert!(
            reply["result"]["structuredContent"]["error"]["kind"].is_string(),
            "{reply}"
        );
    }
    assert_eq!(replies[0]["result"]["structuredContent"]["error"]["kind"], "io");
    assert_eq!(replies[1]["result"]["structuredContent"]["error"]["kind"], "usage");
}

#[test]
fn protocol_errors_are_json_rpc_errors() {
    let dir = fixture("clean");
    let input = format!(
        "not json\n{}\n{}\n",
        json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/list" }),
        call(2, "fix_everything", &json!({}))
    );
    let result = run(skif().current_dir(dir.path()).arg("mcp").write_stdin(input));
    assert_eq!(result.code, 0);
    let codes: Vec<i64> = result
        .stdout
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["error"]["code"]
                .as_i64()
                .unwrap()
        })
        .collect();
    assert_eq!(codes, [-32700, -32601, -32602]);
}

#[test]
fn explain_usage_returns_the_report() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".git")).unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"name":"app","version":"1.0.0","private":true,"main":"index.ts"}"#,
    )
    .unwrap();
    fs::write(dir.path().join("index.ts"), "import { run } from './lib';\nrun();\n").unwrap();
    fs::write(
        dir.path().join("lib.ts"),
        "export function run() {}\nexport function idle() {}\n",
    )
    .unwrap();
    fs::write(dir.path().join("orphan.ts"), "export const lonely = 1;\n").unwrap();
    let replies = session(
        dir.path(),
        &[
            call(1, "explain_usage", &json!({ "file": "lib.ts", "export": "idle" })),
            call(2, "explain_usage", &json!({ "file": "orphan.ts" })),
            call(3, "explain_usage", &json!({})),
        ],
    );
    assert_eq!(replies[0]["result"]["isError"], false, "{}", replies[0]);
    assert_eq!(
        replies[0]["result"]["structuredContent"]["report"]["export"]["verdict"],
        "unused"
    );
    let orphan = &replies[1]["result"];
    assert_eq!(orphan["isError"], true);
    assert_eq!(orphan["structuredContent"]["report"]["in-graph"], false, "{orphan}");
    assert_eq!(replies[2]["result"]["isError"], true);
}

/// A temporary project whose `index.ts` uses `run` of `lib.ts`, and leaves `idle` and `orphan.ts` unused.
fn explain_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join(".git")).unwrap();
    fs::write(
        dir.path().join("package.json"),
        r#"{"name":"app","version":"1.0.0","private":true,"main":"index.ts"}"#,
    )
    .unwrap();
    fs::write(dir.path().join("index.ts"), "import { run } from './lib';\nrun();\n").unwrap();
    fs::write(
        dir.path().join("lib.ts"),
        "export function run() {}\nexport function idle() {}\n",
    )
    .unwrap();
    fs::write(dir.path().join("orphan.ts"), "export const lonely = 1;\n").unwrap();
    dir
}

/// Each call answers with exactly the document its command prints with `--format json`, and is an error exactly
/// when the command exits 2. Every case runs the binary twice in the same directory: as `skif ARGV` and as one
/// `tools/call` of `skif mcp`.
#[test]
fn every_tool_answers_with_the_document_of_its_command() {
    let dirty = fixture("dirty");
    let multi = fixture("multi");
    let explained = explain_fixture();
    let cases: Vec<(&Path, &str, Value, Vec<&str>)> = vec![
        (dirty.path(), "lint", json!({}), vec![]),
        (dirty.path(), "lint", json!({ "path": "." }), vec!["."]),
        (
            dirty.path(),
            "lint",
            json!({ "include": ["empty-dependencies", "root-package-private-field"] }),
            vec!["--include", "empty-dependencies,root-package-private-field"],
        ),
        (
            dirty.path(),
            "lint",
            json!({ "exclude": ["empty-dependencies"], "no_js": true, "production": true }),
            vec!["--exclude", "empty-dependencies", "--no-js", "--production"],
        ),
        (
            dirty.path(),
            "lint",
            json!({ "max_show_issues": 0 }),
            vec!["--max-show-issues", "0"],
        ),
        (dirty.path(), "lint", json!({ "workspace": ["."] }), vec!["-W", "."]),
        (dirty.path(), "lint", json!({ "strict": true }), vec!["--strict"]),
        (dirty.path(), "lint", json!({ "no_config": true }), vec!["--no-config"]),
        (dirty.path(), "lint", json!({ "fix": true }), vec!["--fix", "--dry-run"]),
        (
            dirty.path(),
            "lint",
            json!({ "fix_unsafe": true, "select": "highest", "fix_type": ["empty-dependencies"] }),
            vec![
                "--fix-unsafe",
                "--dry-run",
                "--select",
                "highest",
                "--fix-type",
                "empty-dependencies",
            ],
        ),
        (dirty.path(), "lint", json!({ "path": "missing" }), vec!["missing"]),
        (
            dirty.path(),
            "lint",
            json!({ "include": ["no-such-rule"] }),
            vec!["--include", "no-such-rule"],
        ),
        (
            dirty.path(),
            "lint",
            json!({ "fix": true, "max_show_issues": 1 }),
            vec!["--fix", "--dry-run", "--max-show-issues", "1"],
        ),
        (multi.path(), "lint", json!({ "recursive": true }), vec!["-r"]),
        (dirty.path(), "config", json!({}), vec!["config", "--show"]),
        (
            dirty.path(),
            "config",
            json!({ "explain": "lint.fail-on-warnings" }),
            vec!["config", "--explain", "lint.fail-on-warnings"],
        ),
        (
            multi.path(),
            "config",
            json!({ "recursive": true }),
            vec!["config", "--show", "-r"],
        ),
        (dirty.path(), "list_rules", json!({}), vec!["rules"]),
        (
            explained.path(),
            "explain_usage",
            json!({ "file": "lib.ts", "export": "idle" }),
            vec!["explain-usage", "lib.ts#idle"],
        ),
        (
            explained.path(),
            "explain_usage",
            json!({ "file": "orphan.ts" }),
            vec!["explain-usage", "orphan.ts"],
        ),
        (
            explained.path(),
            "explain_usage",
            json!({ "dependency": "left-pad" }),
            vec!["explain-usage", "--dependency", "left-pad"],
        ),
    ];
    for (dir, tool, arguments, argv) in cases {
        let cli = run(skif().current_dir(dir).args(&argv).args(["--format", "json"]));
        let expected: Value = serde_json::from_str(&cli.stdout)
            .unwrap_or_else(|error| panic!("skif {argv:?}: {error}: {}{}", cli.stdout, cli.stderr));
        let replies = session(dir, &[call(1, tool, &arguments)]);
        let result = &replies[0]["result"];
        assert_eq!(
            result["structuredContent"], expected,
            "{tool} {arguments} against skif {argv:?}"
        );
        let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, expected, "{tool} {arguments}: the text is the same document");
        assert_eq!(
            result["isError"],
            cli.code == 2,
            "{tool} {arguments}: isError against exit code {}",
            cli.code
        );
    }
}
