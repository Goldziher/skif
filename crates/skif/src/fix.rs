//! Turning the fixes attached to a report into a plan of file changes, without touching the file system.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_config::Prefer;
use skif_core::{
    Diagnostic, FileChange, FileSystem, Fix, FixOp, FixPreference, FixSafety, Report, RuleId, TextEdit,
    apply_text_edits, slash,
};
use skif_edit::{EditPlanError, FileKind, plan_edits};
use skif_workspace::Workspace;

/// What to do when a diagnostic has several alternative fixes and `choices` holds no pick for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Alternatives {
    /// Leave the diagnostic unfixed and record [`SkipReason::NeedsChoice`], so the caller can ask.
    #[default]
    Ask,
    /// Take the first alternative, which a rule orders by configuration (`[policy] prefer`).
    First,
    /// Take the alternative tagged with the preferred version (see [`preferred_alternative`]), as `[policy] prefer`
    /// determines it; leave the diagnostic unfixed ([`SkipReason::NeedsChoice`]) when no alternative names it.
    Preferred(Prefer),
}

/// The version `prefer` selects, as a fix carries it.
const fn preference_of(prefer: Prefer) -> FixPreference {
    match prefer {
        Prefer::Lowest => FixPreference::Lowest,
        _ => FixPreference::Highest,
    }
}

/// The index of the alternative in `fixes` that carries the [`FixPreference`] `prefer` selects, when there is one.
///
/// A rule tags the alternatives that align on a version ([`Fix::with_preference`]); configuration determines the
/// answer when `[policy] prefer` selects one of those tags. The title is only text for the user.
///
/// # Examples
/// ```
/// use skif::{Fix, FixPreference, FixSafety, Prefer, preferred_alternative};
///
/// let fix = |title: &str, preference| Fix::new(title.to_owned(), FixSafety::Unsafe, Vec::new()).with_preference(preference);
/// let fixes = [fix("Use 2.0.0", FixPreference::Highest), fix("Use 0.1.0", FixPreference::Lowest)];
/// assert_eq!(preferred_alternative(&fixes, Prefer::Lowest), Some(1));
/// assert_eq!(preferred_alternative(&fixes[..1], Prefer::Lowest), None);
/// ```
#[must_use]
pub fn preferred_alternative(fixes: &[Fix], prefer: Prefer) -> Option<usize> {
    let wanted = Some(preference_of(prefer));
    fixes.iter().position(|fix| fix.preference == wanted)
}

/// Which fixes go into a [`FixPlan`].
///
/// # Examples
/// ```
/// use skif::{Alternatives, FixSafety, FixSelection};
///
/// let selection = FixSelection::new(FixSafety::Unsafe)
///     .with_alternatives(Alternatives::First)
///     .choose(3, 1);
/// assert_eq!(selection.safety, FixSafety::Unsafe);
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct FixSelection {
    /// The highest safety level to include: `Safe` takes only safe fixes, `Unsafe` takes both.
    pub safety: FixSafety,
    /// Only fixes of these rule IDs; `None` means every rule.
    pub rules: Option<BTreeSet<String>>,
    /// Explicit picks: index into `Report::diagnostics` to index into that diagnostic's `fixes`.
    pub choices: BTreeMap<usize, usize>,
    /// How diagnostics with several fixes and no pick are handled.
    pub alternatives: Alternatives,
}

impl FixSelection {
    /// A selection taking fixes up to `safety`, from every rule, asking when there are alternatives.
    #[must_use]
    pub const fn new(safety: FixSafety) -> Self {
        Self {
            safety,
            rules: None,
            choices: BTreeMap::new(),
            alternatives: Alternatives::Ask,
        }
    }

    /// Restricts the selection to the fixes of `rules`.
    #[must_use]
    pub fn with_rules(mut self, rules: impl IntoIterator<Item = String>) -> Self {
        self.rules = Some(rules.into_iter().collect());
        self
    }

    /// Sets how alternatives without a pick are handled.
    #[must_use]
    pub const fn with_alternatives(mut self, alternatives: Alternatives) -> Self {
        self.alternatives = alternatives;
        self
    }

    /// Picks alternative `fix` of the diagnostic at index `diagnostic` of the report.
    #[must_use]
    pub fn choose(mut self, diagnostic: usize, fix: usize) -> Self {
        self.choices.insert(diagnostic, fix);
        self
    }

