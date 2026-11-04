//! Findings parity with knip: runs skif and a pinned knip on each corpus repository and diffs the findings.
//!
//! Both tools' findings are reduced to a key per finding, `<file>|<name>` within a knip issue type. The two sides
//! disagree where the tools model things differently, so the numbers are a measure of drift, not a verdict: the
//! committed baselines say what the agreement was, and `--check` fails when it moves.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::corpus::{self, Repo, Tools};
use crate::diff::{self, BASELINE_SCHEMA, Baseline, COMPARED, Counts, Entry, Gate, Keys, Samples};
use crate::proc::{capture, find_on_path};

/// skif rule to knip issue type.
const RULE_CATEGORIES: [(&str, &str); 12] = [
    ("unused-dependencies", "dependencies"),
    ("unused-dev-dependencies", "devDependencies"),
    ("unused-optional-peer-dependencies", "optionalPeerDependencies"),
    ("unlisted-dependencies", "unlisted"),
    ("unlisted-binaries", "binaries"),
    ("unresolved-imports", "unresolved"),
    ("unused-files", "files"),
    ("unused-exports", "exports"),
    ("unused-types", "types"),
    ("unused-enum-members", "enumMembers"),
    ("unused-namespace-members", "namespaceMembers"),
    ("duplicate-exports", "duplicates"),
];

/// knip issue type (as named in its JSON) to the common category. `nsExports` and `nsTypes` are the older names of
/// `namespaceMembers`; `classMembers` has no skif rule and is left out of the comparison.
const KNIP_CATEGORIES: [(&str, &str); 14] = [
    ("dependencies", "dependencies"),
    ("devDependencies", "devDependencies"),
    ("optionalPeerDependencies", "optionalPeerDependencies"),
    ("unlisted", "unlisted"),
    ("binaries", "binaries"),
    ("unresolved", "unresolved"),
    ("files", "files"),
    ("exports", "exports"),
    ("types", "types"),
    ("enumMembers", "enumMembers"),
    ("namespaceMembers", "namespaceMembers"),
    ("nsExports", "namespaceMembers"),
    ("nsTypes", "namespaceMembers"),
    ("duplicates", "duplicates"),
];

/// Marker written into a checkout once its dependencies are installed, holding the pinned commit.
const INSTALLED_MARKER: &str = ".skif-bench-installed";

pub struct Options {
    pub skif: PathBuf,
    pub corpus_file: PathBuf,
    pub corpus_dir: PathBuf,
    pub out: PathBuf,
    pub only: Vec<String>,
    pub install: bool,
    pub recursive: bool,
    pub timeout: Duration,
    pub gate: Gate,
}

/// Makes a path relative to the checkout with `/` separators.
pub fn normalize(raw: &str, root: &Path) -> String {
    let unified = raw.replace('\\', "/");
    let prefix = format!("{}/", root.to_string_lossy().replace('\\', "/").trim_end_matches('/'));
    let relative = unified.strip_prefix(&prefix).unwrap_or(&unified);
    relative.trim_start_matches("./").to_owned()
}

/// The backtick-quoted names in a diagnostic message.
fn backticked(message: &str) -> Vec<&str> {
    message.split('`').skip(1).step_by(2).collect()
}

fn joined_sorted(mut names: Vec<String>) -> String {
    names.sort();
    names.join("|")
}

