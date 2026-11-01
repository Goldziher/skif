//! Turning the fixes attached to a report into a plan of file changes, without touching the file system.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use skif_core::{
    Diagnostic, FileChange, FileSystem, Fix, FixOp, FixSafety, Report, RuleId, TextEdit, apply_text_edits,
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

    fn allows(&self, safety: FixSafety) -> bool {
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
}

impl FixPlan {
    /// Whether the plan changes no file.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// The workspace-relative path each change touches.
    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.changes.iter().map(|change| match change {
            FileChange::Modify { path, .. } | FileChange::Create { path, .. } | FileChange::Delete { path } => {
                path.as_path()
            }
        })
    }
}

/// Planning the fixes failed as a whole (as opposed to a single fix being skipped).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FixError {
    /// The edits of a file that every accepted fix agreed on could not be combined.
    #[error("cannot plan edits for {}: {message}", .path.display())]
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
        let text = format!("{}: {}", path.display(), self.message());
        match self {
            Self::Conflict(_) => SkipReason::Conflict(text),
            Self::Other(_) => SkipReason::Failed(text),
        }
    }
}

/// What the accepted fixes do to one file.
enum Entry<'a> {
    Edit(Vec<&'a FixOp>),
    Create(&'a str),
    Delete,
}

struct Planner<'a> {
    root: &'a Path,
    fs: &'a dyn FileSystem,
    sources: BTreeMap<PathBuf, Arc<str>>,
    entries: BTreeMap<PathBuf, Entry<'a>>,
}

fn op_path(op: &FixOp) -> &Path {
    match op {
        FixOp::Json { path, .. }
        | FixOp::Yaml { path, .. }
        | FixOp::Text { path, .. }
        | FixOp::CreateFile { path, .. }
        | FixOp::DeleteFile { path } => path,
        _ => Path::new(""),
    }
}

impl<'a> Planner<'a> {
    fn source(&mut self, path: &Path) -> Result<Arc<str>, String> {
        if let Some(source) = self.sources.get(path) {
            return Ok(Arc::clone(source));
        }
        let source = self
            .fs
            .read_to_string(&self.root.join(path))
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        self.sources.insert(path.to_path_buf(), Arc::clone(&source));
        Ok(source)
    }

    /// Adds every op of `fix` or none: the plan is only extended once all of the fix's files check out.
    fn try_add(&mut self, fix: &'a Fix) -> Result<(), SkipReason> {
        let mut by_file: BTreeMap<&Path, Vec<&'a FixOp>> = BTreeMap::new();
        for op in &fix.ops {
            by_file.entry(op_path(op)).or_default().push(op);
        }
        let mut next: Vec<(PathBuf, Entry<'a>)> = Vec::new();
        for (path, ops) in by_file {
            let entry = self.combine(path, &ops)?;
            next.push((path.to_path_buf(), entry));
        }
        self.entries.extend(next);
        Ok(())
    }

    fn combine(&mut self, path: &Path, ops: &[&'a FixOp]) -> Result<Entry<'a>, SkipReason> {
        let conflict = |what: &str| SkipReason::Conflict(format!("{}: {what}", path.display()));
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
                None => Ok(Entry::Create(contents)),
                Some(_) => Err(conflict("another fix already touches a file this fix creates")),
            },
            (None, true, false) => match existing {
                None | Some(Entry::Delete) => Ok(Entry::Delete),
                Some(_) => Err(conflict("another fix already touches a file this fix deletes")),
            },
            (None, false, true) => {
                let mut combined: Vec<&'a FixOp> = match existing {
                    None => Vec::new(),
                    Some(Entry::Edit(earlier)) => earlier.clone(),
                    Some(_) => return Err(conflict("another fix creates or deletes a file this fix edits")),
                };
                combined.extend(ops.iter().copied());
                self.plan(path, &combined).map_err(|failure| failure.into_skip(path))?;
                Ok(Entry::Edit(combined))
            }
            _ => Err(conflict("a single fix mixes creating, deleting and editing one file")),
        }
    }

