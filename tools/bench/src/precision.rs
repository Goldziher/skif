//! Dead-code precision and recall against an answer key: generates a workspace with seeded dead code
//! ([`skif_bench::deadcode`]), runs skif and optionally the pinned knip on it, and scores both tools' findings.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use skif_bench::deadcode::{self, AnswerKey, CategoryScore, PATTERNS, Score};
use skif_bench::generate;

use crate::corpus;
use crate::parity::{empty_knip_config, knip_keys, parse_knip_report, skif_keys};
use crate::proc::{capture, find_on_path};

pub struct Options {
    pub skif: PathBuf,
    pub packages: usize,
    pub seed: u64,
    pub out: PathBuf,
    pub knip: bool,
    pub corpus_file: PathBuf,
    pub json: bool,
    pub timeout: Duration,
}

fn run_skif(skif: &Path, root: &Path, scratch: &Path, timeout: Duration) -> Result<Value> {
    let run = capture(
        Command::new(skif)
            .args([".", "--format", "json", "--no-install"])
            .current_dir(root),
        scratch,
        "skif",
        timeout,
    )?;
    if run.timed_out {
        bail!("skif timed out");
    }
    let document: Value = serde_json::from_slice(&run.stdout)
        .with_context(|| format!("skif output is not JSON; stderr: {}", run.stderr_summary()))?;
    if document.get("diagnostics").is_none() {
        bail!(
            "skif failed: {}",
            document["error"]["message"]
                .as_str()
                .unwrap_or("no diagnostics in the output")
        );
    }
    Ok(document)
}

fn run_knip(version: &str, root: &Path, scratch: &Path, timeout: Duration) -> Result<Value> {
    if find_on_path("npx").is_none() {
        bail!("npx is required to run the pinned knip: install Node.js");
    }
    let config = empty_knip_config(scratch)?;
    let spec = format!("knip@{version}");
    let run = capture(
        Command::new("npx")
            .args(["--yes", spec.as_str(), "--reporter", "json", "--config"])
            .arg(config)
            .current_dir(root)
            .env("CI", "1")
            .env("DOTENV_CONFIG_QUIET", "true"),
        scratch,
        "knip",
        timeout,
    )?;
    if run.timed_out {
        bail!("knip timed out");
    }
    parse_knip_report(&run.stdout).map_err(|reason| {
        anyhow::anyhow!(
            "knip failed (exit {}): {reason}; stderr: {}",
            run.code.map_or_else(|| "none".to_owned(), |c| c.to_string()),
            run.stderr_summary()
        )
    })
}

fn ratio(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_owned(), |v| format!("{v:.3}"))
}

fn cell(score: Option<&CategoryScore>) -> String {
    score.map_or_else(
        || "- | - | -".to_owned(),
        |s| {
            format!(
                "{}/{} | {} | {}",
                s.true_positives,
                s.reported,
                ratio(s.precision()),
                ratio(s.recall())
            )
        },
    )
}

/// The report as Markdown: one row per category, one column group per tool, then the false findings by pattern and
/// every false finding and miss.
fn markdown(key: &AnswerKey, tools: &[(String, Score)]) -> String {
    let mut out = format!(
        "# Dead-code precision and recall\n\nSeed {}, {} packages, {} expected findings, {} live items reached through {} patterns.\n\n| Category | Expected |",
        key.seed,
        key.packages,
        key.expected.len(),
        key.live.len(),
        PATTERNS.len()
    );
    let mut rule = String::from("| --- | ---: |");
    for (tool, _) in tools {
        let _ = write!(out, " {tool} TP/reported | precision | recall |");
        rule.push_str(" ---: | ---: | ---: |");
    }
    let _ = writeln!(out, "\n{rule}");
    let mut categories: Vec<&String> = tools.iter().flat_map(|(_, s)| s.categories.keys()).collect();
    categories.sort();
    categories.dedup();
    let row = |label: &str, expected: usize, pick: &dyn Fn(&Score) -> Option<&CategoryScore>| {
        let mut line = format!("| {label} | {expected} |");
        for (_, score) in tools {
            let _ = write!(line, " {} |", cell(pick(score)));
        }
        line
    };
    for category in categories {
        let expected = tools
            .iter()
            .find_map(|(_, s)| s.categories.get(category))
            .map_or(0, |s| s.expected);
        let _ = writeln!(out, "{}", row(category, expected, &|s| s.categories.get(category)));
    }
    let _ = writeln!(out, "{}", row("**all**", key.expected.len(), &|s| Some(&s.total)));
    for (tool, score) in tools {
        let _ = writeln!(out, "\n## {tool}\n");
        if score.false_by_pattern.is_empty() {
            out.push_str("No false findings.\n");
        } else {
            out.push_str("False findings by live pattern:\n\n");
            for (pattern, count) in &score.false_by_pattern {
                let _ = writeln!(out, "- `{pattern}`: {count}");
            }
        }
        for (label, keys) in [
            ("False findings", &score.total.false_positives),
            ("Missed", &score.total.false_negatives),
        ] {
            if !keys.is_empty() {
                let _ = writeln!(out, "\n{label}:\n");
                for finding in keys {
                    let _ = writeln!(out, "- `{finding}`");
                }
            }
        }
    }
    out
}