    /// Whether a fix of `safety` is within the selection's safety ceiling.
    #[must_use]
    pub fn allows(&self, safety: FixSafety) -> bool {
        self.safety == FixSafety::Unsafe || safety == FixSafety::Safe
    }

    fn includes(&self, rule: &RuleId) -> bool {
        self.rules.as_ref().is_none_or(|rules| rules.contains(rule.as_str()))
    }
}

impl Default for FixSelection {
    fn default() -> Self {
        Self::new(FixSafety::Safe)
    }
}

/// A fix that is part of the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AppliedFix {
    /// Index of the diagnostic in `Report::diagnostics`.
    pub diagnostic: usize,
    /// Rule that reported the diagnostic.
    pub rule: RuleId,
    /// Message of the diagnostic.
    pub message: String,
    /// Title of the chosen fix.
    pub title: String,
}

/// Why a diagnostic's fix is not in the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkipReason {
    /// Every candidate needs a higher safety level than the selection allows.
    Safety(FixSafety),
    /// Several alternatives exist and neither a pick nor [`Alternatives::First`] chose one.
    NeedsChoice {
        /// Titles of the alternatives, in order.
        alternatives: Vec<String>,
    },
    /// The fix overlaps an earlier fix, or targets a file another fix creates or deletes.
    Conflict(String),
    /// The fix could not be planned (unreadable file, path missing from the document, invalid pick).
    Failed(String),
}

/// A diagnostic whose fix is not in the plan.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SkippedFix {
    /// Index of the diagnostic in `Report::diagnostics`.
    pub diagnostic: usize,
    /// Rule that reported the diagnostic.
    pub rule: RuleId,
    /// Message of the diagnostic.
    pub message: String,
    /// Why nothing was planned.
    pub reason: SkipReason,
}

/// The file changes selected fixes amount to. Paths are relative to the workspace root; nothing has been written.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct FixPlan {
    /// One change per file, sorted by path.
    pub changes: Vec<FileChange>,
    /// Fixes included in `changes`, in diagnostic order.
    pub applied: Vec<AppliedFix>,
    /// Fixes left out, in diagnostic order.
    pub skipped: Vec<SkippedFix>,
    /// [`content_hash`] of the contents each modified or deleted file had when it was planned.
    bases: BTreeMap<PathBuf, u64>,
}

/// A stable 64-bit hash (FNV-1a) of file contents, for the staleness guard of a [`FixPlan`]: see
/// [`FixPlan::base_hash`]. It is not cryptographic and will not change between releases.
#[must_use]
pub fn content_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl FixPlan {
    /// The [`content_hash`] of the file at `path` as the plan read it, for a file the plan modifies or deletes. A host that
    /// writes the plan later compares it with the hash of the file as it is then and refuses to write over a file
    /// that changed in between.
    #[must_use]
    pub fn base_hash(&self, path: &Path) -> Option<u64> {
        self.bases.get(path).copied()
    }

    /// Records `hash` as the [`content_hash`] the file at `path` had when the plan was made, for a host that assembles
    /// or merges plans itself.
    #[must_use]
    pub fn with_base_hash(mut self, path: impl Into<PathBuf>, hash: u64) -> Self {
        self.bases.insert(path.into(), hash);
        self
    }

    /// Whether the plan changes no file.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// The workspace-relative path each change touches.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.changes.iter().map(FileChange::path)
    }
}

/// Planning the fixes failed as a whole (as opposed to a single fix being skipped).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FixError {
    /// The edits of a file that every accepted fix agreed on could not be combined.
    #[error("cannot plan edits for {}: {message}", slash(.path))]
    Plan {
        /// Workspace-relative file.
        path: PathBuf,
        /// What went wrong.
        message: String,
    },
}

/// Why the operations aimed at one file cannot be planned.
enum PlanFailure {
    Conflict(String),
    Other(String),
}

impl PlanFailure {
    fn message(&self) -> &str {
        match self {
            Self::Conflict(message) | Self::Other(message) => message,
        }
    }

    fn into_skip(self, path: &Path) -> SkipReason {
        let text = format!("{}: {}", slash(path), self.message());
        match self {
            Self::Conflict(_) => SkipReason::Conflict(text),
            Self::Other(_) => SkipReason::Failed(text),
        }
    }
}