    fn plan(&mut self, path: &Path, ops: &[&FixOp]) -> Result<Vec<TextEdit>, PlanFailure> {
        let kind = FileKind::from_path(path).ok_or_else(|| PlanFailure::Other("unsupported file type".to_owned()))?;
        let source = self.source(path).map_err(PlanFailure::Other)?;
        plan_edits(&source, kind, ops).map_err(|error| match error {
            EditPlanError::Conflict { .. } => PlanFailure::Conflict(error.to_string()),
            _ => PlanFailure::Other(error.to_string()),
        })
    }

    fn finish(mut self) -> Result<Vec<FileChange>, FixError> {
        let entries = std::mem::take(&mut self.entries);
        let mut changes = Vec::with_capacity(entries.len());
        for (path, entry) in entries {
            let plan_error = |failure: PlanFailure| FixError::Plan {
                path: path.clone(),
                message: failure.message().to_owned(),
            };
            changes.push(match entry {
                Entry::Create(contents) => FileChange::Create {
                    path: path.clone(),
                    contents: contents.to_owned(),
                },
                Entry::Delete => FileChange::Delete { path: path.clone() },
                Entry::Edit(ops) => {
                    let edits = self.plan(&path, &ops).map_err(plan_error)?;
                    let source = self.source(&path).map_err(|message| plan_error(PlanFailure::Other(message)))?;
                    let new_contents = apply_text_edits(&source, &edits).map_err(|error| FixError::Plan {
                        path: path.clone(),
                        message: error.to_string(),
                    })?;
                    FileChange::Modify {
                        path: path.clone(),
                        edits,
                        new_contents,
                    }
                }
            });
        }
        Ok(changes)
    }
}

enum Pick<'a> {
    Fix(&'a Fix),
    Skip(SkipReason),
}

fn pick<'a>(selection: &FixSelection, index: usize, diagnostic: &'a Diagnostic) -> Pick<'a> {
    let chosen = match selection.choices.get(&index) {
        Some(&fix) => match diagnostic.fixes.get(fix) {
            Some(fix) => fix,
            None => return Pick::Skip(SkipReason::Failed(format!("diagnostic has no fix number {fix}"))),
        },
        None => match (diagnostic.fixes.as_slice(), selection.alternatives) {
            ([only], _) => only,
            ([first, ..], Alternatives::First) => first,
            (fixes, _) => {
                return Pick::Skip(SkipReason::NeedsChoice {
                    alternatives: fixes.iter().map(|fix| fix.title.clone()).collect(),
                });
            }
        },
    };
    if selection.allows(chosen.safety) {
        Pick::Fix(chosen)
    } else {
        Pick::Skip(SkipReason::Safety(chosen.safety))
    }
}

/// Plans the fixes `selection` picks from `report` and returns them as data; the file system is only read.
///
/// For each diagnostic in report order the pick is: the explicit choice, else the only fix, else (for several
/// alternatives) the first when [`Alternatives::First`], else a [`SkipReason::NeedsChoice`] skip. A fix over the safety
/// ceiling is skipped, a fix that overlaps an earlier accepted fix is skipped as a conflict (the earlier wins), and
/// all operations of one file are planned together into a single [`FileChange::Modify`], so bytes outside every
/// edit are unchanged. Paths in the plan are relative to the workspace root; resolve them against `workspace.root`
/// to write them.
///
/// # Errors
/// [`FixError::Plan`] when the accepted fixes of a file cannot be turned into contents (not expected: each fix is
/// validated against the earlier ones before it is accepted).
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
    let mut planner = Planner {
        root: &workspace.root,
        fs,
        sources: BTreeMap::new(),
        entries: BTreeMap::new(),
    };
    let mut applied = Vec::new();
    let mut skipped = Vec::new();
    for (index, diagnostic) in report.diagnostics.iter().enumerate() {
        if diagnostic.fixes.is_empty() || !selection.includes(&diagnostic.rule) {
            continue;
        }
        let outcome = match pick(selection, index, diagnostic) {
            Pick::Fix(fix) => planner.try_add(fix).map(|()| fix.title.clone()),
            Pick::Skip(reason) => Err(reason),
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
    Ok(FixPlan {
        changes: planner.finish()?,
        applied,
        skipped,
    })
}
