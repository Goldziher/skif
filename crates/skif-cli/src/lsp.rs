//! `skif lsp`: a minimal Language Server Protocol server over stdio (ADR 0056, ADR 0066).
//!
//! It lints the root containing a document and publishes the diagnostics of every file of that root. The lint reads
//! the text of the open documents, unsaved changes included, through a [`BufferFs`] over the disk: on open, save and
//! a close that discards changes at once, and [`DEBOUNCE`] after the last change of a root. It keeps the last lint of
//! each root to answer `textDocument/codeAction` with the fixes of those diagnostics ([`actions`]), and a module cache
//! per root, so a lint parses only the files that changed.
//!
//! A malformed frame (no `Content-Length`, one that is not a number or exceeds [`MAX_BODY`], a header line longer
//! than [`MAX_HEADER_LINE`], or input ending inside a message) cannot be skipped, since the next message's start is
//! unknown: the server writes the reason to stderr and exits with code 1.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use skif::{
    BufferFs, ConfigOverrides, Diagnostic, FileSystem, MemoryModuleCache, ModuleCache, OsFs, Severity, content_hash,
};
use skif_core::LineIndex;

use crate::lint::lint_root;
use crate::module_cache::CacheHost;
use crate::output::stderr_line;
use crate::roots::RootRun;

mod actions;

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
/// `TextDocumentSyncKind.Full`: every change carries the whole text, so no range arithmetic can drift from the
/// client's buffer.
const SYNC_FULL: u8 = 1;
/// The LSP error for a request that arrives before `initialize`.
const SERVER_NOT_INITIALIZED: i64 = -32002;
/// The largest message body accepted; a larger `Content-Length` is refused before anything is allocated.
const MAX_BODY: usize = 64 * 1024 * 1024;
/// The longest header line accepted, so a client that never sends a newline cannot make the server buffer forever.
const MAX_HEADER_LINE: u64 = 8 * 1024;
/// How long after the last change of a root its lint starts, so a burst of keystrokes causes one lint.
const DEBOUNCE: Duration = Duration::from_millis(250);

/// Serves stdin and stdout until `exit`; returns 0 when `shutdown` came first, 1 otherwise, as the protocol asks.
/// On a malformed frame or an I/O error it reports the reason on stderr and returns 1: the server exits.
pub fn run() -> u8 {
    match serve(io::BufReader::new(io::stdin()), io::stdout().lock()) {
        Ok(code) => code,
        Err(error) => {
            stderr_line(&format!("error: skif lsp: {error}; exiting"));
            1
        }
    }
}

/// Answers the framed messages of `input` on `output` until `exit` or the end of `input`, and returns the exit code.
///
/// A reader thread passes the frames over a channel, so the loop can wait for the next message and for the
/// deadline of a pending lint at once. The thread is not joined: after `exit` the server returns even when the
/// client keeps its end of the input open.
///
/// # Errors
/// An error reading `input` or writing `output`, or a malformed header.
pub fn serve(mut input: impl BufRead + Send + 'static, mut output: impl Write) -> io::Result<u8> {
    let (sender, frames) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            let frame = read_message(&mut input);
            let last = !matches!(frame, Ok(Some(_)));
            if sender.send(frame).is_err() || last {
                return;
            }
        }
    });
    let mut server = Server::default();
    loop {
        let frame = match server.next_deadline() {
            Some(deadline) => match frames.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(frame) => frame,
                Err(RecvTimeoutError::Timeout) => {
                    for notification in server.lint_due(Instant::now()) {
                        write_message(&mut output, &notification)?;
                    }
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => Ok(None),
            },
            None => frames.recv().unwrap_or(Ok(None)),
        };
        let Some(body) = frame? else {
            // The input ended: the pending lints still publish, so a scripted session sees them.
            for notification in server.lint_all_pending() {
                write_message(&mut output, &notification)?;
            }
            return Ok(u8::from(!server.shut_down));
        };
        let replies = match serde_json::from_slice::<Value>(&body) {
            Ok(message) => match server.handle(&message, Instant::now()) {
                Step::Reply(replies) => replies,
                Step::Exit => return Ok(u8::from(!server.shut_down)),
            },
            Err(error) => vec![error_reply(&Value::Null, PARSE_ERROR, &format!("parse error: {error}"))],
        };
        for reply in &replies {
            write_message(&mut output, reply)?;
        }
    }
}