/// What the accepted fixes do to one file.
enum Entry<'a> {
    /// The operations of every accepted fix (identical ones once) and the edits they planned to, sorted by position.
    Edit {
        ops: Vec<&'a FixOp>,
        edits: Vec<TextEdit>,
    },
    Create(&'a str),
    Delete,
}

/// The effect one fix has on one file, computed before anything is accepted.
enum Staged<'a> {
    Edit {
        ops: Vec<&'a FixOp>,
        edits: Vec<TextEdit>,
    },
    Create(&'a str),
    Delete,
    /// Everything the fix does to the file is already in the plan.
    Nothing,
}

struct Planner<'a> {
    root: &'a Path,
    fs: &'a dyn FileSystem,
    sources: BTreeMap<PathBuf, Arc<str>>,
    entries: BTreeMap<PathBuf, Entry<'a>>,
    /// The diagnostics whose accepted fixes touch each path, in the order they were accepted.
    contributors: BTreeMap<PathBuf, Vec<usize>>,
}

/// The accepted fixes of one file do not combine into a valid file.
struct Invalid {
    path: PathBuf,
    message: String,
    /// The diagnostic accepted last for the file; dropping it is the first thing to try.
    culprit: Option<usize>,
}

fn op_path(op: &FixOp) -> Option<&Path> {
    match op {
        FixOp::Json { path, .. }
        | FixOp::Yaml { path, .. }
        | FixOp::Text { path, .. }
        | FixOp::CreateFile { path, .. }
        | FixOp::DeleteFile { path, .. } => Some(path),
        _ => None,
    }
}

/// Whether two edits replace overlapping ranges; touching ranges and inserts at a range boundary do not overlap.
const fn overlaps(first: &TextEdit, second: &TextEdit) -> bool {
    first.start < second.end && second.start < first.end
}

/// Whether `edit` collides with a different edit of `accepted` (sorted by position, pairwise non-overlapping).
fn collides(accepted: &[TextEdit], edit: &TextEdit) -> bool {
    let first = accepted.partition_point(|other| other.end <= edit.start);
    accepted
        .get(first)
        .is_some_and(|other| other != edit && overlaps(other, edit))
}

fn contains(accepted: &[TextEdit], edit: &TextEdit) -> bool {
    let first = accepted.partition_point(|other| (other.start, other.end) < (edit.start, edit.end));
    accepted[first..]
        .iter()
        .take_while(|other| (other.start, other.end) == (edit.start, edit.end))
        .any(|other| other == edit)
}

fn insert_sorted(accepted: &mut Vec<TextEdit>, edit: TextEdit) {
    let at = accepted.partition_point(|other| (other.start, other.end) <= (edit.start, edit.end));
    accepted.insert(at, edit);
}

impl<'a> Planner<'a> {
    fn source(&mut self, path: &Path) -> Result<Arc<str>, String> {
        if let Some(source) = self.sources.get(path) {
            return Ok(Arc::clone(source));
        }
        let source = self
            .fs
            .read_to_string(&self.root.join(path))
            .map_err(|error| format!("cannot read {}: {error}", slash(path)))?;
        self.sources.insert(path.to_path_buf(), Arc::clone(&source));
        Ok(source)
    }

