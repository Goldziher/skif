//! The seeded source edits of `skif-bench cache-diff`: which file of a corpus checkout to change and how, and the
//! guard that puts the file back.
//!
//! An edit is planned from a seed alone, so a run is repeatable. It is applied with a journal beside the checkout
//! ([`corpus::edit_journal_path`]) naming the file, and the file is restored with `git checkout -- <path>` and checked
//! byte for byte against the original, both when the pass over the edited tree ends and when it fails. A run that is
//! killed leaves the journal behind, and the next command that locks the checkout restores the file
//! ([`corpus::recover_edit`]).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use skif_bench::rng::Rng;

use crate::corpus;

/// Source extensions an edit may change. Declaration files are left out: the analysis reads them differently.
const SOURCE_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];
/// Extensions of the files a `declare global` block is valid in.
const TYPESCRIPT_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts"];
/// Files larger than this (bundles, generated tables) are not edited.
const MAX_FILE_BYTES: u64 = 1 << 20;
/// Files tried for one edit before the kind is reported as applying nowhere.
const MAX_ATTEMPTS: usize = 200;
/// The identifier the edits introduce, so a changed finding names its cause.
const MARKER: &str = "skifBenchCacheDiff";
/// Declarations whose `export` keyword [`EditKind::DropExport`] removes.
const EXPORTED_DECLARATIONS: &[&str] = &[
    "const ",
    "let ",
    "var ",
    "function ",
    "async function ",
    "class ",
    "abstract class ",
    "interface ",
    "type ",
    "enum ",
    "const enum ",
];

/// One way of changing a source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// A line comment at the end: the content changes, the module does not.
    AppendComment,
    /// The `export` keyword of the first exported declaration goes, so its users lose an import target.
    DropExport,
    /// An unused class property or enum member is added.
    AddUnusedMember,
    /// A side-effect import of a sibling file is added at the top.
    AddImport,
    /// A `declare global` block is appended, which changes the global scope of every file.
    DeclareGlobal,
}

impl EditKind {
    pub const ALL: [Self; 5] = [
        Self::AppendComment,
        Self::DropExport,
        Self::AddUnusedMember,
        Self::AddImport,
        Self::DeclareGlobal,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::AppendComment => "append-comment",
            Self::DropExport => "drop-export",
            Self::AddUnusedMember => "add-unused-member",
            Self::AddImport => "add-import",
            Self::DeclareGlobal => "declare-global",
        }
    }
}

/// A planned edit: the file (relative to the checkout, `/`-separated) and its whole new content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub kind: EditKind,
    pub path: String,
    pub content: String,
}

fn extension(path: &str) -> &str {
    path.rsplit_once('.').map_or("", |(_, ext)| ext)
}

fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

