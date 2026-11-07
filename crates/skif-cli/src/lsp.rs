//! `skif lsp`: a minimal Language Server Protocol server over stdio (ADR 0050).
//!
//! On open and save of a document it lints the root containing the file, as saved on disk, and publishes the
//! diagnostics of every file of that root. Code actions and unsaved buffers are not supported.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use skif::{ConfigOverrides, Diagnostic, Severity};
use skif_core::LineIndex;

use crate::lint::lint_root;
use crate::output::stderr_line;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;

/// Serves stdin and stdout until `exit`; returns 0 when `shutdown` came first, 1 otherwise, as the protocol asks.
pub fn run() -> u8 {
    match serve(io::stdin().lock(), io::stdout().lock()) {
        Ok(code) => code,
        Err(error) => {
            stderr_line(&format!("error: skif lsp: {error}"));
            1
        }
    }
}

/// Answers the framed messages of `input` on `output` until `exit` or the end of `input`, and returns the exit code.
///
/// # Errors
/// An error reading `input` or writing `output`, or a malformed header.
pub fn serve(mut input: impl BufRead, mut output: impl Write) -> io::Result<u8> {
    let mut server = Server::default();
    while let Some(body) = read_message(&mut input)? {
        let replies = match serde_json::from_slice::<Value>(&body) {
            Ok(message) => match server.handle(&message) {
                Step::Reply(replies) => replies,
                Step::Exit => return Ok(u8::from(!server.shut_down)),
            },
            Err(error) => vec![error_reply(&Value::Null, PARSE_ERROR, &format!("parse error: {error}"))],
        };
        for reply in &replies {
            write_message(&mut output, reply)?;
        }
    }
    Ok(u8::from(!server.shut_down))
}

/// The body of the next message, or `None` at the end of the input.
fn read_message(input: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut length = None;
    let mut seen_header = false;
    loop {
        let mut line = String::new();
        if input.read_line(&mut line)? == 0 {
            return if seen_header {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the input ended inside a header",
                ))
            } else {
                Ok(None)
            };
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            if seen_header {
                break;
            }
            continue;
        }
        seen_header = true;
        if let Some((name, value)) = line.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length =
                Some(value.trim().parse::<usize>().map_err(|error| {
                    io::Error::new(io::ErrorKind::InvalidData, format!("bad Content-Length: {error}"))
                })?);
        }
    }
    let length =
        length.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "a message without Content-Length"))?;
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    Ok(Some(body))
}

fn write_message(output: &mut impl Write, message: &Value) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(io::Error::other)?;
    write!(output, "Content-Length: {}\r\n\r\n", body.len())?;
    output.write_all(&body)?;
    output.flush()
}