/// The body of the next message, or `None` at the end of the input. A header line is read through a bounded
/// `take`, and the body is allocated only once its length is known to be at most [`MAX_BODY`].
fn read_message(input: &mut impl BufRead) -> io::Result<Option<Vec<u8>>> {
    let mut length = None;
    let mut seen_header = false;
    loop {
        let mut line = String::new();
        let read = io::Read::take(&mut *input, MAX_HEADER_LINE).read_line(&mut line)?;
        if read as u64 == MAX_HEADER_LINE && !line.ends_with('\n') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("a header line longer than {MAX_HEADER_LINE} bytes"),
            ));
        }
        if read == 0 {
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
    if length > MAX_BODY {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Content-Length {length} exceeds the limit of {MAX_BODY} bytes"),
        ));
    }
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

/// An open document: its text as the editor holds it and the version the client gave it.
struct Document {
    text: Arc<str>,
    version: Option<i64>,
}

/// One in-memory module cache per root, pruned after each lint of that root to what the lint used.
#[derive(Default)]
struct RootCaches(Mutex<BTreeMap<PathBuf, Arc<MemoryModuleCache>>>);

impl RootCaches {
    fn prune(&self, base: &Path) {
        if let Some(cache) = self.0.lock().unwrap_or_else(PoisonError::into_inner).get(base) {
            cache.prune_unused();
        }
    }
}

impl CacheHost for RootCaches {
    fn modules(&self, base: &Path) -> Arc<dyn ModuleCache> {
        let mut caches = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(caches.entry(base.to_path_buf()).or_default()) as Arc<dyn ModuleCache>
    }

    /// The server prunes the cache of the root it linted; see [`RootCaches::prune`].
    fn finish(&self) {}
}

#[derive(Default)]
struct Server {
    /// Whether `initialize` has been answered; until then requests get [`SERVER_NOT_INITIALIZED`] and
    /// notifications other than `exit` are dropped, as the protocol asks.
    initialized: bool,
    shut_down: bool,
    /// The client applies a `WorkspaceEdit` with `documentChanges`, so an edit can carry the document's version.
    document_changes: bool,
    /// For each root, the documents that last received a non-empty list, so a fixed file is cleared.
    published: BTreeMap<PathBuf, BTreeSet<String>>,
    /// For each root, the runs of its last lint, whose workspaces and reports code actions are planned from.
    runs: BTreeMap<PathBuf, Vec<RootRun>>,
    /// The open documents, by canonical path.
    documents: BTreeMap<PathBuf, Document>,
    /// For each root, the [`content_hash`] of the text of every open document its last lint read: a fix that edits
    /// an open document is offered only while the document still has that text.
    linted: BTreeMap<PathBuf, BTreeMap<PathBuf, u64>>,
    /// The lints waiting for their root to be quiet: by root (or by directory before the first lint of a root),
    /// when they are due and the directory to lint.
    pending: BTreeMap<PathBuf, (Instant, PathBuf)>,
    modules: RootCaches,
}