/// The findings of a skif JSON document, by knip issue type.
pub fn skif_keys(document: &Value, root: &Path) -> Keys {
    let mut keys = Keys::new();
    let Some(diagnostics) = document.get("diagnostics").and_then(Value::as_array) else {
        return keys;
    };
    for diagnostic in diagnostics {
        let Some(rule) = diagnostic.get("rule").and_then(Value::as_str) else {
            continue;
        };
        let Some((_, category)) = RULE_CATEGORIES.iter().find(|(id, _)| *id == rule) else {
            continue;
        };
        let path = normalize(
            diagnostic
                .pointer("/location/path")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            root,
        );
        let message = diagnostic.get("message").and_then(Value::as_str).unwrap_or_default();
        let names = backticked(message);
        let key = match *category {
            "files" => path,
            "duplicates" => format!(
                "{path}|{}",
                joined_sorted(names.iter().skip(1).map(|n| (*n).to_owned()).collect())
            ),
            _ => {
                let name = diagnostic
                    .get("dependency")
                    .and_then(Value::as_str)
                    .or_else(|| names.first().copied())
                    .unwrap_or_default();
                format!("{path}|{name}")
            }
        };
        keys.entry((*category).to_owned()).or_default().insert(key);
    }
    keys
}

fn name_of(value: &Value) -> Option<String> {
    value
        .get("name")
        .or_else(|| value.get("symbol"))
        .and_then(Value::as_str)
        .or_else(|| value.as_str())
        .map(str::to_owned)
}

/// The names under one knip issue type: a list of items, a list of lists (duplicates) or a map of lists (enums).
fn knip_names(value: &Value) -> Vec<String> {
    match value {
        Value::Array(items) => items
            .iter()
            .flat_map(|item| match item {
                Value::Array(group) => vec![joined_sorted(group.iter().filter_map(name_of).collect())],
                other => name_of(other).into_iter().collect(),
            })
            .collect(),
        Value::Object(map) => map.values().flat_map(knip_names).collect(),
        _ => Vec::new(),
    }
}

/// The findings of a knip JSON report, by issue type.
pub fn knip_keys(document: &Value, root: &Path) -> Keys {
    let mut keys = Keys::new();
    if let Some(files) = document.get("files").and_then(Value::as_array) {
        keys.entry("files".to_owned())
            .or_default()
            .extend(files.iter().filter_map(Value::as_str).map(|f| normalize(f, root)));
    }
    let Some(issues) = document.get("issues").and_then(Value::as_array) else {
        return keys;
    };
    for issue in issues {
        let file = normalize(issue.get("file").and_then(Value::as_str).unwrap_or_default(), root);
        for (type_name, category) in KNIP_CATEGORIES {
            let Some(value) = issue.get(type_name) else {
                continue;
            };
            let names = knip_names(value);
            if names.is_empty() {
                continue;
            }
            let set = keys.entry(category.to_owned()).or_default();
            if category == "files" {
                set.insert(file.clone());
            } else {
                set.extend(names.into_iter().map(|name| format!("{file}|{name}")));
            }
        }
    }
    keys
}

fn total(keys: &Keys) -> usize {
    keys.values().map(std::collections::BTreeSet::len).sum()
}

/// The install command of a package manager, or why there is none.
fn install_command(repo: &Repo, dir: &Path) -> Result<(String, Vec<&'static str>), String> {
    if !repo.install {
        return Err(format!("install is disabled for this repository: {}", repo.notes));
    }
    match repo.package_manager.as_str() {
        "pnpm" => Ok((
            "pnpm".to_owned(),
            vec!["install", "--ignore-scripts", "--frozen-lockfile"],
        )),
        "npm" => Ok(("npm".to_owned(), vec!["ci", "--ignore-scripts"])),
        "bun" => Ok((
            "bun".to_owned(),
            vec!["install", "--ignore-scripts", "--frozen-lockfile"],
        )),
        "yarn" if dir.join(".yarnrc.yml").is_file() => {
            Ok(("yarn".to_owned(), vec!["install", "--immutable", "--mode=skip-build"]))
        }
        "yarn" => Ok((
            "yarn".to_owned(),
            vec!["install", "--frozen-lockfile", "--ignore-scripts"],
        )),
        other => Err(format!("no install step for package manager {other}")),
    }
}

