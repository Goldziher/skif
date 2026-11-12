//! End-to-end test of `skif lsp`, the language server of ADR 0028 and ADR 0029, over a scripted stdin.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use serde_json::{Value, json};
use support::{fixture, run, skif};

fn framed(messages: &[Value]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for message in messages {
        let body = message.to_string();
        bytes.extend(format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes());
        bytes.extend(body.into_bytes());
    }
    bytes
}

fn unframed(mut text: &str) -> Vec<Value> {
    let mut messages = Vec::new();
    while let Some((header, rest)) = text.split_once("\r\n\r\n") {
        let length: usize = header.strip_prefix("Content-Length: ").unwrap().parse().unwrap();
        messages.push(serde_json::from_str(&rest[..length]).unwrap());
        text = &rest[length..];
    }
    messages
}

/// `path` resolved as an editor names it: canonical, without the `\\?\` prefix Windows gives a canonical drive path,
/// so the URIs the test sends and expects are the ones a client would use.
fn canonical(path: &std::path::Path) -> std::path::PathBuf {
    let resolved = path.canonicalize().unwrap();
    let text = resolved.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => std::path::PathBuf::from(rest),
        _ => resolved,
    }
}

fn file_uri(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

#[test]
fn opening_a_manifest_publishes_the_root_diagnostics() {
    let dir = fixture("dirty");
    let root = canonical(dir.path());
    let manifest = root.join("packages").join("a").join("package.json");
    let input = framed(&[
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "processId": null, "rootUri": null, "capabilities": {} } }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        did_open(&manifest, 1, &std::fs::read_to_string(&manifest).unwrap()),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown" }),
        json!({ "jsonrpc": "2.0", "method": "exit" }),
    ]);
    let result = run(skif().arg("lsp").write_stdin(input));
    assert_eq!(result.code, 0, "{}", result.stderr);
    let messages = unframed(&result.stdout);

    assert_eq!(messages[0]["id"], 1);
    // Full document sync: every change carries the whole text.
    assert_eq!(messages[0]["result"]["capabilities"]["textDocumentSync"]["change"], 1);
    assert_eq!(
        messages[0]["result"]["capabilities"]["textDocumentSync"]["save"]["includeText"],
        false
    );
    let published: Vec<&Value> = messages
        .iter()
        .filter(|message| message["method"] == "textDocument/publishDiagnostics")
        .collect();
    let for_file = |suffix: &str| {
        published
            .iter()
            .find(|message| message["params"]["uri"].as_str().unwrap().ends_with(suffix))
            .unwrap_or_else(|| panic!("no diagnostics for {suffix}: {published:#?}"))
    };
    let member = &for_file("/packages/a/package.json")["params"]["diagnostics"][0];
    assert_eq!(member["code"], "empty-dependencies");
    assert_eq!(member["source"], "skif");
    // `"devDependencies": {}` on line 7, column 3 (1-based) of the fixture.
    assert_eq!(
        member["range"],
        json!({ "start": { "line": 6, "character": 2 }, "end": { "line": 6, "character": 23 } })
    );
    let root_name = root.file_name().unwrap().to_str().unwrap();
    let private = &for_file(&format!("/{root_name}/package.json"))["params"]["diagnostics"][0];
    assert_eq!(private["code"], "root-package-private-field");
    assert_eq!(private["severity"], 1);
    assert_eq!(
        messages.last().unwrap(),
        &json!({ "jsonrpc": "2.0", "id": 2, "result": null })
    );
}

#[test]
fn a_malformed_frame_is_reported_on_stderr_and_the_server_exits() {
    let mut input =
        framed(&[json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "capabilities": {} } })]);
    input.extend(b"Content-Length: 99999999999\r\n\r\n");
    let result = run(skif().arg("lsp").write_stdin(input));
    assert_eq!(result.code, 1, "{}", result.stderr);
    assert_eq!(unframed(&result.stdout).len(), 1, "{}", result.stdout);
    assert!(
        result.stderr.contains("Content-Length 99999999999 exceeds"),
        "{}",
        result.stderr
    );
    assert!(result.stderr.contains("exiting"), "{}", result.stderr);
}