impl Server {
    fn handle(&mut self, message: &Value, now: Instant) -> Step {
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id");
        match (method, id) {
            (Some("exit"), _) => Step::Exit,
            (Some(method), Some(id)) if !self.initialized && method != "initialize" => Step::Reply(vec![error_reply(
                id,
                SERVER_NOT_INITIALIZED,
                "the server is not initialized",
            )]),
            (Some(_), None) if !self.initialized => Step::Reply(Vec::new()),
            (Some("shutdown"), Some(id)) if !self.shut_down => {
                // The lints still pending publish before the answer, so nothing typed is left unlinted.
                let mut replies = self.lint_all_pending();
                replies.push(self.request("shutdown", id, message));
                Step::Reply(replies)
            }
            (Some(method), Some(id)) => Step::Reply(vec![self.request(method, id, message)]),
            (Some("textDocument/didOpen"), None) => {
                let Some(file) = document_file(message) else {
                    return Step::Reply(Vec::new());
                };
                if let Some(text) = message.pointer("/params/textDocument/text").and_then(Value::as_str) {
                    self.documents.insert(
                        file.clone(),
                        Document {
                            text: Arc::from(text),
                            version: message.pointer("/params/textDocument/version").and_then(Value::as_i64),
                        },
                    );
                }
                Step::Reply(self.lint_now(&file))
            }
            (Some("textDocument/didChange"), None) => {
                let Some(file) = document_file(message) else {
                    return Step::Reply(Vec::new());
                };
                // Full sync: the last change carries the whole text.
                let text = message
                    .pointer("/params/contentChanges")
                    .and_then(Value::as_array)
                    .and_then(|changes| changes.last())
                    .and_then(|change| change.get("text"))
                    .and_then(Value::as_str);
                if let Some(text) = text {
                    self.documents.insert(
                        file.clone(),
                        Document {
                            text: Arc::from(text),
                            version: message.pointer("/params/textDocument/version").and_then(Value::as_i64),
                        },
                    );
                    self.schedule(&file, now);
                }
                Step::Reply(Vec::new())
            }
            (Some("textDocument/didSave"), None) => match document_file(message) {
                // The text is kept: it now matches the file.
                Some(file) => Step::Reply(self.lint_now(&file)),
                None => Step::Reply(Vec::new()),
            },
            (Some("textDocument/didClose"), None) => {
                let Some(file) = document_file(message) else {
                    return Step::Reply(Vec::new());
                };
                // A document closed with unsaved changes reverts to the file: its root is linted again, so no
                // diagnostic or fix is left at positions of the discarded text.
                let discarded = self.documents.remove(&file).is_some_and(|document| {
                    std::fs::read_to_string(&file).map_or(true, |saved| *saved != *document.text)
                });
                if discarded {
                    Step::Reply(self.lint_now(&file))
                } else {
                    Step::Reply(Vec::new())
                }
            }
            // Other notifications, and responses to requests this server never sends.
            _ => Step::Reply(Vec::new()),
        }
    }

    fn request(&mut self, method: &str, id: &Value, message: &Value) -> Value {
        if self.shut_down {
            return error_reply(id, INVALID_REQUEST, "the server is shut down");
        }
        let result = match method {
            "initialize" => {
                self.initialized = true;
                self.document_changes = message
                    .pointer("/params/capabilities/workspace/workspaceEdit/documentChanges")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                json!({
                "capabilities": {
                    "positionEncoding": "utf-16",
                    "textDocumentSync": {
                        "openClose": true,
                        "change": SYNC_FULL,
                        "save": { "includeText": false },
                    },
                    "codeActionProvider": { "codeActionKinds": [actions::QUICK_FIX_KIND, actions::FIX_ALL_KIND] },
                },
                "serverInfo": { "name": "skif", "version": env!("CARGO_PKG_VERSION") },
                })
            }
            "shutdown" => {
                self.shut_down = true;
                Value::Null
            }
            "textDocument/codeAction" => match self.code_actions(message.get("params").unwrap_or(&Value::Null)) {
                Some(actions) => Value::Array(actions),
                None => {
                    return error_reply(
                        id,
                        INVALID_PARAMS,
                        "invalid params: expected a document URI and a range",
                    );
                }
            },
            other => return error_reply(id, METHOD_NOT_FOUND, &format!("method not found: {other}")),
        };
        json!({ "jsonrpc": "2.0", "id": id, "result": result })
    }

    /// The root `file` belongs to: the deepest root of a lint that contains it, else its directory.
    fn root_of(&self, file: &Path) -> Option<PathBuf> {
        self.runs
            .keys()
            .filter(|base| file.starts_with(base))
            .max_by_key(|base| base.components().count())
            .cloned()
            .or_else(|| file.parent().map(Path::to_path_buf))
    }

    /// Schedules a lint of the root of `file`, [`DEBOUNCE`] after `now`, replacing the one pending for that root.
    fn schedule(&mut self, file: &Path, now: Instant) {
        if let (Some(root), Some(dir)) = (self.root_of(file), file.parent()) {
            self.pending.insert(root, (now + DEBOUNCE, dir.to_path_buf()));
        }
    }