fn error_reply(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

enum Step {
    Reply(Vec<Value>),
    Exit,
}

#[derive(Default)]
struct Server {
    shut_down: bool,
    /// For each root, the documents that last received a non-empty list, so a fixed file is cleared.
    published: BTreeMap<PathBuf, BTreeSet<String>>,
}

impl Server {
    fn handle(&mut self, message: &Value) -> Step {
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id");
        match (method, id) {
            (Some("exit"), _) => Step::Exit,
            (Some(method), Some(id)) => Step::Reply(vec![self.request(method, id)]),
            (Some("textDocument/didOpen" | "textDocument/didSave"), None) => {
                let uri = message.pointer("/params/textDocument/uri").and_then(Value::as_str);
                Step::Reply(uri.map(|uri| self.publish(uri)).unwrap_or_default())
            }
            // Other notifications, and responses to requests this server never sends.
            _ => Step::Reply(Vec::new()),
        }
    }

    fn request(&mut self, method: &str, id: &Value) -> Value {
        if self.shut_down {
            return error_reply(id, INVALID_REQUEST, "the server is shut down");
        }
        let result = match method {
            "initialize" => json!({
                "capabilities": {
                    "positionEncoding": "utf-16",
                    "textDocumentSync": { "openClose": true, "change": 0, "save": { "includeText": false } },
                },
                "serverInfo": { "name": "skif", "version": env!("CARGO_PKG_VERSION") },
            }),
            "shutdown" => {
                self.shut_down = true;
                Value::Null
            }
            other => return error_reply(id, METHOD_NOT_FOUND, &format!("method not found: {other}")),
        };
        json!({ "jsonrpc": "2.0", "id": id, "result": result })
    }

    /// Lints the root containing the document at `uri` and returns the `publishDiagnostics` notifications of its
    /// files, or a log message when the root cannot be linted.
    fn publish(&mut self, uri: &str) -> Vec<Value> {
        let Some(file) = uri_to_path(uri) else {
            return Vec::new();
        };
        let Some(dir) = file.parent().filter(|_| file.is_file()) else {
            return Vec::new();
        };
        let (base, report) = match lint_root(dir, &ConfigOverrides::default()) {
            Ok(linted) => linted,
            Err(error) => {
                return vec![json!({
                    "jsonrpc": "2.0",
                    "method": "window/logMessage",
                    "params": { "type": 1, "message": format!("skif: {error}") },
                })];
            }
        };
        let mut by_file: BTreeMap<PathBuf, Vec<&Diagnostic>> = BTreeMap::new();
        for diagnostic in &report.diagnostics {
            by_file
                .entry(base.join(&diagnostic.location.path))
                .or_default()
                .push(diagnostic);
        }
        let mut notifications = Vec::new();
        let mut current = BTreeSet::new();
        for (path, diagnostics) in &by_file {
            let uri = path_to_uri(path);
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let index = LineIndex::new(&text);
            let items: Vec<Value> = diagnostics
                .iter()
                .map(|diagnostic| lsp_diagnostic(diagnostic, &index, &text))
                .collect();
            notifications.push(publish_diagnostics(&uri, &items));
            current.insert(uri);
        }
        let previous = self.published.insert(base, current.clone()).unwrap_or_default();
        for uri in previous.difference(&current) {
            notifications.push(publish_diagnostics(uri, &[]));
        }
        notifications
    }
}

fn publish_diagnostics(uri: &str, diagnostics: &[Value]) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": { "uri": uri, "diagnostics": diagnostics },
    })
}

/// A 0-based LSP position, in UTF-16 code units, of a byte offset of `text`.
fn position(index: &LineIndex, offset: u32) -> Value {
    let (line, _) = index.line_col(offset);
    json!({ "line": line - 1, "character": index.utf16_col(offset) - 1 })
}

fn lsp_diagnostic(diagnostic: &Diagnostic, index: &LineIndex, text: &str) -> Value {
    let range = match &diagnostic.location.span {
        Some(span) if span.end as usize <= text.len() => {
            json!({ "start": position(index, span.start), "end": position(index, span.end) })
        }
        Some(span) => {
            let start =
                json!({ "line": span.start_line.saturating_sub(1), "character": span.start_col.saturating_sub(1) });
            let end = json!({ "line": span.end_line.saturating_sub(1), "character": span.end_col.saturating_sub(1) });
            json!({ "start": start, "end": end })
        }
        None => json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }),
    };
    let severity = match diagnostic.severity {
        Severity::Error => 1,
        Severity::Warning => 2,
        _ => 3,
    };
    let message = match &diagnostic.help {
        Some(help) => format!("{}\nhelp: {help}", diagnostic.message),
        None => diagnostic.message.clone(),
    };
    json!({
        "range": range,
        "severity": severity,
        "code": diagnostic.rule.as_str(),
        "source": "skif",
        "message": message,
    })
}

/// The bytes a `file` URI keeps as they are: unreserved characters, the path separator and the drive colon.
fn keeps(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':')
}

/// The `file` URI of an absolute path.
fn path_to_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from("file://");
    if !text.starts_with('/') {
        uri.push('/');
    }
    for byte in text.bytes() {
        if keeps(byte) {
            uri.push(char::from(byte));
        } else {
            let _ = write!(uri, "%{byte:02X}");
        }
    }
    uri
}