/// A `didOpen` of `file` with `text` at `version`.
fn did_open(file: &std::path::Path, version: i64, text: &str) -> Value {
    json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": { "textDocument": {
        "uri": file_uri(file), "languageId": "json", "version": version, "text": text
    } } })
}

/// A full-sync `didChange` of `file` to `text` at `version`.
fn did_change(file: &std::path::Path, version: i64, text: &str) -> Value {
    json!({ "jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
        "textDocument": { "uri": file_uri(file), "version": version },
        "contentChanges": [{ "text": text }]
    } })
}

/// The `initialize` and `initialized` messages, with the client's `capabilities`.
fn initialized(capabilities: &Value) -> Vec<Value> {
    vec![
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "processId": null, "rootUri": null, "capabilities": capabilities } }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
    ]
}

/// The `initialize`, `initialized` and `didOpen` messages of a session on `file`, opened as saved.
fn opened(file: &std::path::Path) -> Vec<Value> {
    let mut messages = initialized(&json!({}));
    messages.push(did_open(file, 1, &std::fs::read_to_string(file).unwrap()));
    messages
}

/// A `textDocument/codeAction` request for `file` at the 0-based `line` and `character`, with `context.only`.
fn code_action(id: u64, file: &std::path::Path, (line, character): (u64, u64), only: Option<&[&str]>) -> Value {
    let mut context = json!({ "diagnostics": [] });
    if let Some(only) = only {
        context["only"] = json!(only);
    }
    let at = json!({ "line": line, "character": character });
    json!({ "jsonrpc": "2.0", "id": id, "method": "textDocument/codeAction", "params": {
        "textDocument": { "uri": file_uri(file) }, "range": { "start": at, "end": at }, "context": context
    } })
}

/// Runs a session of `messages` followed by `shutdown` and `exit`, and returns the results of the requests by id.
fn session(messages: Vec<Value>) -> std::collections::BTreeMap<u64, Value> {
    let mut messages = messages;
    messages.push(json!({ "jsonrpc": "2.0", "id": 999, "method": "shutdown" }));
    messages.push(json!({ "jsonrpc": "2.0", "method": "exit" }));
    let result = run(skif().arg("lsp").write_stdin(framed(&messages)));
    assert_eq!(result.code, 0, "{}", result.stderr);
    unframed(&result.stdout)
        .into_iter()
        .filter_map(|message| Some((message["id"].as_u64()?, message["result"].clone())))
        .collect()
}

/// `text` with the LSP `edits` applied; the fixtures are ASCII, so a character is a byte.
fn apply(text: &str, edits: &[Value]) -> String {
    let offset = |position: &Value| {
        let line = usize::try_from(position["line"].as_u64().unwrap()).unwrap();
        let character = usize::try_from(position["character"].as_u64().unwrap()).unwrap();
        text.split_inclusive('\n').take(line).map(str::len).sum::<usize>() + character
    };
    let mut edits: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|edit| {
            (
                offset(&edit["range"]["start"]),
                offset(&edit["range"]["end"]),
                edit["newText"].as_str().unwrap(),
            )
        })
        .collect();
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    let mut text = text.to_owned();
    for (start, end, new_text) in edits {
        text.replace_range(start..end, new_text);
    }
    text
}