/// Installs the dependencies of a checkout without running lifecycle scripts; `Some(reason)` when that fails.
fn install(repo: &Repo, dir: &Path, scratch: &Path, timeout: Duration) -> Result<Option<String>> {
    let marker = dir.join(INSTALLED_MARKER);
    if fs::read_to_string(&marker).is_ok_and(|rev| rev.trim() == repo.rev) {
        return Ok(None);
    }
    let (program, args) = match install_command(repo, dir) {
        Ok(command) => command,
        Err(reason) => return Ok(Some(reason)),
    };
    if find_on_path(&program).is_none() {
        return Ok(Some(format!("{program} is not installed")));
    }
    eprintln!("parity: {} {program} {}", repo.name, args.join(" "));
    let run = capture(
        Command::new(&program)
            .args(&args)
            .current_dir(dir)
            .env("CI", "1")
            .env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"),
        scratch,
        "install",
        timeout,
    )?;
    if run.timed_out {
        return Ok(Some(format!("{program} install timed out")));
    }
    if run.code != Some(0) {
        return Ok(Some(format!("{program} install failed: {}", run.stderr_summary())));
    }
    fs::write(&marker, &repo.rev).with_context(|| format!("writing {}", marker.display()))?;
    Ok(None)
}

struct Outcome {
    entry: Entry,
    detail: Value,
}

fn skipped(reason: impl Into<String>) -> Outcome {
    let reason = reason.into();
    Outcome {
        detail: json!({ "status": diff::SKIPPED, "reason": reason }),
        entry: Entry::skipped(reason),
    }
}

fn parity_repo(options: &Options, skif: &Path, tools: &Tools, repo: &Repo, dir: &Path) -> Result<Outcome> {
    let root = fs::canonicalize(dir).with_context(|| format!("resolving {}", dir.display()))?;
    let scratch = options.out.join("scratch").join(&repo.name);
    if options.install
        && let Some(reason) = install(repo, &root, &scratch, options.timeout)?
    {
        return Ok(skipped(format!("install: {reason}")));
    }

    let mut skif_command = Command::new(skif);
    skif_command
        .args([".", "--format", "json", "--no-install"])
        .current_dir(&root);
    if options.recursive {
        skif_command.arg("--recursive");
    }
    let skif_run = capture(&mut skif_command, &scratch, "skif", options.timeout)?;
    if skif_run.timed_out {
        return Ok(skipped("skif timed out"));
    }
    let skif_doc: Value = match serde_json::from_slice(&skif_run.stdout) {
        Ok(doc) => doc,
        Err(error) => return Ok(skipped(format!("skif output is not JSON: {error}"))),
    };
    if skif_doc.get("diagnostics").is_none() {
        let message = skif_doc["error"]["message"]
            .as_str()
            .unwrap_or("no diagnostics in the output");
        return Ok(skipped(format!("skif failed: {message}")));
    }

    let spec = format!("knip@{}", tools.knip);
    let knip_run = capture(
        Command::new("npx")
            .args(["--yes", spec.as_str(), "--reporter", "json"])
            .current_dir(&root)
            .env("CI", "1"),
        &scratch,
        "knip",
        options.timeout,
    )?;
    if knip_run.timed_out {
        return Ok(skipped("knip timed out"));
    }
    let knip_doc: Value = match serde_json::from_slice(&knip_run.stdout) {
        Ok(doc) => doc,
        Err(_) => return Ok(skipped(format!("knip failed: {}", knip_run.stderr_summary()))),
    };

    let skif_found = skif_keys(&skif_doc, &root);
    let knip_found = knip_keys(&knip_doc, &root);
    let (counts, samples) = diff::compare(&skif_found, &knip_found);
    let detail = json!({
        "status": COMPARED,
        "skif_ms": skif_run.elapsed.as_millis(),
        "knip_ms": knip_run.elapsed.as_millis(),
        "skif_total": total(&skif_found),
        "knip_total": total(&knip_found),
        "categories": category_detail(&counts, &samples),
    });
    Ok(Outcome {
        entry: Entry::compared(counts),
        detail,
    })
}

