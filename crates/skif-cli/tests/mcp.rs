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
    assert_eq!(
        names,
        [
            "lint",
            "explain_usage",
            "config",
            "list_rules",
            "cache_stats",
            "cache_clean"
        ]
    );

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

/// Every tool states what a call does to the machine. The table is explicit, so a tool added later fails here until
/// it is listed with its own annotations instead of inheriting read-only ones.
#[test]
fn every_tool_is_annotated_with_what_it_does() {
    let dir = fixture("dirty");
    let replies = session(
        dir.path(),
        &[
            initialize("2025-06-18"),
            json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }),
        ],
    );
    let read_only =
        json!({ "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false });
    let destructive =
        json!({ "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false });
    let expected = [
        ("lint", &read_only),
        ("explain_usage", &read_only),
        ("config", &read_only),
        ("list_rules", &read_only),
        ("cache_stats", &read_only),
        ("cache_clean", &destructive),
    ];
    let listed: Vec<(&str, &Value)> = replies[1]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| (tool["name"].as_str().unwrap(), &tool["annotations"]))
        .collect();
    assert_eq!(listed, expected);
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

/// What a call must return, besides the document of its command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// A document that is not the `error` one, not flagged `isError`.
    Answer,
    /// The lint report with a non-empty `fix.changes`.
    Fixes,
    /// A document that is not the `error` one, flagged `isError`: the command exited 2 after answering.
    Failed,
    /// The `error` document, flagged `isError`.
    Error,
}

/// Each call answers with exactly the document its command prints with `--format json`, and is an error exactly
/// when the command exits 2. Every case runs the binary twice in the same directory: as `skif ARGV` and as one
/// `tools/call` of `skif mcp`. Each case also names what it must return, so a case where both sides fail alike, or
/// plan nothing, does not pass as parity.
#[test]
fn every_tool_answers_with_the_document_of_its_command() {
    use Expect::{Answer, Error, Failed, Fixes};
    let dirty = fixture("dirty");
    let multi = fixture("multi");
    // One root of a recursive run has an unreadable skif.toml; the others are still linted.
    let broken = fixture("multi");
    fs::write(broken.path().join("unrelated/skif.toml"), "[lint\n").unwrap();
    let explained = explain_fixture();
    let cases: Vec<(&Path, &str, Value, Vec<&str>, Expect)> = vec![
        (dirty.path(), "lint", json!({}), vec![], Answer),
        (dirty.path(), "lint", json!({ "path": "." }), vec!["."], Answer),
        (
            dirty.path(),
            "lint",
            json!({ "include": ["empty-dependencies", "root-package-private-field"] }),
            vec!["--include", "empty-dependencies,root-package-private-field"],
            Answer,
        ),
        (
            dirty.path(),
            "lint",
            json!({ "exclude": ["empty-dependencies"], "no_js": true, "production": true }),
            vec!["--exclude", "empty-dependencies", "--no-js", "--production"],
            Answer,
        ),
        (
            dirty.path(),
            "lint",
            json!({ "max_show_issues": 0 }),
            vec!["--max-show-issues", "0"],
            Answer,
        ),
        (
            dirty.path(),
            "lint",
            json!({ "workspace": ["."] }),
            vec!["-W", "."],
            Answer,
        ),
        (
            dirty.path(),
            "lint",
            json!({ "strict": true }),
            vec!["--strict"],
            Answer,
        ),
        (
            dirty.path(),
            "lint",
            json!({ "no_config": true }),
            vec!["--no-config"],
            Answer,
        ),
        (
            dirty.path(),
            "lint",
            json!({ "fix": true }),
            vec!["--fix", "--dry-run"],
            Fixes,
        ),
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
            Fixes,
        ),
        (
            dirty.path(),
            "lint",
            json!({ "path": "missing" }),
            vec!["missing"],
            Error,
        ),
        (
            dirty.path(),
            "lint",
            json!({ "include": ["no-such-rule"] }),
            vec!["--include", "no-such-rule"],
            Error,
        ),
        (
            dirty.path(),
            "lint",
            json!({ "fix": true, "max_show_issues": 1 }),
            vec!["--fix", "--dry-run", "--max-show-issues", "1"],
            Error,
        ),
        (multi.path(), "lint", json!({ "recursive": true }), vec!["-r"], Answer),
        (broken.path(), "lint", json!({ "recursive": true }), vec!["-r"], Failed),
        (dirty.path(), "config", json!({}), vec!["config", "--show"], Answer),
        (
            dirty.path(),
            "config",
            json!({ "explain": "lint.fail-on-warnings" }),
            vec!["config", "--explain", "lint.fail-on-warnings"],
            Answer,
        ),
        (
            multi.path(),
            "config",
            json!({ "recursive": true }),
            vec!["config", "--show", "-r"],
            Answer,
        ),
        (
            broken.path(),
            "config",
            json!({ "recursive": true }),
            vec!["config", "--show", "-r"],
            Failed,
        ),
        (dirty.path(), "list_rules", json!({}), vec!["rules"], Answer),
        (
            explained.path(),
            "explain_usage",
            json!({ "file": "lib.ts", "export": "idle" }),
            vec!["explain-usage", "lib.ts#idle"],
            Answer,
        ),
        (
            explained.path(),
            "explain_usage",
            json!({ "file": "orphan.ts" }),
            vec!["explain-usage", "orphan.ts"],
            Error,
        ),
        (
            explained.path(),
            "explain_usage",
            json!({ "dependency": "left-pad" }),
            vec!["explain-usage", "--dependency", "left-pad"],
            Answer,
        ),
    ];
    for (dir, tool, arguments, argv, expect) in cases {
        let cli = run(skif().current_dir(dir).args(&argv).args(["--format", "json"]));
        let expected: Value = serde_json::from_str(&cli.stdout)
            .unwrap_or_else(|error| panic!("skif {argv:?}: {error}: {}{}", cli.stdout, cli.stderr));
        let replies = session(dir, &[call(1, tool, &arguments)]);
        let result = &replies[0]["result"];
        let document = &result["structuredContent"];
        assert_eq!(document, &expected, "{tool} {arguments} against skif {argv:?}");
        let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, expected, "{tool} {arguments}: the text is the same document");
        assert_eq!(
            result["isError"],
            cli.code == 2,
            "{tool} {arguments}: isError against exit code {}",
            cli.code
        );
        assert_eq!(
            result["isError"],
            matches!(expect, Failed | Error),
            "{tool} {arguments}: expected {expect:?}: {document}"
        );
        assert_eq!(
            document["error"].is_object(),
            expect == Error,
            "{tool} {arguments}: expected {expect:?}: {document}"
        );
        if expect == Fixes {
            let changes = document["fix"]["changes"].as_array();
            assert!(
                changes.is_some_and(|changes| !changes.is_empty()),
                "{tool} {arguments}: no planned change: {document}"
            );
        }
        if dir == broken.path() {
            let errors = document["errors"].as_array();
            assert!(
                errors.is_some_and(|errors| !errors.is_empty()),
                "{tool} {arguments}: the failed root is listed: {document}"
            );
        }
    }
}