#[test]
fn a_safe_fix_is_a_preferred_quick_fix_and_part_of_fix_all() {
    let dir = fixture("dirty");
    let root = canonical(dir.path());
    let manifest = root.join("packages").join("a").join("package.json");
    let mut messages = opened(&manifest);
    // On `"devDependencies": {}` (line 7, column 3 1-based), then outside every diagnostic, then fix-all only.
    messages.push(code_action(2, &manifest, (6, 5), None));
    messages.push(code_action(3, &manifest, (1, 0), Some(&["quickfix"])));
    messages.push(code_action(4, &manifest, (1, 0), Some(&["source.fixAll"])));
    let results = session(messages);

    let capabilities = &results[&1]["capabilities"];
    assert_eq!(
        capabilities["codeActionProvider"]["codeActionKinds"],
        json!(["quickfix", "source.fixAll.skif"])
    );
    let actions = results[&2].as_array().unwrap();
    assert_eq!(actions.len(), 2, "{actions:#?}");
    let quick_fix = &actions[0];
    assert_eq!(quick_fix["title"], "Remove the empty `devDependencies` field");
    assert_eq!(quick_fix["kind"], "quickfix");
    assert_eq!(quick_fix["isPreferred"], true);
    assert_eq!(quick_fix["diagnostics"][0]["code"], "empty-dependencies");
    let uri = file_uri(&manifest);
    let changes = quick_fix["edit"]["changes"].as_object().unwrap();
    assert_eq!(changes.keys().collect::<Vec<_>>(), [&uri]);
    let before = std::fs::read_to_string(&manifest).unwrap();
    let after = apply(&before, changes[&uri].as_array().unwrap());
    assert_eq!(
        after,
        "{\n  \"name\": \"@cli/a\",\n  \"version\": \"0.0.0\",\n  \"dependencies\": {\n    \"is-number\": \"7.0.0\"\n  }\n}\n"
    );
    assert_eq!(actions[1]["kind"], "source.fixAll.skif");
    assert_eq!(actions[1]["edit"], quick_fix["edit"]);

    assert_eq!(results[&3], json!([]));
    let fix_all = results[&4].as_array().unwrap();
    assert_eq!(fix_all.len(), 1, "{fix_all:#?}");
    assert_eq!(fix_all[0]["kind"], "source.fixAll.skif");
    // The server never writes: the file is as it was.
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), before);
}

#[test]
fn an_unsafe_fix_is_labelled_and_left_out_of_fix_all() {
    let dir = fixture("mismatch");
    let root = canonical(dir.path());
    let manifest = root.join("packages").join("b").join("package.json");
    let before = std::fs::read_to_string(&manifest).unwrap();
    let line = u64::try_from(before.lines().position(|line| line.contains("6.0.0")).unwrap()).unwrap();
    let mut messages = opened(&manifest);
    messages.push(code_action(2, &manifest, (line, 8), None));
    let results = session(messages);

    let actions = results[&2].as_array().unwrap();
    assert_eq!(actions.len(), 1, "{actions:#?}");
    assert_eq!(actions[0]["title"], "Set `is-number` to `7.0.0` (unsafe)");
    assert_eq!(actions[0]["isPreferred"], false);
    let edits = actions[0]["edit"]["changes"][file_uri(&manifest)].as_array().unwrap();
    assert_eq!(apply(&before, edits), before.replace("6.0.0", "7.0.0"));
}

/// The diagnostics of the last `publishDiagnostics` for `file` among `messages`.
fn last_published<'a>(messages: &'a [Value], file: &std::path::Path) -> &'a Value {
    let uri = file_uri(file);
    &messages
        .iter()
        .rev()
        .find(|message| message["method"] == "textDocument/publishDiagnostics" && message["params"]["uri"] == uri)
        .unwrap_or_else(|| panic!("nothing published for {uri}: {messages:#?}"))["params"]["diagnostics"]
}

#[test]
fn an_unsaved_change_is_linted_before_shutdown_is_answered() {
    let dir = fixture("dirty");
    let root = canonical(dir.path());
    let manifest = root.join("packages").join("b").join("package.json");
    let saved = std::fs::read_to_string(&manifest).unwrap();
    let typed = saved.replace("  }\n}", "  },\n  \"devDependencies\": {}\n}");
    assert_ne!(typed, saved);
    let mut messages = initialized(&json!({}));
    messages.push(did_open(&manifest, 1, &saved));
    messages.push(did_change(&manifest, 2, &typed));
    messages.push(json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown" }));
    messages.push(json!({ "jsonrpc": "2.0", "method": "exit" }));
    let result = run(skif().arg("lsp").write_stdin(framed(&messages)));
    assert_eq!(result.code, 0, "{}", result.stderr);
    let messages = unframed(&result.stdout);

    // The lint of the change publishes before the answer to `shutdown`, the last message.
    assert_eq!(messages.last().unwrap()["id"], 2);
    let diagnostics = last_published(&messages, &manifest).as_array().unwrap();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(diagnostics[0]["code"], "empty-dependencies");
    // `"devDependencies": {}` on line 7 of the buffer; the saved file has six lines.
    assert_eq!(
        diagnostics[0]["range"],
        json!({ "start": { "line": 6, "character": 2 }, "end": { "line": 6, "character": 23 } })
    );
    // The server never writes: the file is as saved.
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), saved);
}