    /// The earliest deadline of a pending lint.
    fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().map(|(deadline, _)| *deadline).min()
    }

    /// Takes the directories whose lint is due at `now`, in root order.
    fn take_due(&mut self, now: Instant) -> Vec<PathBuf> {
        let due: Vec<PathBuf> = self
            .pending
            .iter()
            .filter(|(_, (deadline, _))| *deadline <= now)
            .map(|(root, _)| root.clone())
            .collect();
        due.into_iter()
            .filter_map(|root| self.pending.remove(&root).map(|(_, dir)| dir))
            .collect()
    }

    /// Runs the lints due at `now` and returns their notifications.
    fn lint_due(&mut self, now: Instant) -> Vec<Value> {
        let dirs = self.take_due(now);
        dirs.iter().flat_map(|dir| self.lint_dir(dir)).collect()
    }

    /// Runs every pending lint, due or not, and returns their notifications.
    fn lint_all_pending(&mut self) -> Vec<Value> {
        let dirs: Vec<PathBuf> = std::mem::take(&mut self.pending)
            .into_values()
            .map(|(_, dir)| dir)
            .collect();
        dirs.iter().flat_map(|dir| self.lint_dir(dir)).collect()
    }

    /// Lints the root of `file` now, cancelling the lint pending for it.
    fn lint_now(&mut self, file: &Path) -> Vec<Value> {
        if let Some(root) = self.root_of(file) {
            self.pending.remove(&root);
        }
        match file.parent().filter(|_| file.is_file()) {
            Some(dir) => self.lint_dir(dir),
            None => Vec::new(),
        }
    }

    /// The open documents over the disk: what a lint and a fix plan read.
    fn overlay(&self) -> Arc<dyn FileSystem> {
        let buffers = self
            .documents
            .iter()
            .map(|(path, document)| (path.clone(), Arc::clone(&document.text)))
            .collect();
        Arc::new(BufferFs::new(OsFs, buffers))
    }

    /// Lints the root containing `dir` and returns the `publishDiagnostics` notifications of its files, or a log
    /// message when the root cannot be linted.
    fn lint_dir(&mut self, dir: &Path) -> Vec<Value> {
        let fs = self.overlay();
        let linted = lint_root(dir, &ConfigOverrides::default(), &fs, Some(&self.modules));
        let (base, report, runs) = match linted {
            Ok(linted) => linted,
            Err(error) => {
                return vec![json!({
                    "jsonrpc": "2.0",
                    "method": "window/logMessage",
                    "params": { "type": 1, "message": format!("skif: {error}") },
                })];
            }
        };
        self.modules.prune(&base);
        let read: BTreeMap<PathBuf, u64> = self
            .documents
            .iter()
            .filter(|(path, _)| path.starts_with(&base))
            .map(|(path, document)| (path.clone(), content_hash(document.text.as_bytes())))
            .collect();
        self.linted.insert(base.clone(), read);
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
            // Positions are computed in the text the lint read: the buffer of an open document.
            let text = fs.read_to_string(path).unwrap_or_default();
            let index = LineIndex::new(&text);
            let items: Vec<Value> = diagnostics
                .iter()
                .map(|diagnostic| lsp_diagnostic(diagnostic, &index, &text))
                .collect();
            notifications.push(publish_diagnostics(&uri, &items));
            current.insert(uri);
        }
        self.runs.insert(base.clone(), runs);
        let previous = self.published.insert(base, current.clone()).unwrap_or_default();
        for uri in previous.difference(&current) {
            notifications.push(publish_diagnostics(uri, &[]));
        }
        notifications
    }
}