fn category_detail(counts: &BTreeMap<String, Counts>, samples: &BTreeMap<String, Samples>) -> Value {
    Value::Object(
        counts
            .iter()
            .map(|(category, found)| {
                (
                    category.clone(),
                    json!({
                        "skif_only": found.skif_only,
                        "knip_only": found.tool_only,
                        "both": found.both,
                        "samples": samples.get(category).map(|s| json!({ "skif_only": s.skif_only, "knip_only": s.tool_only })),
                    }),
                )
            })
            .collect(),
    )
}

fn markdown(tool: &str, installed: bool, entries: &BTreeMap<String, Entry>) -> String {
    let mut out = format!(
        "# skif findings parity with {tool}\n\nDependencies installed: {}.\n\n| Repository | Status | skif only | {tool} only | Both |\n| --- | --- | ---: | ---: | ---: |\n",
        if installed { "yes" } else { "no" }
    );
    for (name, entry) in entries {
        if entry.status == COMPARED {
            let sum = |f: fn(&Counts) -> usize| entry.categories.values().map(f).sum::<usize>();
            out.push_str(&format!(
                "| {name} | compared | {} | {} | {} |\n",
                sum(|c| c.skif_only),
                sum(|c| c.tool_only),
                sum(|c| c.both)
            ));
        } else {
            out.push_str(&format!(
                "| {name} | skipped: {} | - | - | - |\n",
                entry.reason.as_deref().unwrap_or("")
            ));
        }
    }
    out
}