#[test]
fn a_fix_for_an_unsaved_buffer_edits_the_buffer_until_the_buffer_changes() {
    let dir = fixture("dirty");
    let root = canonical(dir.path());
    let manifest = root.join("packages").join("a").join("package.json");
    let saved = std::fs::read_to_string(&manifest).unwrap();
    // One line more than the saved file, so `"devDependencies": {}` moves to line 8 (0-based 7).
    let buffer = saved.replace("  \"version\"", "  \"description\": \"unsaved\",\n  \"version\"");
    let mut messages = initialized(&json!({}));
    messages.push(did_open(&manifest, 1, &buffer));
    messages.push(code_action(2, &manifest, (7, 5), Some(&["quickfix"])));
    // A further change with no lint in between: the plan's positions may be stale, so nothing is offered.
    messages.push(did_change(&manifest, 2, &format!("{buffer}\n")));
    messages.push(code_action(3, &manifest, (7, 5), Some(&["quickfix"])));
    let results = session(messages);

    let actions = results[&2].as_array().unwrap();
    assert_eq!(actions.len(), 1, "{actions:#?}");
    let edits = actions[0]["edit"]["changes"][file_uri(&manifest)].as_array().unwrap();
    assert_eq!(
        apply(&buffer, edits),
        "{\n  \"name\": \"@cli/a\",\n  \"description\": \"unsaved\",\n  \"version\": \"0.0.0\",\n  \"dependencies\": {\n    \"is-number\": \"7.0.0\"\n  }\n}\n"
    );
    assert_eq!(results[&3], json!([]));
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), saved);
}