fn directory(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// Whether `path` is a source file an edit may change.
pub fn is_source(path: &str) -> bool {
    let name = file_name(path);
    SOURCE_EXTENSIONS.contains(&extension(path))
        && ![".d.ts", ".d.mts", ".d.cts"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
        && !path.split('/').any(|part| part == "node_modules")
}

/// The newline of the lines an edit adds: CRLF in a file with any CRLF, so the file keeps its newline style.
fn newline(content: &str) -> &'static str {
    if content.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Whether the file is an ES module: some line starts with `import ` or `export `.
fn is_module(content: &str) -> bool {
    content
        .lines()
        .any(|line| line.starts_with("import ") || line.starts_with("export "))
}

/// `content` with `addition` (lines without their newline) after its last line.
fn append(content: &str, addition: &[&str]) -> String {
    let nl = newline(content);
    let mut out = content.to_owned();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push_str(nl);
    }
    for line in addition {
        out.push_str(line);
        out.push_str(nl);
    }
    out
}

/// The byte offset of the start of each line, with the line (without its newline).
fn lines_at(content: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    content.split_inclusive('\n').map(move |raw| {
        let start = offset;
        offset += raw.len();
        (start, raw.trim_end_matches(['\n', '\r']))
    })
}

fn indent(line: &str) -> &str {
    &line[..line.len() - line.trim_start().len()]
}

fn drop_export(content: &str) -> Option<String> {
    let (start, _) = lines_at(content).find(|(_, line)| {
        line.strip_prefix("export ")
            .is_some_and(|rest| EXPORTED_DECLARATIONS.iter().any(|decl| rest.starts_with(decl)))
    })?;
    let mut out = content.to_owned();
    out.replace_range(start..start + "export ".len(), "");
    Some(out)
}

/// The first line that opens a class or enum body (`class X {` or `enum X {` ending the line), with the member to add
/// to it.
fn member_insertion(content: &str) -> Option<(usize, String)> {
    let opens = |line: &str, keyword: &str| {
        let trimmed = line.trim();
        trimmed.ends_with('{')
            && trimmed
                .split_whitespace()
                .collect::<Vec<_>>()
                .windows(2)
                .any(|pair| pair[0] == keyword && pair[1].chars().next().is_some_and(char::is_alphabetic))
    };
    let lines: Vec<(usize, &str)> = lines_at(content).collect();
    for keyword in ["class", "enum"] {
        for (index, (start, line)) in lines.iter().enumerate() {
            if !opens(line, keyword) || line.trim_start().starts_with("//") {
                continue;
            }
            let body_indent = lines
                .get(index + 1)
                .map(|(_, next)| indent(next))
                .filter(|next| next.len() > indent(line).len())
                .map_or_else(|| format!("{}  ", indent(line)), str::to_owned);
            let member = if keyword == "class" {
                format!("{body_indent}{MARKER}Unused = 0;")
            } else {
                format!("{body_indent}{MARKER}Unused = \"skif-bench-cache-diff\",")
            };
            return Some((start + line.len(), member));
        }
    }
    None
}

fn add_member(content: &str) -> Option<String> {
    let (end_of_line, member) = member_insertion(content)?;
    let mut out = content.to_owned();
    out.insert_str(end_of_line, &format!("{}{member}", newline(content)));
    Some(out)
}

/// The import specifier of the sibling `name`: TypeScript sources without their extension, others with it.
fn specifier(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, "ts" | "tsx")) => format!("./{stem}"),
        _ => format!("./{name}"),
    }
}

fn add_import(content: &str, sibling: &str) -> String {
    let nl = newline(content);
    let line = format!("import \"{}\";{nl}", specifier(sibling));
    let at = if content.starts_with("#!") {
        content.find('\n').map_or(content.len(), |end| end + 1)
    } else {
        0
    };
    let mut out = content.to_owned();
    if at == out.len() && !out.is_empty() && !out.ends_with('\n') {
        out.push_str(nl);
    }
    let at = at.min(out.len());
    out.insert_str(at, &line);
    out
}

/// Applies `kind` to the file `path` with `content`, or `None` when the kind does not apply to it. `siblings` are the
/// names of the other source files in the same directory, sorted.
pub fn apply(kind: EditKind, path: &str, content: &str, siblings: &[&str]) -> Option<String> {
    match kind {
        EditKind::AppendComment => Some(append(content, &["// skif-bench cache-diff edit"])),
        EditKind::DropExport => drop_export(content),
        EditKind::AddUnusedMember => add_member(content),
        EditKind::AddImport => {
            let sibling = siblings.first().filter(|_| is_module(content))?;
            Some(add_import(content, sibling))
        }
        EditKind::DeclareGlobal => {
            (TYPESCRIPT_EXTENSIONS.contains(&extension(path)) && is_module(content)).then(|| {
                append(
                    content,
                    &[
                        "declare global {",
                        &format!("  interface {MARKER}Global {{"),
                        "    edited: true;",
                        "  }",
                        "}",
                    ],
                )
            })
        }
    }
}