/// Generates the workspace, runs the tools and prints the scores.
///
/// # Errors
/// Fails when the output directory cannot be written, skif is missing or fails, or knip was asked for and fails.
pub fn run(options: &Options) -> Result<()> {
    let skif = fs::canonicalize(&options.skif).with_context(|| {
        format!(
            "skif binary not found at {} (build it or pass --skif)",
            options.skif.display()
        )
    })?;
    let generated = deadcode::build(&deadcode::Options {
        packages: options.packages,
        seed: options.seed,
    })?;
    let repo = options.out.join("repo");
    if repo.exists() {
        fs::remove_dir_all(&repo).with_context(|| format!("cleaning {}", repo.display()))?;
    }
    generate::write_files(&generated.files, &repo)?;
    fs::write(
        options.out.join("answer-key.json"),
        serde_json::to_string_pretty(&generated.answer_key)? + "\n",
    )?;
    let root = fs::canonicalize(&repo)?;
    let scratch = options.out.join("scratch");

    let mut scores = Vec::new();
    let skif_doc = run_skif(&skif, &root, &scratch, options.timeout)?;
    let skif_found = skif_keys(&skif_doc, &root);
    scores.push(("skif".to_owned(), deadcode::score(&generated.answer_key, &skif_found)));
    if options.knip {
        let manifest = corpus::load_manifest(&options.corpus_file)?;
        let knip_doc = run_knip(&manifest.tools.knip, &root, &scratch, options.timeout)?;
        let knip_found = knip_keys(&knip_doc, &root);
        scores.push((
            format!("knip@{}", manifest.tools.knip),
            deadcode::score(&generated.answer_key, &knip_found),
        ));
    }

    let table = markdown(&generated.answer_key, &scores);
    fs::write(options.out.join("deadcode.md"), &table)?;
    let report = json!({
        "seed": options.seed,
        "packages": options.packages,
        "expected": generated.answer_key.expected.len(),
        "tools": scores.iter().map(|(tool, score)| {
            let categories: serde_json::Map<String, Value> = score.categories.iter().map(|(category, s)| {
                (category.clone(), json!({
                    "expected": s.expected,
                    "reported": s.reported,
                    "true_positives": s.true_positives,
                    "precision": s.precision(),
                    "recall": s.recall(),
                    "false_positives": s.false_positives,
                    "false_negatives": s.false_negatives,
                }))
            }).collect();
            (tool.clone(), json!({
                "precision": score.total.precision(),
                "recall": score.total.recall(),
                "categories": categories,
                "false_by_pattern": score.false_by_pattern,
            }))
        }).collect::<serde_json::Map<String, Value>>(),
    });
    let json_text = serde_json::to_string_pretty(&report)? + "\n";
    fs::write(options.out.join("deadcode.json"), &json_text)?;
    if options.json {
        print!("{json_text}");
    } else {
        println!("{table}");
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use skif::{ConfigOverrides, Linter, MemoryFs, Workspace, load_config};
    use skif_bench::deadcode::{DeadCode, Options as DeadCodeOptions};

    use super::*;
    use crate::diff::Keys;

    const ROOT: &str = "/repo";

    /// skif's findings on a generated workspace, run in process and keyed exactly like the CLI's JSON report.
    fn skif_in_process(generated: &DeadCode) -> Keys {
        let mut fs = MemoryFs::new();
        for (path, text) in &generated.files {
            fs.insert(format!("{ROOT}/{}", path.to_string_lossy()), text.clone());
        }
        let fs = Arc::new(fs);
        let root = Path::new(ROOT);
        let config = load_config(root, &*fs, &ConfigOverrides::default()).unwrap();
        let workspace = Workspace::discover_with_fs(root, &config, &*fs).unwrap();
        let report = Linter::with_fs(&config, fs).unwrap().run(&workspace).unwrap();
        skif_keys(&serde_json::to_value(&report).unwrap(), root)
    }

    /// Known deviations of skif from the answer key, as `<category>:<key>` per package. Each one is a false finding
    /// that [issue #26](https://github.com/Goldziher/skif/issues/26) measures; remove an entry when skif stops making it.
    ///
    /// - `fixtures/sample.ts`: skif's `unused-files` reports modules in `fixtures` directories. ADR 0023 skips them
    ///   only in root discovery, and the analysis still reads them; the answer key counts fixtures as test data.
    fn known_false_findings(packages: usize) -> Vec<String> {
        (0..packages)
            .map(|i| format!("files:packages/p{i}/fixtures/sample.ts"))
            .collect()
    }

    #[test]
    fn skif_finds_every_seeded_finding_and_only_the_known_false_ones() {
        for seed in [1, 26] {
            let packages = 3;
            let generated = deadcode::build(&DeadCodeOptions { packages, seed }).unwrap();
            let score = deadcode::score(&generated.answer_key, &skif_in_process(&generated));
            assert_eq!(
                score.total.recall(),
                Some(1.0),
                "seed {seed}: missed {:?}",
                score.total.false_negatives
            );
            let mut false_findings = score.total.false_positives.clone();
            false_findings.sort();
            assert_eq!(false_findings, known_false_findings(packages), "seed {seed}");
            assert_eq!(score.false_by_pattern.keys().collect::<Vec<_>>(), ["fixture-dir"]);
        }
    }

    #[test]
    fn the_report_has_a_column_group_per_tool_and_lists_false_findings() {
        let generated = deadcode::build(&DeadCodeOptions { packages: 1, seed: 1 }).unwrap();
        let mut found = generated.answer_key.keys();
        found
            .entry("files".to_owned())
            .or_default()
            .insert("packages/p0/src/lazy.ts".to_owned());
        let scores = [
            (
                "skif".to_owned(),
                deadcode::score(&generated.answer_key, &generated.answer_key.keys()),
            ),
            ("knip".to_owned(), deadcode::score(&generated.answer_key, &found)),
        ];
        let table = markdown(&generated.answer_key, &scores);
        assert!(table.contains("| skif TP/reported | precision | recall | knip TP/reported |"));
        assert!(table.contains("`dynamic-import`: 1"));
        assert!(table.contains("- `files:packages/p0/src/lazy.ts`"));
        assert!(table.contains("No false findings."));
    }
}
