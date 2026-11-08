//! `textDocument/codeAction` for `skif lsp`: the fixes of the last lint of a root, planned by [`skif::apply_fixes`]
//! as `skif --fix --dry-run` plans them and offered as `WorkspaceEdit`s.
//!
//! Each fix of a diagnostic under the requested range is a `quickfix`; an unsafe fix is offered too, with
//! ` (unsafe)` after its title, because choosing it is the explicit consent `--fix-unsafe` stands for on the
//! command line. A `source.fixAll.skif` action applies every safe fix of the document's diagnostics, taking the
//! alternative `[policy] prefer` names where there are several, as a non-interactive `skif --fix` does. A plan that
//! creates or deletes a file is never offered (deleting files stays a CLI opt-in, ADR 0052).
//!
//! Plans read the open documents' buffers (ADR 0066), so their edits are positions in the text the editor holds. A
//! plan is offered only when every file it edits still has the text it was planned against, and every open document
//! it edits still has the text the last lint of its root read; otherwise it waits for the next lint. When the client
//! supports `documentChanges`, the edit of an open document carries its version, so the client refuses it if the
//! user typed in between.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use skif::{
    Alternatives, CachedFs, Diagnostic, FileChange, FileSystem, FixPlan, FixSafety, FixSelection, Report, apply_fixes,
    content_hash,
};
use skif_core::LineIndex;

use super::{lsp_diagnostic, path_to_uri, position};
use crate::roots::RootRun;

/// The kind of the action that applies every safe fix of a document.
pub const FIX_ALL_KIND: &str = "source.fixAll.skif";
/// The kind of the action that applies one fix of one diagnostic.
pub const QUICK_FIX_KIND: &str = "quickfix";

/// A 0-based LSP position as `(line, character)`, ordered as the protocol orders positions.
type Position = (u64, u64);

/// What a `textDocument/codeAction` request asks for.
pub struct Request<'a> {
    /// The document, as a path.
    pub file: &'a Path,
    /// The requested range.
    pub range: (Position, Position),
    /// `context.only`: the kinds the client wants, or `None` for every kind.
    pub only: Option<Vec<String>>,
    /// The open documents by canonical path: the [`content_hash`] of their current text and their version.
    pub documents: &'a BTreeMap<PathBuf, (u64, Option<i64>)>,
    /// For each root, the hash of every open document's text its last lint read.
    pub linted: &'a BTreeMap<PathBuf, BTreeMap<PathBuf, u64>>,
    /// Whether the client takes `documentChanges` in a `WorkspaceEdit`.
    pub document_changes: bool,
}

/// The edits of one file: the version of its open document, if it is one, and the LSP `TextEdit`s.
type FileEdits = (Option<i64>, Vec<Value>);

/// The code actions of `request` from the runs of the last lint of each root, keyed by the directory their prefixes
/// are relative to. Files are read through `fs`, the open documents over the disk.
pub fn code_actions(request: &Request<'_>, roots: &BTreeMap<PathBuf, Vec<RootRun>>, fs: &dyn FileSystem) -> Vec<Value> {
    let fs = CachedFs::new(fs);
    let mut quick_fixes = Vec::new();
    let mut fix_all: BTreeMap<String, FileEdits> = BTreeMap::new();
    for (base, runs) in roots {
        let check = Check {
            request,
            linted: request.linted.get(base),
        };
        for run in runs {
            let in_file: Vec<&Diagnostic> = run
                .report
                .diagnostics
                .iter()
                .filter(|diagnostic| base.join(run.prefix.join(&diagnostic.location.path)) == request.file)
                .collect();
            if in_file.is_empty() {
                continue;
            }
            let text = fs.read_to_string(request.file).unwrap_or_default();
            let index = LineIndex::new(&text);
            if wants(request.only.as_deref(), QUICK_FIX_KIND) {
                for diagnostic in &in_file {
                    let lsp = lsp_diagnostic(diagnostic, &index, &text);
                    if overlaps(&lsp["range"], request.range) {
                        quick_fixes.extend(fixes_of(run, diagnostic, &lsp, &check, &fs));
                    }
                }
            }
            if wants(request.only.as_deref(), FIX_ALL_KIND) {
                let report = Report::new(in_file.into_iter().cloned().collect(), 0);
                let selection = FixSelection::new(FixSafety::Safe)
                    .with_alternatives(Alternatives::Preferred(run.config.policy.prefer));
                if let Ok(plan) = apply_fixes(&run.workspace, &report, &selection, &fs)
                    && let Some(changes) = workspace_changes(&plan, &run.workspace.root, &check, &fs)
                {
                    for (uri, (version, edits)) in changes {
                        fix_all
                            .entry(uri)
                            .or_insert_with(|| (version, Vec::new()))
                            .1
                            .extend(edits);
                    }
                }
            }
        }
    }
    if !fix_all.is_empty() {
        quick_fixes.push(json!({
            "title": "Fix all safe skif problems in this file",
            "kind": FIX_ALL_KIND,
            "edit": workspace_edit(fix_all, request.document_changes),
        }));
    }
    quick_fixes
}