    /// Adds every op of `fix` or none: the plan is only extended once all of the fix's files check out.
    fn try_add(&mut self, index: usize, fix: &'a Fix) -> Result<(), SkipReason> {
        let mut by_file: BTreeMap<&Path, Vec<&'a FixOp>> = BTreeMap::new();
        for op in &fix.ops {
            let path = op_path(op).ok_or_else(|| SkipReason::Failed("unsupported operation".to_owned()))?;
            by_file.entry(path).or_default().push(op);
        }
        let mut staged: Vec<(PathBuf, Staged<'a>)> = Vec::new();
        for (path, ops) in by_file {
            staged.push((path.to_path_buf(), self.stage(path, &ops)?));
        }
        for (path, change) in staged {
            self.contributors.entry(path.clone()).or_default().push(index);
            self.commit(path, change);
        }
        Ok(())
    }

    fn stage(&mut self, path: &Path, ops: &[&'a FixOp]) -> Result<Staged<'a>, SkipReason> {
        let conflict = |what: &str| SkipReason::Conflict(format!("{}: {what}", slash(path)));
        let creates = ops.iter().copied().find_map(|op| match op {
            FixOp::CreateFile { contents, .. } => Some(contents.as_str()),
            _ => None,
        });
        let deletes = ops.iter().any(|op| matches!(op, FixOp::DeleteFile { .. }));
        let edits = ops
            .iter()
            .any(|op| matches!(op, FixOp::Json { .. } | FixOp::Yaml { .. } | FixOp::Text { .. }));
        let existing = self.entries.get(path);
        match (creates, deletes, edits) {
            (Some(contents), false, false) => match existing {
                None if self.fs.is_file(&self.root.join(path)) => Err(SkipReason::Failed(format!(
                    "{}: cannot create a file that already exists",
                    slash(path)
                ))),
                None => Ok(Staged::Create(contents)),
                Some(Entry::Create(earlier)) if *earlier == contents => Ok(Staged::Nothing),
                Some(_) => Err(conflict("another fix already touches a file this fix creates")),
            },
            (None, true, false) => match existing {
                None => Ok(Staged::Delete),
                Some(Entry::Delete) => Ok(Staged::Nothing),
                Some(_) => Err(conflict("another fix already touches a file this fix deletes")),
            },
            (None, false, true) => self.stage_edits(path, ops),
            _ => Err(conflict("a single fix mixes creating, deleting and editing one file")),
        }
    }

    /// Plans the operations of one fix on their own, then checks the result against the edits already accepted.
    fn stage_edits(&mut self, path: &Path, ops: &[&'a FixOp]) -> Result<Staged<'a>, SkipReason> {
        let conflict = |what: &str| SkipReason::Conflict(format!("{}: {what}", slash(path)));
        let fresh: Vec<&'a FixOp> = {
            let earlier: &[&FixOp] = match self.entries.get(path) {
                None => &[],
                Some(Entry::Edit { ops, .. }) => ops.as_slice(),
                Some(_) => return Err(conflict("another fix creates or deletes a file this fix edits")),
            };
            let mut fresh: Vec<&'a FixOp> = Vec::new();
            for &op in ops {
                if !earlier.contains(&op) && !fresh.contains(&op) {
                    fresh.push(op);
                }
            }
            fresh
        };
        if fresh.is_empty() {
            return Ok(Staged::Nothing);
        }
        let planned = self.plan(path, &fresh).map_err(|failure| failure.into_skip(path))?;
        let accepted: &[TextEdit] = match self.entries.get(path) {
            Some(Entry::Edit { edits, .. }) => edits.as_slice(),
            _ => &[],
        };
        let edits: Vec<TextEdit> = planned.into_iter().filter(|edit| !contains(accepted, edit)).collect();
        if edits.iter().any(|edit| collides(accepted, edit)) {
            return Err(conflict("the edits overlap an earlier fix"));
        }
        Ok(Staged::Edit { ops: fresh, edits })
    }

    fn commit(&mut self, path: PathBuf, change: Staged<'a>) {
        match change {
            Staged::Nothing => {}
            Staged::Create(contents) => {
                self.entries.insert(path, Entry::Create(contents));
            }
            Staged::Delete => {
                self.entries.insert(path, Entry::Delete);
            }
            Staged::Edit { ops, edits } => match self.entries.entry(path) {
                std::collections::btree_map::Entry::Occupied(mut occupied) => {
                    if let Entry::Edit {
                        ops: accepted_ops,
                        edits: accepted_edits,
                    } = occupied.get_mut()
                    {
                        accepted_ops.extend(ops);
                        for edit in edits {
                            insert_sorted(accepted_edits, edit);
                        }
                    }
                }
                std::collections::btree_map::Entry::Vacant(vacant) => {
                    let mut sorted = Vec::with_capacity(edits.len());
                    for edit in edits {
                        insert_sorted(&mut sorted, edit);
                    }
                    vacant.insert(Entry::Edit { ops, edits: sorted });
                }
            },
        }
    }

    /// The text edits `ops` amount to over the current contents of `path`.
    fn plan(&mut self, path: &Path, ops: &[&FixOp]) -> Result<Vec<TextEdit>, PlanFailure> {
        let source = self.source(path).map_err(PlanFailure::Other)?;
        match FileKind::from_path(path) {
            Some(kind) => plan_edits(&source, kind, ops).map_err(|error| plan_failure(&error)),
            None => text_edits(&source, ops),
        }
    }

    /// The contents every accepted fix amounts to. Each file is checked as a whole: the accepted fixes were planned one
    /// at a time, so only the combined result proves that they still form a document that parses.
    fn finish(mut self) -> Result<(Vec<FileChange>, BTreeMap<PathBuf, u64>), Invalid> {
        let entries = std::mem::take(&mut self.entries);
        let mut changes = Vec::with_capacity(entries.len());
        let mut bases = BTreeMap::new();
        for (path, entry) in entries {
            let culprit = self
                .contributors
                .get(&path)
                .and_then(|accepted| accepted.last().copied());
            let invalid = |message: String| Invalid {
                path: path.clone(),
                message,
                culprit,
            };
            changes.push(match entry {
                Entry::Create(contents) => FileChange::Create {
                    path: path.clone(),
                    contents: contents.to_owned(),
                },
                Entry::Delete => {
                    if let Ok(bytes) = self.fs.read_bytes(&self.root.join(&path)) {
                        bases.insert(path.clone(), content_hash(&bytes));
                    }
                    FileChange::Delete { path: path.clone() }
                }
                Entry::Edit { ops, edits } => {
                    let source = self.source(&path).map_err(invalid)?;
                    // Per-fix plans decide which fixes are accepted; the file is written from one joint plan of
                    // every accepted op, because independent removals of neighbouring members need the joint
                    // view to place separators (two tail removals planned alone leave a dangling comma).
                    let edits = match FileKind::from_path(&path) {
                        Some(kind) => {
                            skif_edit::plan_edits(&source, kind, &ops).map_err(|error| invalid(error.to_string()))?
                        }
                        None => edits,
                    };
                    let new_contents = apply_text_edits(&source, &edits).map_err(|error| invalid(error.to_string()))?;
                    bases.insert(path.clone(), content_hash(source.as_bytes()));
                    FileChange::Modify {
                        path: path.clone(),
                        edits,
                        new_contents,
                    }
                }
            });
        }
        Ok((changes, bases))
    }
}

fn plan_failure(error: &EditPlanError) -> PlanFailure {
    match error {
        EditPlanError::Conflict { .. } => PlanFailure::Conflict(error.to_string()),
        _ => PlanFailure::Other(error.to_string()),
    }
}

/// Edits of a file that is neither JSON nor YAML: only byte-range text operations apply, and they must fit the file.
fn text_edits(source: &str, ops: &[&FixOp]) -> Result<Vec<TextEdit>, PlanFailure> {
    let edits: Vec<TextEdit> = ops
        .iter()
        .map(|op| match op {
            FixOp::Text { edit, .. } => Ok(edit.clone()),
            _ => Err(PlanFailure::Other("unsupported file type".to_owned())),
        })
        .collect::<Result<_, _>>()?;
    apply_text_edits(source, &edits).map_err(|error| PlanFailure::Other(error.to_string()))?;
    Ok(edits)
}

enum Pick<'a> {
    Fix(&'a Fix),
    Skip(SkipReason),
}

fn pick<'a>(selection: &FixSelection, index: usize, diagnostic: &'a Diagnostic) -> Pick<'a> {
    if let Some(&number) = selection.choices.get(&index) {
        return match diagnostic.fixes.get(number) {
            Some(chosen) if selection.allows(chosen.safety) => Pick::Fix(chosen),
            Some(chosen) => Pick::Skip(SkipReason::Safety(chosen.safety)),
            None => Pick::Skip(SkipReason::Failed(format!("diagnostic has no fix number {number}"))),
        };
    }
    let allowed: Vec<&Fix> = diagnostic
        .fixes
        .iter()
        .filter(|fix| selection.allows(fix.safety))
        .collect();
    match (allowed.as_slice(), diagnostic.fixes.first()) {
        ([only], _) => Pick::Fix(only),
        ([], Some(first)) => Pick::Skip(SkipReason::Safety(first.safety)),
        (alternatives, _) => {
            let chosen = match selection.alternatives {
                Alternatives::First => alternatives.first().copied(),
                Alternatives::Preferred(prefer) => alternatives
                    .iter()
                    .copied()
                    .find(|fix| fix.preference == Some(preference_of(prefer))),
                Alternatives::Ask => None,
            };
            chosen.map_or_else(
                || {
                    Pick::Skip(SkipReason::NeedsChoice {
                        alternatives: alternatives.iter().map(|fix| fix.title.clone()).collect(),
                    })
                },
                Pick::Fix,
            )
        }
    }
}

/// Plans the fixes `selection` picks from `report` and returns them as data; the file system is only read.
///
/// For each diagnostic in report order the pick is: the explicit choice, else the only fix the safety ceiling allows,
/// else (for several allowed alternatives) the first when [`Alternatives::First`], else a [`SkipReason::NeedsChoice`]
/// skip. A diagnostic whose fixes all exceed the ceiling is skipped with [`SkipReason::Safety`]. Each accepted fix
/// is planned on its own against the file as it is on disk (a fix whose edits do not fit is skipped, the others are
/// unaffected); a fix whose edits overlap an earlier accepted fix is skipped as a conflict (the earlier wins) and
/// operations or edits that two fixes share count once. The edits of one file become a single
/// [`FileChange::Modify`], so bytes outside every edit are unchanged, and a `CreateFile` over an existing file is
/// skipped. Paths in the plan are relative to the workspace root; resolve them against `workspace.root` to write
/// them.
///
/// # Errors
/// [`FixError::Plan`] when the accepted fixes of a file cannot be turned into contents (not expected: each fix is
/// validated against the file before it is accepted).
///
/// # Examples
/// ```
/// use skif::{FixSelection, FixSafety, FixPlan};
///
/// let selection = FixSelection::new(FixSafety::Safe);
/// assert!(FixPlan::default().is_empty());
/// assert!(selection.rules.is_none());
/// ```
pub fn apply_fixes(
    workspace: &Workspace,
    report: &Report,
    selection: &FixSelection,
    fs: &dyn FileSystem,
) -> Result<FixPlan, FixError> {
    let mut dropped: BTreeMap<usize, SkipReason> = BTreeMap::new();
    loop {
        match plan_once(workspace, report, selection, fs, &dropped) {
            Ok(plan) => return Ok(plan),
            Err(Invalid {
                path,
                culprit: Some(index),
                ..
            }) => {
                dropped.insert(
                    index,
                    SkipReason::Conflict(format!(
                        "{}: the combined edits of the accepted fixes do not form a valid file",
                        slash(&path)
                    )),
                );
            }
            Err(Invalid { path, message, .. }) => return Err(FixError::Plan { path, message }),
        }
    }
}

/// One planning pass; the diagnostics in `dropped` are skipped with the reason given instead of being planned.
fn plan_once(
    workspace: &Workspace,
    report: &Report,
    selection: &FixSelection,
    fs: &dyn FileSystem,
    dropped: &BTreeMap<usize, SkipReason>,
) -> Result<FixPlan, Invalid> {
    let mut planner = Planner {
        root: &workspace.root,
        fs,
        sources: BTreeMap::new(),
        entries: BTreeMap::new(),
        contributors: BTreeMap::new(),
    };
    let mut applied = Vec::new();
    let mut skipped = Vec::new();
    for (index, diagnostic) in report.diagnostics.iter().enumerate() {
        if diagnostic.fixes.is_empty() || !selection.includes(&diagnostic.rule) {
            continue;
        }
        let outcome = match (dropped.get(&index), pick(selection, index, diagnostic)) {
            (Some(reason), _) => Err(reason.clone()),
            (None, Pick::Fix(fix)) => planner.try_add(index, fix).map(|()| fix.title.clone()),
            (None, Pick::Skip(reason)) => Err(reason),
        };
        match outcome {
            Ok(title) => applied.push(AppliedFix {
                diagnostic: index,
                rule: diagnostic.rule.clone(),
                message: diagnostic.message.clone(),
                title,
            }),
            Err(reason) => skipped.push(SkippedFix {
                diagnostic: index,
                rule: diagnostic.rule.clone(),
                message: diagnostic.message.clone(),
                reason,
            }),
        }
    }
    let (changes, bases) = planner.finish()?;
    Ok(FixPlan {
        changes,
        applied,
        skipped,
        bases,
    })
}