/// The names of the other candidates in the directory of `path`, sorted.
fn siblings<'a>(candidates: &'a [String], path: &str) -> Vec<&'a str> {
    let dir = directory(path);
    let mut names: Vec<&str> = candidates
        .iter()
        .filter(|other| other.as_str() != path && directory(other) == dir)
        .map(|other| file_name(other))
        .collect();
    names.sort_unstable();
    names
}

/// Plans `count` edits over `candidates` (sorted relative paths), reading files with `read`. The kinds rotate through
/// [`EditKind::ALL`] from a seeded start, so `count >= 5` uses each; the file of each is the first that accepts the
/// kind, scanning the candidates from a place drawn from `rng`. An entry is `Err` when none of the [`MAX_ATTEMPTS`]
/// files tried accepts its kind.
pub fn plan(
    rng: &mut Rng,
    candidates: &[String],
    count: usize,
    read: impl Fn(&str) -> Option<String>,
) -> Vec<Result<Edit, String>> {
    let start = rng.below_usize(EditKind::ALL.len());
    (0..count)
        .map(|index| {
            let kind = EditKind::ALL[(start + index) % EditKind::ALL.len()];
            let first = rng.below_usize(candidates.len());
            for attempt in 0..MAX_ATTEMPTS.min(candidates.len()) {
                let path = &candidates[(first + attempt) % candidates.len()];
                let Some(content) = read(path) else { continue };
                if let Some(new) = apply(kind, path, &content, &siblings(candidates, path)) {
                    return Ok(Edit {
                        kind,
                        path: path.clone(),
                        content: new,
                    });
                }
            }
            Err(format!(
                "no source file tried accepts the {} edit ({} candidate(s))",
                kind.name(),
                candidates.len()
            ))
        })
        .collect()
}

/// The tracked source files of the checkout `root` that exist on disk (a sparse checkout lists others) and are small
/// enough to edit, sorted. A tracked symlink is left out: an edit would go through it to its target, which
/// `git checkout` of the link does not restore.
///
/// # Errors
/// Fails when git cannot list the files.
pub fn candidates(root: &Path) -> Result<Vec<String>> {
    let listed = corpus::git(root, &["ls-files"])?;
    let mut found: Vec<String> = listed
        .lines()
        .filter(|path| is_source(path))
        .filter(|path| {
            fs::symlink_metadata(root.join(path)).is_ok_and(|meta| meta.is_file() && meta.len() <= MAX_FILE_BYTES)
        })
        .map(str::to_owned)
        .collect();
    found.sort_unstable();
    Ok(found)
}

/// An edit applied to a checkout. [`EditGuard::restore`] puts the file back and checks it; dropping the guard without
/// that (an error or a panic in between) restores it too and reports a failure loudly.
pub struct EditGuard {
    root: PathBuf,
    path: String,
    original: Vec<u8>,
    journal: PathBuf,
    restored: bool,
}

impl EditGuard {
    /// Writes the journal, then the edited content.
    ///
    /// # Errors
    /// Fails, before changing anything, when the file has uncommitted changes (`git checkout` would not restore it) or
    /// a journal is already there; and when the journal or the file cannot be written.
    pub fn apply(root: &Path, edit: &Edit) -> Result<Self> {
        let status = corpus::git(root, &["status", "--porcelain", "--", &edit.path])?;
        if !status.is_empty() {
            bail!(
                "{} has uncommitted changes in {} ({status}); cache-diff restores edits with git checkout and edits \
                 only clean files",
                edit.path,
                root.display()
            );
        }
        let journal = corpus::edit_journal_path(root);
        if journal.exists() {
            bail!(
                "{} exists: an earlier edit was not restored; lock the checkout again (any skif-bench command) to \
                 restore it",
                journal.display()
            );
        }
        let file = root.join(&edit.path);
        let original = fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
        fs::write(&journal, &edit.path).with_context(|| format!("writing {}", journal.display()))?;
        let mut guard = Self {
            root: root.to_owned(),
            path: edit.path.clone(),
            original,
            journal,
            restored: false,
        };
        if let Err(error) = fs::write(&file, &edit.content) {
            guard.restore()?;
            return Err(error).with_context(|| format!("writing {}", file.display()));
        }
        Ok(guard)
    }