/// One `quickfix` per fix of `diagnostic` that plans to edits of existing files that still hold the linted text.
fn fixes_of(run: &RootRun, diagnostic: &Diagnostic, lsp: &Value, check: &Check<'_>, fs: &dyn FileSystem) -> Vec<Value> {
    let report = Report::new(vec![diagnostic.clone()], 0);
    let preferred = diagnostic.fixes.len() == 1;
    diagnostic
        .fixes
        .iter()
        .enumerate()
        .filter_map(|(number, fix)| {
            let selection = FixSelection::new(FixSafety::Unsafe).choose(0, number);
            let plan = apply_fixes(&run.workspace, &report, &selection, fs).ok()?;
            if plan.applied.is_empty() {
                return None;
            }
            let changes = workspace_changes(&plan, &run.workspace.root, check, fs)?;
            let (title, is_preferred) = match fix.safety {
                FixSafety::Safe => (fix.title.clone(), preferred),
                _ => (format!("{} (unsafe)", fix.title), false),
            };
            Some(json!({
                "title": title,
                "kind": QUICK_FIX_KIND,
                "diagnostics": [lsp],
                "isPreferred": is_preferred,
                "edit": workspace_edit(changes, check.request.document_changes),
            }))
        })
        .collect()
}

/// What a plan's edits are checked against: the open documents of the request and the texts the root's last lint
/// read.
struct Check<'a> {
    request: &'a Request<'a>,
    linted: Option<&'a BTreeMap<PathBuf, u64>>,
}

impl Check<'_> {
    /// Whether `file`, whose current text hashes to `hash`, may be edited: a file without an open document always
    /// may, an open document only while it has the text the last lint read.
    fn editable(&self, file: &Path, hash: u64) -> bool {
        self.request.documents.get(file).is_none_or(|(current, _)| {
            *current == hash && self.linted.and_then(|linted| linted.get(file)) == Some(&hash)
        })
    }

    /// The version of the open document of `file`, if it is one and the client gave one.
    fn version(&self, file: &Path) -> Option<i64> {
        self.request.documents.get(file).and_then(|(_, version)| *version)
    }
}

/// The edits of `plan` by document URI, or `None` when the plan changes nothing, creates or deletes a file, or edits
/// a file that no longer has the text it was planned against or an open document the last lint did not read as it is.
fn workspace_changes(
    plan: &FixPlan,
    root: &Path,
    check: &Check<'_>,
    fs: &dyn FileSystem,
) -> Option<BTreeMap<String, FileEdits>> {
    if plan.is_empty() {
        return None;
    }
    let mut changes = BTreeMap::new();
    for change in &plan.changes {
        let FileChange::Modify { path, edits, .. } = change else {
            return None;
        };
        let file = root.join(path);
        let source = fs.read_to_string(&file).ok()?;
        let hash = content_hash(source.as_bytes());
        if plan.base_hash(path) != Some(hash) {
            return None;
        }
        if !check.editable(&file, hash) {
            return None;
        }
        let version = check.version(&file);
        let index = LineIndex::new(&source);
        let edits: Vec<Value> = edits
            .iter()
            .map(|edit| {
                json!({
                    "range": { "start": position(&index, edit.start), "end": position(&index, edit.end) },
                    "newText": edit.replacement,
                })
            })
            .collect();
        changes.insert(path_to_uri(&file), (version, edits));
    }
    Some(changes)
}

/// The `WorkspaceEdit` of `changes`: `documentChanges` with each document's version when the client supports them
/// (a file without an open document has version `null`), else `changes`.
fn workspace_edit(changes: BTreeMap<String, FileEdits>, document_changes: bool) -> Value {
    if document_changes {
        let documents: Vec<Value> = changes
            .into_iter()
            .map(
                |(uri, (version, edits))| json!({ "textDocument": { "uri": uri, "version": version }, "edits": edits }),
            )
            .collect();
        json!({ "documentChanges": documents })
    } else {
        let changes: BTreeMap<String, Vec<Value>> = changes.into_iter().map(|(uri, (_, edits))| (uri, edits)).collect();
        json!({ "changes": changes })
    }
}

/// Whether `kind` is one of the kinds `only` asks for: equal to one, or below one in the dotted hierarchy.
fn wants(only: Option<&[String]>, kind: &str) -> bool {
    only.is_none_or(|only| {
        only.iter().any(|wanted| {
            kind == wanted
                || kind
                    .strip_prefix(wanted.as_str())
                    .is_some_and(|rest| rest.starts_with('.'))
        })
    })
}

/// A JSON position as `(line, character)`; a missing field is 0.
pub fn position_of(value: &Value) -> Position {
    let field = |name: &str| value.get(name).and_then(Value::as_u64).unwrap_or(0);
    (field("line"), field("character"))
}

/// Whether the JSON range `range` and `requested` share a position; an empty range touching the other counts.
fn overlaps(range: &Value, requested: (Position, Position)) -> bool {
    let (start, end) = (position_of(&range["start"]), position_of(&range["end"]));
    start <= requested.1 && requested.0 <= end
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_selects_a_kind_and_its_sub_kinds() {
        let only = |kinds: &[&str]| kinds.iter().map(|kind| (*kind).to_owned()).collect::<Vec<_>>();
        assert!(wants(None, FIX_ALL_KIND));
        assert!(wants(Some(&only(&["source"])), FIX_ALL_KIND));
        assert!(wants(Some(&only(&["source.fixAll"])), FIX_ALL_KIND));
        assert!(!wants(Some(&only(&["source.fix"])), FIX_ALL_KIND));
        assert!(!wants(Some(&only(&["quickfix"])), FIX_ALL_KIND));
        assert!(wants(Some(&only(&["refactor", "quickfix"])), QUICK_FIX_KIND));
    }

    #[test]
    fn ranges_overlap_when_they_share_a_position() {
        let range = json!({ "start": { "line": 6, "character": 2 }, "end": { "line": 6, "character": 23 } });
        assert!(overlaps(&range, ((6, 5), (6, 5))));
        assert!(overlaps(&range, ((6, 23), (7, 0))));
        assert!(overlaps(&range, ((0, 0), (9, 0))));
        assert!(!overlaps(&range, ((6, 24), (6, 30))));
        assert!(!overlaps(&range, ((5, 0), (6, 1))));
    }
}