impl Server {
    /// The code actions for the `CodeActionParams` `params`, or `None` when they name no document or range. A
    /// document that is not a local file, or that no lint has covered yet, has none.
    fn code_actions(&self, params: &Value) -> Option<Vec<Value>> {
        let uri = params.pointer("/textDocument/uri")?.as_str()?;
        let range = params.get("range").filter(|range| range.is_object())?;
        let only = params
            .pointer("/context/only")
            .and_then(Value::as_array)
            .map(|kinds| kinds.iter().filter_map(Value::as_str).map(str::to_owned).collect());
        let Some(file) = local_file(uri) else {
            return Some(Vec::new());
        };
        let versions: BTreeMap<PathBuf, (u64, Option<i64>)> = self
            .documents
            .iter()
            .map(|(path, document)| (path.clone(), (content_hash(document.text.as_bytes()), document.version)))
            .collect();
        let request = actions::Request {
            file: &file,
            range: (
                actions::position_of(&range["start"]),
                actions::position_of(&range["end"]),
            ),
            only,
            documents: &versions,
            linted: &self.linted,
            document_changes: self.document_changes,
        };
        Some(actions::code_actions(&request, &self.runs, self.overlay().as_ref()))
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

/// The canonical path of the document of a `textDocument` notification, when it is a local file.
fn document_file(message: &Value) -> Option<PathBuf> {
    local_file(message.pointer("/params/textDocument/uri")?.as_str()?)
}

/// The path of a `file` URI, canonical when the file exists, so it compares equal to the paths of a lint, whose
/// root is canonical, however the client spells the URI.
fn local_file(uri: &str) -> Option<PathBuf> {
    let path = uri_to_path(uri)?;
    Some(std::fs::canonicalize(&path).map_or(path, crate::lint::simplified))
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
        assert_eq!(serve(io::Cursor::new(input), &mut output).unwrap(), 0);
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
    fn a_content_length_above_the_cap_is_invalid_data_without_allocating() {
        let input = format!("Content-Length: {}\r\n\r\n{{}}", MAX_BODY + 1);
        let error = read_message(&mut input.as_bytes()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
        // A length near `usize::MAX` is refused the same way, not attempted.
        let input = format!("Content-Length: {}\r\n\r\n", usize::MAX);
        let error = read_message(&mut input.as_bytes()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
    }

    #[test]
    fn a_header_line_without_a_newline_is_cut_off() {
        let input = vec![b'a'; usize::try_from(MAX_HEADER_LINE).unwrap() * 4];
        let error = read_message(&mut input.as_slice()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData, "{error}");
    }

    #[test]
    fn requests_before_initialize_are_refused() {
        let mut input = Vec::new();
        for message in [
            json!({ "jsonrpc": "2.0", "id": 1, "method": "textDocument/hover", "params": {} }),
            json!({ "jsonrpc": "2.0", "id": 2, "method": "shutdown" }),
            json!({ "jsonrpc": "2.0", "id": 3, "method": "initialize", "params": { "capabilities": {} } }),
            json!({ "jsonrpc": "2.0", "method": "exit" }),
        ] {
            input.extend(frame(&message));
        }
        let mut output = Vec::new();
        assert_eq!(serve(io::Cursor::new(input), &mut output).unwrap(), 1);
        let mut reader = output.as_slice();
        let mut replies = Vec::new();
        while let Some(body) = read_message(&mut reader).unwrap() {
            replies.push(serde_json::from_slice::<Value>(&body).unwrap());
        }
        assert_eq!(replies.len(), 3);
        assert_eq!(replies[0]["error"]["code"], SERVER_NOT_INITIALIZED);
        assert_eq!(replies[1]["error"]["code"], SERVER_NOT_INITIALIZED);
        assert!(replies[2]["result"]["capabilities"].is_object(), "{}", replies[2]);
    }

    #[test]
    fn a_burst_of_changes_schedules_one_lint_after_the_last() {
        let start = Instant::now();
        let ms = Duration::from_millis(1);
        let mut server = Server::default();
        let file = Path::new("/ws/packages/a/package.json");
        server.schedule(file, start);
        server.schedule(file, start + 100 * ms);
        server.schedule(&Path::new("/ws/packages/a").join("index.js"), start + 200 * ms);
        assert_eq!(server.pending.len(), 1);
        assert_eq!(server.next_deadline(), Some(start + 200 * ms + DEBOUNCE));
        assert_eq!(server.take_due(start + 300 * ms), Vec::<PathBuf>::new());
        assert_eq!(
            server.take_due(start + 200 * ms + DEBOUNCE),
            [PathBuf::from("/ws/packages/a")]
        );
        assert_eq!(server.next_deadline(), None);
    }

    #[test]
    fn exit_without_shutdown_exits_one() {
        let input = frame(&json!({ "jsonrpc": "2.0", "method": "exit" }));
        assert_eq!(serve(io::Cursor::new(input), Vec::new()).unwrap(), 1);
    }
}