    /// Restores the file with `git checkout -- <path>`, checks it is byte-identical to the original and removes the
    /// journal.
    ///
    /// # Errors
    /// Fails when git fails or the restored file differs; the original bytes are then written back, and the journal
    /// stays when even that fails.
    pub fn restore(&mut self) -> Result<()> {
        if self.restored {
            return Ok(());
        }
        let file = self.root.join(&self.path);
        let checkout = corpus::git(&self.root, &["checkout", "--", &self.path]);
        let identical = fs::read(&file).is_ok_and(|bytes| bytes == self.original);
        if checkout.is_err() || !identical {
            fs::write(&file, &self.original).with_context(|| {
                format!(
                    "writing back the original {} after git checkout failed to restore it; {} names it",
                    file.display(),
                    self.journal.display()
                )
            })?;
            self.restored = true;
            fs::remove_file(&self.journal).with_context(|| format!("removing {}", self.journal.display()))?;
            checkout?;
            bail!(
                "git checkout did not restore {} byte for byte; the original bytes were written back",
                file.display()
            );
        }
        self.restored = true;
        fs::remove_file(&self.journal).with_context(|| format!("removing {}", self.journal.display()))
    }
}

impl Drop for EditGuard {
    fn drop(&mut self) {
        if !self.restored
            && let Err(error) = self.restore()
        {
            eprintln!(
                "cache-diff: FAILED to restore {} in {}: {error:#}",
                self.path,
                self.root.display()
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::BTreeMap;
    use std::process::Command;

    use super::*;

    const MODULE: &str = "import { b } from \"./b\";\n\nexport const a = b;\n\nexport class Box {\n  value = 1;\n}\n";

    fn tree() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("src/a.ts".to_owned(), MODULE.to_owned()),
            ("src/b.ts".to_owned(), "export const b = 1;\n".to_owned()),
            ("src/c.js".to_owned(), "module.exports = 1;\n".to_owned()),
            ("lib/d.mjs".to_owned(), "export function d() {}\n".to_owned()),
            ("lib/e.ts".to_owned(), "export enum E {\n    One,\n}\n".to_owned()),
        ])
    }

    fn plan_over(files: &BTreeMap<String, String>, seed: u64, count: usize) -> Vec<Result<Edit, String>> {
        let candidates: Vec<String> = files.keys().cloned().collect();
        plan(&mut Rng::new(seed), &candidates, count, |path| files.get(path).cloned())
    }

    #[test]
    fn the_plan_is_deterministic_for_a_seed_and_uses_every_kind() {
        let files = tree();
        let first = plan_over(&files, 7, 10);
        assert_eq!(first, plan_over(&files, 7, 10));
        let kinds: Vec<EditKind> = first.iter().map(|edit| edit.as_ref().unwrap().kind).collect();
        for kind in EditKind::ALL {
            assert!(kinds.contains(&kind), "{kind:?} missing from {kinds:?}");
        }
        let other_seeds: Vec<_> = (0..8).map(|seed| plan_over(&files, seed, 10)).collect();
        assert!(
            other_seeds.iter().any(|plan| plan != &first),
            "the seed changes nothing"
        );
    }

    #[test]
    fn a_kind_that_applies_nowhere_is_an_error_entry() {
        let files = BTreeMap::from([("a.js".to_owned(), "module.exports = 1;\n".to_owned())]);
        let planned = plan_over(&files, 1, 5);
        let failed: Vec<&String> = planned.iter().filter_map(|edit| edit.as_ref().err()).collect();
        assert_eq!(failed.len(), 4, "{planned:?}");
        assert!(failed.iter().any(|reason| reason.contains("declare-global")));
    }

