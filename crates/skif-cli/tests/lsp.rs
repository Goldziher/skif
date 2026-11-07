//! End-to-end test of `skif lsp`, the language server of ADR 0050, over a scripted stdin.

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
