//! End-to-end test of `skif lsp`, the language server of ADR 0056, over a scripted stdin.

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
    let root = dir.path().canonicalize().unwrap();
    let manifest = root.join("packages").join("a").join("package.json");
    let input = framed(&[
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "processId": null, "rootUri": null, "capabilities": {} } }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": { "textDocument": {
            "uri": file_uri(&manifest), "languageId": "json", "version": 1, "text": ""
        } } }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown" }),
        json!({ "jsonrpc": "2.0", "method": "exit" }),
    ]);
    let result = run(skif().arg("lsp").write_stdin(input));
    assert_eq!(result.code, 0, "{}", result.stderr);
    let messages = unframed(&result.stdout);

    assert_eq!(messages[0]["id"], 1);
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

/// The `initialize`, `initialized` and `didOpen` messages of a session on `file`.
fn opened(file: &std::path::Path) -> Vec<Value> {
    vec![
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "processId": null, "rootUri": null, "capabilities": {} } }),
        json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
        json!({ "jsonrpc": "2.0", "method": "textDocument/didOpen", "params": { "textDocument": {
            "uri": file_uri(file), "languageId": "json", "version": 1, "text": ""
        } } }),
    ]
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
    let root = dir.path().canonicalize().unwrap();
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
    let root = dir.path().canonicalize().unwrap();
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

#[test]
fn a_document_with_unsaved_changes_gets_no_fixes_until_it_is_saved() {
    let dir = fixture("dirty");
    let root = dir.path().canonicalize().unwrap();
    let manifest = root.join("packages").join("a").join("package.json");
    let uri = file_uri(&manifest);
    let mut messages = opened(&manifest);
    messages.push(json!({ "jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
        "textDocument": { "uri": uri, "version": 2 },
        "contentChanges": [{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }, "text": " " }]
    } }));
    messages.push(code_action(2, &manifest, (6, 5), None));
    messages.push(
        json!({ "jsonrpc": "2.0", "method": "textDocument/didSave", "params": { "textDocument": { "uri": uri } } }),
    );
    messages.push(code_action(3, &manifest, (6, 5), None));
    let results = session(messages);

    assert_eq!(results[&2], json!([]));
    assert_eq!(results[&3].as_array().unwrap().len(), 2, "{:#?}", results[&3]);
}