    #[test]
    fn each_kind_changes_the_file_as_described() {
        let edit = |kind| apply(kind, "src/a.ts", MODULE, &["b.ts"]).unwrap();
        assert!(
            edit(EditKind::AppendComment)
                .ends_with("export class Box {\n  value = 1;\n}\n// skif-bench cache-diff edit\n")
        );
        assert!(edit(EditKind::DropExport).contains("\nconst a = b;\n"));
        assert!(edit(EditKind::DropExport).contains("export class Box"));
        assert!(
            edit(EditKind::AddUnusedMember)
                .contains("export class Box {\n  skifBenchCacheDiffUnused = 0;\n  value = 1;")
        );
        assert!(edit(EditKind::AddImport).starts_with("import \"./b\";\nimport { b }"));
        assert!(edit(EditKind::DeclareGlobal).contains("declare global {\n  interface skifBenchCacheDiffGlobal {"));
        let enum_member = apply(EditKind::AddUnusedMember, "e.ts", "export enum E {\n    One,\n}\n", &[]).unwrap();
        assert_eq!(
            enum_member,
            "export enum E {\n    skifBenchCacheDiffUnused = \"skif-bench-cache-diff\",\n    One,\n}\n"
        );
    }

    #[test]
    fn kinds_refuse_files_they_do_not_fit() {
        let script = "module.exports = 1;\n";
        assert_eq!(apply(EditKind::DropExport, "a.js", script, &[]), None);
        assert_eq!(apply(EditKind::AddUnusedMember, "a.js", script, &[]), None);
        assert_eq!(
            apply(EditKind::AddImport, "a.js", script, &["b.js"]),
            None,
            "not a module"
        );
        assert_eq!(apply(EditKind::AddImport, "a.ts", MODULE, &[]), None, "no sibling");
        assert_eq!(
            apply(EditKind::DeclareGlobal, "a.mjs", MODULE, &[]),
            None,
            "not TypeScript"
        );
        assert_eq!(apply(EditKind::DeclareGlobal, "a.ts", script, &[]), None, "a script");
        assert_eq!(
            apply(EditKind::DropExport, "a.ts", "export default 1;\nexport { a };\n", &[]),
            None
        );
    }

    #[test]
    fn edits_keep_crlf_and_a_shebang() {
        let crlf = MODULE.replace('\n', "\r\n");
        for kind in EditKind::ALL {
            let edited = apply(kind, "a.ts", &crlf, &["b.ts"]).unwrap();
            assert!(!edited.replace("\r\n", "").contains('\n'), "{kind:?} added a bare LF");
        }
        let shebang = "#!/usr/bin/env node\nexport const a = 1;\n";
        assert_eq!(
            apply(EditKind::AddImport, "a.mjs", shebang, &["b.mjs"]).unwrap(),
            "#!/usr/bin/env node\nimport \"./b.mjs\";\nexport const a = 1;\n"
        );
    }