/// Runs the comparison on the parity suite and applies the baseline gate.
///
/// # Errors
/// Fails when npx or skif is missing, the corpus cannot be fetched, a command cannot start, the report cannot be
/// written, or the gate finds drift.
pub fn run(options: &Options) -> Result<()> {
    if find_on_path("npx").is_none() {
        bail!("npx is required to run the pinned knip: install Node.js");
    }
    let skif = fs::canonicalize(&options.skif).with_context(|| {
        format!(
            "skif binary not found at {} (build it or pass --skif)",
            options.skif.display()
        )
    })?;
    let (tools, repos) = corpus::fetch_suite(&options.corpus_file, "parity", &options.only, &options.corpus_dir)?;
    let tool = format!("knip@{}", tools.knip);
    let mut entries = BTreeMap::new();
    let mut details = serde_json::Map::new();
    for (repo, dir) in &repos {
        eprintln!("parity: {}", repo.name);
        let outcome = parity_repo(options, &skif, &tools, repo, dir)?;
        if let Some(reason) = &outcome.entry.reason {
            eprintln!("parity: {} skipped: {reason}", repo.name);
        }
        details.insert(repo.name.clone(), outcome.detail);
        entries.insert(repo.name.clone(), outcome.entry);
    }
    let table = markdown(&tool, options.install, &entries);
    fs::create_dir_all(&options.out).with_context(|| format!("creating {}", options.out.display()))?;
    let full = json!({
        "tool": tool,
        "installed": options.install,
        "recursive": options.recursive,
        "repos": details,
    });
    fs::write(
        options.out.join("parity.json"),
        serde_json::to_string_pretty(&full)? + "\n",
    )?;
    fs::write(options.out.join("parity.md"), &table)?;
    println!("{table}");
    let current = Baseline {
        schema: BASELINE_SCHEMA,
        tool,
        installed: options.install,
        entries,
    };
    diff::conclude(&options.gate, &current)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn set(keys: &Keys, category: &str) -> Vec<String> {
        keys[category].iter().cloned().collect()
    }

    #[test]
    fn normalizes_absolute_and_relative_paths() {
        let root = Path::new("/work/repo");
        assert_eq!(normalize("/work/repo/a/b.ts", root), "a/b.ts");
        assert_eq!(normalize("./a/b.ts", root), "a/b.ts");
        assert_eq!(normalize("a\\b.ts", root), "a/b.ts");
        assert_eq!(normalize("/work/repository/a.ts", root), "/work/repository/a.ts");
    }

    #[test]
    fn maps_skif_diagnostics_to_knip_categories() {
        let doc = json!({ "diagnostics": [
            { "rule": "unused-dependencies", "message": "`left-pad` is never used", "location": { "path": "packages/a/package.json" } },
            { "rule": "unlisted-dependencies", "message": "`react` is imported", "dependency": "react", "location": { "path": "/r/src/a.ts" } },
            { "rule": "unused-files", "message": "`src/dead.ts` is not reachable", "location": { "path": "src/dead.ts" } },
            { "rule": "duplicate-exports", "message": "`local` is exported as `b` and `a`", "location": { "path": "src/d.ts" } },
            { "rule": "version-mismatch", "message": "x", "location": { "path": "package.json" } },
        ]});
        let keys = skif_keys(&doc, Path::new("/r"));
        assert_eq!(set(&keys, "dependencies"), ["packages/a/package.json|left-pad"]);
        assert_eq!(set(&keys, "unlisted"), ["src/a.ts|react"]);
        assert_eq!(set(&keys, "files"), ["src/dead.ts"]);
        assert_eq!(set(&keys, "duplicates"), ["src/d.ts|a|b"]);
        assert_eq!(keys.len(), 4);
    }

    #[test]
    fn maps_knip_issues_of_every_shape() {
        let doc = json!({ "files": ["old.ts"], "issues": [
            { "file": "package.json", "dependencies": [{ "name": "zzz" }], "devDependencies": [] },
            { "file": "src/dead.ts", "files": [{ "name": "src/dead.ts" }] },
            { "file": "src/e.ts", "enumMembers": { "Color": [{ "name": "Red" }] }, "exports": [{ "name": "x", "line": 1 }],
              "duplicates": [[{ "name": "b" }, { "name": "a" }]], "nsTypes": [{ "name": "T" }], "classMembers": [{ "name": "m" }] },
        ]});
        let keys = knip_keys(&doc, Path::new("/r"));
        assert_eq!(set(&keys, "dependencies"), ["package.json|zzz"]);
        assert_eq!(set(&keys, "files"), ["old.ts", "src/dead.ts"]);
        assert_eq!(set(&keys, "enumMembers"), ["src/e.ts|Red"]);
        assert_eq!(set(&keys, "exports"), ["src/e.ts|x"]);
        assert_eq!(set(&keys, "duplicates"), ["src/e.ts|a|b"]);
        assert_eq!(set(&keys, "namespaceMembers"), ["src/e.ts|T"]);
        assert!(!keys.contains_key("devDependencies"));
        assert!(!keys.contains_key("classMembers"));
    }

    #[test]
    fn equal_findings_agree_completely() {
        let skif = json!({ "diagnostics": [
            { "rule": "unused-exports", "message": "export `x` is never imported", "location": { "path": "src/e.ts" } },
        ]});
        let knip = json!({ "issues": [{ "file": "src/e.ts", "exports": [{ "name": "x" }] }] });
        let (counts, _) = diff::compare(&skif_keys(&skif, Path::new("/r")), &knip_keys(&knip, Path::new("/r")));
        assert_eq!(
            counts["exports"],
            Counts {
                skif_only: 0,
                tool_only: 0,
                both: 1
            }
        );
    }

    #[test]
    fn install_commands_follow_the_package_manager() {
        let repo = |pm: &str, install: bool| Repo {
            name: "r".into(),
            url: String::new(),
            rev: "0".repeat(40),
            package_manager: pm.into(),
            suites: vec!["parity".into()],
            sparse: Vec::new(),
            install,
            notes: "why".into(),
        };
        let dir = Path::new("/nonexistent");
        assert_eq!(install_command(&repo("npm", true), dir).unwrap().1[0], "ci");
        assert!(
            install_command(&repo("pnpm", true), dir)
                .unwrap()
                .1
                .contains(&"--ignore-scripts")
        );
        assert!(install_command(&repo("rush", true), dir).is_err());
        assert!(install_command(&repo("pnpm", false), dir).unwrap_err().contains("why"));
    }
}