#[test]
fn closing_a_document_with_unsaved_changes_lints_the_saved_file() {
    let dir = fixture("dirty");
    let root = canonical(dir.path());
    let manifest = root.join("packages").join("a").join("package.json");
    let saved = std::fs::read_to_string(&manifest).unwrap();
    // One line more than the saved file, so `"devDependencies": {}` is on line 8 (0-based 7) of the buffer only.
    let buffer = saved.replace("  \"version\"", "  \"description\": \"unsaved\",\n  \"version\"");
    let mut messages = initialized(&json!({}));
    messages.push(did_open(&manifest, 1, &buffer));
    messages.push(json!({ "jsonrpc": "2.0", "method": "textDocument/didClose", "params": {
        "textDocument": { "uri": file_uri(&manifest) }
    } }));
    messages.push(code_action(2, &manifest, (6, 5), Some(&["quickfix"])));
    messages.push(json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown" }));
    messages.push(json!({ "jsonrpc": "2.0", "method": "exit" }));
    let result = run(skif().arg("lsp").write_stdin(framed(&messages)));
    assert_eq!(result.code, 0, "{}", result.stderr);
    let messages = unframed(&result.stdout);

    // The discarded buffer no longer decides the positions: the file on disk does.
    let diagnostics = last_published(&messages, &manifest).as_array().unwrap();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
    assert_eq!(
        diagnostics[0]["range"],
        json!({ "start": { "line": 6, "character": 2 }, "end": { "line": 6, "character": 23 } })
    );
    let actions = messages.iter().find(|message| message["id"] == 2).unwrap()["result"]
        .as_array()
        .unwrap();
    assert_eq!(actions.len(), 1, "{actions:#?}");
    let edits = actions[0]["edit"]["changes"][file_uri(&manifest)].as_array().unwrap();
    assert!(!apply(&saved, edits).contains("devDependencies"));
}

#[test]
fn with_document_changes_an_edit_carries_the_document_version() {
    let dir = fixture("dirty");
    let root = canonical(dir.path());
    let manifest = root.join("packages").join("a").join("package.json");
    let saved = std::fs::read_to_string(&manifest).unwrap();
    let mut messages = initialized(&json!({ "workspace": { "workspaceEdit": { "documentChanges": true } } }));
    messages.push(did_open(&manifest, 7, &saved));
    messages.push(code_action(2, &manifest, (6, 5), None));
    let results = session(messages);

    let actions = results[&2].as_array().unwrap();
    assert_eq!(actions.len(), 2, "{actions:#?}");
    for action in actions {
        let changes = action["edit"]["documentChanges"].as_array().unwrap();
        assert_eq!(changes.len(), 1, "{action:#?}");
        assert_eq!(
            changes[0]["textDocument"],
            json!({ "uri": file_uri(&manifest), "version": 7 })
        );
        assert!(action["edit"].get("changes").is_none(), "{action:#?}");
        let after = apply(&saved, changes[0]["edits"].as_array().unwrap());
        assert!(!after.contains("devDependencies"), "{after}");
    }
}

/// Reads one framed message from the server's stdout.
fn read_frame(reader: &mut impl std::io::BufRead) -> Value {
    let mut length = 0;
    loop {
        let mut line = String::new();
        assert_ne!(reader.read_line(&mut line).unwrap(), 0, "the server closed its output");
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length: ") {
            length = value.parse().unwrap();
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

#[test]
fn a_burst_of_changes_is_linted_once_the_root_is_quiet() {
    use std::io::Write as _;
    use std::process::{Command, Stdio};

    let dir = fixture("dirty");
    let root = canonical(dir.path());
    let manifest = root.join("packages").join("b").join("package.json");
    let saved = std::fs::read_to_string(&manifest).unwrap();
    let typed = saved.replace("  }\n}", "  },\n  \"devDependencies\": {}\n}");
    let mut child = Command::new(env!("CARGO_BIN_EXE_skif"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut messages = initialized(&json!({}));
    messages.push(did_open(&manifest, 1, &saved));
    // A burst: three changes, the last of which introduces the problem.
    messages.push(did_change(&manifest, 2, &saved.replace("0.0.0", "0.0.1")));
    messages.push(did_change(&manifest, 3, &saved));
    messages.push(did_change(&manifest, 4, &typed));
    stdin.write_all(&framed(&messages)).unwrap();
    stdin.flush().unwrap();
    // Without a request or the end of the input, only the debounce timer can lint the change: wait for it.
    // Every lint of the root publishes the problems of `packages/a`, before those of `packages/b`.
    let uri = file_uri(&manifest);
    let other = file_uri(&root.join("packages").join("a").join("package.json"));
    let mut lints = 0;
    loop {
        let message = read_frame(&mut stdout);
        if message["method"] != "textDocument/publishDiagnostics" {
            continue;
        }
        if message["params"]["uri"] == other {
            lints += 1;
        }
        if message["params"]["uri"] == uri {
            assert_eq!(
                message["params"]["diagnostics"][0]["code"], "empty-dependencies",
                "{message:#?}"
            );
            break;
        }
    }
    // The lint of the open and one lint for the whole burst.
    assert_eq!(lints, 2);
    stdin
        .write_all(&framed(&[
            code_action(2, &manifest, (6, 5), Some(&["quickfix"])),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown" }),
            json!({ "jsonrpc": "2.0", "method": "exit" }),
        ]))
        .unwrap();
    drop(stdin);
    let mut actions = Value::Null;
    loop {
        let message = read_frame(&mut stdout);
        if message["id"] == 2 {
            actions = message["result"].clone();
        }
        if message["id"] == 3 {
            break;
        }
    }
    assert!(child.wait().unwrap().success());
    let edits = actions[0]["edit"]["changes"][&uri].as_array().unwrap();
    assert_eq!(apply(&typed, edits), saved);
}