    #[test]
    fn sources_exclude_declarations_and_dependencies() {
        assert!(is_source("src/a.ts") && is_source("a.mjs") && is_source("x/y.tsx"));
        assert!(!is_source("src/a.d.ts") && !is_source("node_modules/x/a.js") && !is_source("a.json"));
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "core.autocrlf=false",
                "-C",
            ])
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
    }

    /// A committed checkout of [`tree`] under `cache/<name>`, so the journal beside it stays inside the test dir.
    fn checkout(name: &str) -> (PathBuf, PathBuf) {
        let cache = std::env::temp_dir().join(format!("skif-bench-cache-edit-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&cache);
        let root = cache.join("repo");
        for (path, content) in tree() {
            let file = root.join(&path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, content).unwrap();
        }
        git(&root, &["init", "--quiet"]);
        // The restore runs `git checkout` without the flags of `git`, so the repository itself must keep the bytes
        // as committed: Git for Windows converts line endings on checkout by default.
        git(&root, &["config", "core.autocrlf", "false"]);
        git(&root, &["add", "."]);
        git(&root, &["commit", "--quiet", "-m", "t"]);
        (cache, root)
    }

    #[test]
    fn every_kind_applies_and_restores_byte_identically() {
        let (cache, root) = checkout("restore");
        let files = tree();
        let candidates = candidates(&root).unwrap();
        assert_eq!(candidates, files.keys().cloned().collect::<Vec<_>>());
        for kind in EditKind::ALL {
            let path = "src/a.ts";
            let edit = Edit {
                kind,
                path: path.to_owned(),
                content: apply(kind, path, &files[path], &["b.ts"]).unwrap(),
            };
            let mut guard = EditGuard::apply(&root, &edit).unwrap();
            assert_eq!(fs::read_to_string(root.join(path)).unwrap(), edit.content);
            assert_eq!(fs::read_to_string(corpus::edit_journal_path(&root)).unwrap(), path);
            guard.restore().unwrap();
            assert_eq!(fs::read(root.join(path)).unwrap(), files[path].as_bytes(), "{kind:?}");
            assert!(!corpus::edit_journal_path(&root).exists());
        }
        fs::remove_dir_all(&cache).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_tracked_symlink_is_not_a_candidate() {
        let (cache, root) = checkout("symlink");
        std::os::unix::fs::symlink("a.ts", root.join("src/link.ts")).unwrap();
        git(&root, &["add", "src/link.ts"]);
        git(&root, &["commit", "--quiet", "-m", "link"]);
        let candidates = candidates(&root).unwrap();
        assert!(candidates.iter().any(|path| path == "src/a.ts"), "{candidates:?}");
        assert!(!candidates.iter().any(|path| path == "src/link.ts"), "{candidates:?}");
        fs::remove_dir_all(&cache).unwrap();
    }

    #[test]
    fn a_dropped_guard_restores_and_a_journal_left_behind_is_recovered() {
        let (cache, root) = checkout("drop");
        let original = fs::read(root.join("src/b.ts")).unwrap();
        let edit = Edit {
            kind: EditKind::AppendComment,
            path: "src/b.ts".to_owned(),
            content: "changed\n".to_owned(),
        };
        drop(EditGuard::apply(&root, &edit).unwrap());
        assert_eq!(fs::read(root.join("src/b.ts")).unwrap(), original, "drop restores");
        let killed = EditGuard::apply(&root, &edit).unwrap();
        std::mem::forget(killed);
        assert_ne!(fs::read(root.join("src/b.ts")).unwrap(), original);
        assert!(
            EditGuard::apply(&root, &edit).is_err(),
            "a pending journal blocks the next edit"
        );
        corpus::recover_edit(&root).unwrap();
        assert_eq!(
            fs::read(root.join("src/b.ts")).unwrap(),
            original,
            "the journal restores"
        );
        assert!(!corpus::edit_journal_path(&root).exists());
        fs::remove_dir_all(&cache).unwrap();
    }

    #[test]
    fn a_file_with_uncommitted_changes_is_not_edited() {
        let (cache, root) = checkout("dirty");
        fs::write(root.join("src/b.ts"), "local\n").unwrap();
        let edit = Edit {
            kind: EditKind::AppendComment,
            path: "src/b.ts".to_owned(),
            content: "changed\n".to_owned(),
        };
        let error = EditGuard::apply(&root, &edit).err().unwrap();
        assert!(format!("{error:#}").contains("uncommitted changes"), "{error:#}");
        assert_eq!(fs::read_to_string(root.join("src/b.ts")).unwrap(), "local\n");
        assert!(!corpus::edit_journal_path(&root).exists());
        fs::remove_dir_all(&cache).unwrap();
    }
}