/// The cache tools mirror `skif cache`, and a lint of the server reads the store without writing it.
#[test]
fn the_cache_tools_show_and_remove_the_store_and_a_lint_never_writes_it() {
    let dir = fixture("js");
    let home = tempfile::tempdir().unwrap();
    let location = home.path().join("cache");
    let cached = |command: &mut assert_cmd::Command| {
        run(command
            .current_dir(dir.path())
            .env_remove("SKIF_CACHE")
            .env("HOME", home.path())
            .env("XDG_CACHE_HOME", home.path().join("xdg"))
            .env("SKIF_CACHE_DIR", &location))
    };
    let stores = || -> Vec<std::path::PathBuf> {
        fs::read_dir(&location).map_or_else(
            |_| Vec::new(),
            |entries| {
                entries
                    .map(|entry| entry.unwrap().path().join("modules.bin"))
                    .filter(|file| file.is_file())
                    .collect()
            },
        )
    };
    let messages = |calls: &[Value]| -> String {
        std::iter::once(initialize("2025-06-18"))
            .chain(calls.iter().cloned())
            .map(|message| message.to_string() + "\n")
            .collect()
    };
    let replies =
        |stdout: &str| -> Vec<Value> { stdout.lines().map(|line| serde_json::from_str(line).unwrap()).collect() };

    // Nothing is stored yet: the server's lint leaves it that way.
    let cold = cached(skif().arg("mcp").write_stdin(messages(&[call(1, "lint", &json!({}))])));
    assert_eq!(replies(&cold.stdout)[1]["result"]["isError"], false, "{}", cold.stdout);
    assert!(stores().is_empty(), "the server wrote the store");

    // A run of the command line writes it; the server reads it and still writes nothing.
    assert!(matches!(cached(skif().args(["--format", "json"])).code, 0 | 1));
    let on_disk = stores();
    let [store] = on_disk.as_slice() else {
        panic!("expected one store in {}", location.display());
    };
    let written = fs::read(store).unwrap();
    let session = cached(skif().arg("mcp").write_stdin(messages(&[
        call(1, "lint", &json!({})),
        call(2, "cache_stats", &json!({})),
        call(3, "cache_clean", &json!({})),
        call(4, "cache_stats", &json!({})),
    ])));
    assert_eq!(session.code, 0, "{}", session.stderr);
    let answers = replies(&session.stdout);
    assert_eq!(
        answers[1]["result"]["structuredContent"],
        replies(&cold.stdout)[1]["result"]["structuredContent"]
    );
    let stats = &answers[2]["result"];
    assert_eq!(stats["isError"], false, "{stats}");
    let found = &stats["structuredContent"]["stores"];
    assert_eq!(found.as_array().unwrap().len(), 1, "{stats}");
    assert_eq!(found[0]["current"], true);
    assert_eq!(found[0]["namespaces"][0]["name"], "modules");
    // The lint before the stats call changed nothing on disk.
    assert_eq!(found[0]["namespaces"][0]["bytes"], written.len());
    let cleaned = &answers[3]["result"];
    assert_eq!(cleaned["isError"], false, "{cleaned}");
    assert_eq!(cleaned["structuredContent"]["stores"], 1);
    assert_eq!(answers[4]["result"]["structuredContent"]["stores"], json!([]));
    assert_eq!(stores(), Vec::<std::path::PathBuf>::new());
}