/// The path of a `file` URI; `None` for another scheme or a remote host.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let mut bytes = Vec::with_capacity(rest.len());
    let mut iter = rest.bytes();
    while let Some(byte) = iter.next() {
        if byte == b'%' {
            let high = iter.next().and_then(|digit| char::from(digit).to_digit(16))?;
            let low = iter.next().and_then(|digit| char::from(digit).to_digit(16))?;
            bytes.push(u8::try_from(high * 16 + low).ok()?);
        } else {
            bytes.push(byte);
        }
    }
    let text = String::from_utf8(bytes).ok()?;
    Some(local_path(&text))
}

#[cfg(windows)]
fn local_path(text: &str) -> PathBuf {
    // `/C:/dir` is the drive path `C:\dir`.
    let text = match text.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => &text[1..],
        _ => text,
    };
    PathBuf::from(text.replace('/', "\\"))
}

#[cfg(not(windows))]
fn local_path(text: &str) -> PathBuf {
    PathBuf::from(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(message: &Value) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_message(&mut bytes, message).unwrap();
        bytes
    }

    #[cfg(not(windows))]
    #[test]
    fn uris_round_trip() {
        let path = Path::new("/home/me/my repo/ünï/package.json");
        let uri = path_to_uri(path);
        assert_eq!(uri, "file:///home/me/my%20repo/%C3%BCn%C3%AF/package.json");
        assert_eq!(uri_to_path(&uri).unwrap(), path);
        assert_eq!(uri_to_path("file://localhost/a/b").unwrap(), Path::new("/a/b"));
        assert_eq!(uri_to_path("untitled:Untitled-1"), None);
        assert_eq!(uri_to_path("file://server/share/a"), None);
    }

    #[cfg(windows)]
    #[test]
    fn uris_round_trip() {
        let path = Path::new(r"C:\my repo\package.json");
        let uri = path_to_uri(path);
        assert_eq!(uri, "file:///C:/my%20repo/package.json");
        assert_eq!(uri_to_path(&uri).unwrap(), path);
        assert_eq!(uri_to_path("file:///c%3A/a/b").unwrap(), Path::new(r"c:\a\b"));
    }

    #[test]
    fn positions_count_utf16_units_from_zero() {
        let text = "{\n  \"n\": \"é😀x\"\n}";
        let index = LineIndex::new(text);
        let x = u32::try_from(text.find('x').unwrap()).unwrap();
        // Two spaces, `"n": "` (six units), `é` (one) and the emoji (two): eleven units before `x`.
        assert_eq!(position(&index, x), json!({ "line": 1, "character": 11 }));
    }

    #[test]
    fn a_session_shuts_down_and_exits_zero() {
        let mut input = Vec::new();
        for message in [
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "capabilities": {} } }),
            json!({ "jsonrpc": "2.0", "method": "initialized", "params": {} }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "textDocument/hover", "params": {} }),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "shutdown" }),
            json!({ "jsonrpc": "2.0", "method": "exit" }),
        ] {
            input.extend(frame(&message));
        }
        let mut output = Vec::new();
        assert_eq!(serve(input.as_slice(), &mut output).unwrap(), 0);
        let mut reader = output.as_slice();
        let mut replies = Vec::new();
        while let Some(body) = read_message(&mut reader).unwrap() {
            replies.push(serde_json::from_slice::<Value>(&body).unwrap());
        }
        assert_eq!(replies.len(), 3);
        assert_eq!(
            replies[0]["result"]["capabilities"]["textDocumentSync"]["openClose"],
            true
        );
        assert_eq!(replies[1]["error"]["code"], METHOD_NOT_FOUND);
        assert_eq!(replies[2], json!({ "jsonrpc": "2.0", "id": 3, "result": null }));
    }

    #[test]
    fn exit_without_shutdown_exits_one() {
        let input = frame(&json!({ "jsonrpc": "2.0", "method": "exit" }));
        assert_eq!(serve(input.as_slice(), Vec::new()).unwrap(), 1);
    }
}
