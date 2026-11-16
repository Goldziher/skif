//! `skif-bench tools`: keeps the exact comparison tool pins of `corpus.toml` at the latest npm releases.
//!
//! The pins stay exact (ADR 0014), so a new release is adopted by `tools update`, which rewrites the `[tools]` table,
//! the `tools/package.json` that repeats it and the lock; `tools check` fails while a pin is behind.

use std::cmp::Ordering;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::{corpus, proc};

/// The comparison tools of the `[tools]` table, in the order they are reported.
const TOOL_NAMES: [&str; 3] = ["knip", "syncpack", "sherif"];

const REGISTRY_TIMEOUT: Duration = Duration::from_secs(120);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);

/// Looks up the latest release of an npm package; injected so tests never reach the network.
pub type Resolver<'a> = &'a dyn Fn(&str) -> Result<String>;

/// A tool's pinned version beside the latest release.
#[derive(Debug, PartialEq, Eq)]
pub struct Status {
    pub tool: &'static str,
    pub pinned: String,
    pub latest: String,
}

impl Status {
    /// Whether the pin is older than the latest release; a latest below the pin counts as current.
    fn behind(&self) -> Result<bool> {
        let pinned = parse_version(&self.pinned)
            .with_context(|| format!("{}: cannot read the pinned version {:?}", self.tool, self.pinned))?;
        let latest = parse_version(&self.latest)
            .with_context(|| format!("{}: cannot read the latest version {:?}", self.tool, self.latest))?;
        Ok(compare(&pinned, &latest) == Ordering::Less)
    }
}

type Version = ([u64; 3], Option<String>);

/// `major.minor.patch` with an optional pre-release; build metadata is dropped.
fn parse_version(text: &str) -> Option<Version> {
    let text = text.split('+').next()?;
    let (core, pre) = match text.split_once('-') {
        Some((core, pre)) => (core, Some(pre.to_owned())),
        None => (text, None),
    };
    let mut parts = core.split('.');
    let mut numbers = [0; 3];
    for number in &mut numbers {
        *number = parts.next()?.parse().ok()?;
    }
    parts.next().is_none().then_some((numbers, pre))
}

/// Semver precedence: a release is above its own pre-releases; pre-release identifiers compare field by field,
/// numerically when both are numbers.
fn compare(a: &Version, b: &Version) -> Ordering {
    a.0.cmp(&b.0).then_with(|| match (&a.1, &b.1) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => compare_prerelease(a, b),
    })
}

fn compare_prerelease(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.split('.'), b.split('.'));
    loop {
        match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let order = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
        }
    }
}

/// The `[tools]` versions of corpus manifest text, in [`TOOL_NAMES`] order.
fn read_pins(corpus_text: &str) -> Result<Vec<(&'static str, String)>> {
    let table: toml::Table = toml::from_str(corpus_text).context("parsing the corpus manifest")?;
    let tools = table
        .get("tools")
        .and_then(toml::Value::as_table)
        .context("the corpus manifest has no [tools] table")?;
    TOOL_NAMES
        .iter()
        .map(|&tool| {
            let version = tools
                .get(tool)
                .and_then(toml::Value::as_str)
                .with_context(|| format!("[tools] has no string version for {tool}"))?;
            Ok((tool, version.to_owned()))
        })
        .collect()
}

/// Every tool's pin beside its latest release; fails when a lookup does.
fn statuses(corpus_text: &str, resolve: Resolver<'_>) -> Result<Vec<Status>> {
    read_pins(corpus_text)?
        .into_iter()
        .map(|(tool, pinned)| {
            let latest = resolve(tool).with_context(|| format!("looking up the latest {tool}"))?;
            Ok(Status {
                tool,
                pinned,
                latest: latest.trim().to_owned(),
            })
        })
        .collect()
}

/// `line` with the quoted value after `key` and `delim` replaced, or `None` when the line is not that assignment.
fn swap_value(line: &str, key: &str, delim: char, new: &str) -> Option<String> {
    let indent = line.len() - line.trim_start().len();
    let after_key = line[indent..].strip_prefix(key)?;
    let after_delim = after_key.trim_start().strip_prefix(delim)?;
    let value_start = line.len() - after_delim.trim_start().len();
    let value = line[value_start..].strip_prefix('"')?;
    let quote_end = value_start + 1 + value.find('"')?;
    Some(format!("{}{new}{}", &line[..=value_start], &line[quote_end..]))
}

/// Applies `new` (tool, version) to the lines `swap` accepts within the scope `in_scope` tracks, keeping every other
/// byte; fails when a tool is not found.
fn rewrite(
    text: &str,
    new: &[(&str, &str)],
    what: &str,
    swap: impl Fn(&str, &str, &str) -> Option<String>,
    in_scope: impl Fn(&str, bool) -> bool,
) -> Result<String> {
    let mut found = vec![false; new.len()];
    let mut scoped = false;
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        scoped = in_scope(line, scoped);
        let mut replaced = None;
        if scoped {
            for (i, (tool, version)) in new.iter().enumerate() {
                if let Some(changed) = swap(line, tool, version) {
                    found[i] = true;
                    replaced = Some(changed);
                    break;
                }
            }
        }
        out.push_str(replaced.as_deref().unwrap_or(line));
    }
    if let Some(i) = found.iter().position(|f| !f) {
        bail!("{what}: no version line for {}", new[i].0);
    }
    Ok(out)
}

/// `corpus_text` with the `[tools]` versions replaced and every other byte unchanged.
fn rewrite_tools_table(corpus_text: &str, new: &[(&str, &str)]) -> Result<String> {
    rewrite(
        corpus_text,
        new,
        "[tools]",
        |line, tool, version| swap_value(line, tool, '=', version),
        |line, scoped| {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                trimmed.strip_prefix("[tools]").is_some_and(|rest| {
                    let rest = rest.trim_start();
                    rest.is_empty() || rest.starts_with('#')
                })
            } else {
                scoped
            }
        },
    )
}

/// `package.json` text with the tool dependency versions replaced and every other byte unchanged.
fn rewrite_package_json(text: &str, new: &[(&str, &str)]) -> Result<String> {
    rewrite(
        text,
        new,
        "tools/package.json",
        |line, tool, version| swap_value(line, &format!("\"{tool}\""), ':', version),
        |_, _| true,
    )
}

/// `tools check`: errors listing each tool whose pin is behind its latest release, naming the fix.
///
/// # Errors
/// Fails when the manifest cannot be read, a lookup fails, or a pin is behind.
pub fn check(corpus_file: &Path, resolve: Resolver<'_>, out: &mut dyn Write) -> Result<()> {
    let text = fs::read_to_string(corpus_file).with_context(|| format!("reading {}", corpus_file.display()))?;
    let mut behind = Vec::new();
    for status in statuses(&text, resolve)? {
        if status.behind()? {
            behind.push(format!(
                "{} pinned {}, latest {}",
                status.tool, status.pinned, status.latest
            ));
        } else {
            writeln!(out, "{} {} is current", status.tool, status.pinned)?;
        }
    }
    if behind.is_empty() {
        return Ok(());
    }
    bail!(
        "comparison tool pins are behind the latest release:\n{}\nrun `skif-bench tools update`",
        behind.join("\n")
    )
}

/// `tools update`: moves every pin that is behind to its latest release in `corpus.toml` and `tools/package.json`, then
/// has `relock` regenerate the lock in the `tools/` directory. Nothing is written when every pin is current or a
/// lookup fails; when `relock` fails the files are restored.
///
/// # Errors
/// Fails when a file cannot be read or written, a lookup fails or `relock` fails.
pub fn update(
    corpus_file: &Path,
    resolve: Resolver<'_>,
    relock: &dyn Fn(&Path) -> Result<()>,
    out: &mut dyn Write,
) -> Result<()> {
    let text = fs::read_to_string(corpus_file).with_context(|| format!("reading {}", corpus_file.display()))?;
    let lock_dir = corpus::tool_lock_dir(corpus_file);
    let package_path = lock_dir.join("package.json");
    let package_text =
        fs::read_to_string(&package_path).with_context(|| format!("reading {}", package_path.display()))?;
    let found = statuses(&text, resolve)?;
    let mut changed = false;
    let mut new = Vec::new();
    for status in &found {
        if status.behind()? {
            writeln!(out, "{} {} -> {}", status.tool, status.pinned, status.latest)?;
            new.push((status.tool, status.latest.as_str()));
            changed = true;
        } else {
            writeln!(out, "{} {} is current", status.tool, status.pinned)?;
            new.push((status.tool, status.pinned.as_str()));
        }
    }
    if !changed {
        return Ok(());
    }
    let (old_text, old_package_text) = (text.clone(), package_text.clone());
    let text = rewrite_tools_table(&text, &new)?;
    let package_text = rewrite_package_json(&package_text, &new)?;
    fs::write(corpus_file, text).with_context(|| format!("writing {}", corpus_file.display()))?;
    fs::write(&package_path, package_text).with_context(|| format!("writing {}", package_path.display()))?;
    let lock_path = lock_dir.join("package-lock.json");
    let lock_text = fs::read_to_string(&lock_path).ok();
    if let Err(error) = relock(&lock_dir) {
        let mut restored = vec![
            fs::write(corpus_file, &old_text).map_err(|e| format!("{}: {e}", corpus_file.display())),
            fs::write(&package_path, &old_package_text).map_err(|e| format!("{}: {e}", package_path.display())),
        ];
        if let Some(lock_text) = &lock_text {
            restored.push(fs::write(&lock_path, lock_text).map_err(|e| format!("{}: {e}", lock_path.display())));
        }
        let failed: Vec<String> = restored.into_iter().filter_map(Result::err).collect();
        if failed.is_empty() {
            return Err(error.context("relocking failed; corpus.toml, package.json and the lock are restored"));
        }
        return Err(error.context(format!(
            "relocking failed and restoring failed too, so revert by hand: {}",
            failed.join("; ")
        )));
    }
    writeln!(
        out,
        "re-record the parity, syncpack and type-aware baselines (see tools/bench/README.md)"
    )?;
    Ok(())
}

/// The npm executable's file name: on Windows a `.cmd` shim.
pub const NPM_PROGRAM: &str = if cfg!(windows) { "npm.cmd" } else { "npm" };

pub fn npm_command() -> Command {
    Command::new(NPM_PROGRAM)
}

/// Runs an npm command and returns its stdout; a failure or timeout names `what`.
fn run_npm(mut command: Command, label: &str, timeout: Duration, what: &str) -> Result<Vec<u8>> {
    let scratch = std::env::temp_dir().join(format!("skif-bench-tools-{label}-{}", std::process::id()));
    let captured = proc::capture(&mut command, &scratch, label, timeout);
    let _ = fs::remove_dir_all(&scratch);
    let captured = captured?;
    if captured.timed_out {
        bail!("`{what}` timed out");
    }
    if captured.code != Some(0) {
        bail!("`{what}` failed: {}", captured.failure_summary());
    }
    Ok(captured.stdout)
}

/// The latest release of `package` from the npm registry (`npm view <package> version`, the `latest` dist-tag).
///
/// # Errors
/// Fails when npm cannot run, exits non-zero or prints no exact version, so an offline run never passes silently.
pub fn npm_latest(package: &str) -> Result<String> {
    let mut command = npm_command();
    command.args(["view", package, "version"]);
    let what = format!("npm view {package} version");
    let stdout = run_npm(command, "view", REGISTRY_TIMEOUT, &what)?;
    let version = String::from_utf8_lossy(&stdout).trim().to_owned();
    if !corpus::is_exact_version(&version) {
        bail!("`{what}` printed {version:?}, not a version");
    }
    Ok(version)
}

/// Regenerates `package-lock.json` in `dir` from its `package.json` without installing or running scripts.
///
/// # Errors
/// Fails when npm cannot run or exits non-zero.
pub fn npm_relock(dir: &Path) -> Result<()> {
    let mut command = npm_command();
    command
        .args([
            "install",
            "--package-lock-only",
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
        ])
        .current_dir(dir);
    let what = format!("npm install --package-lock-only in {}", dir.display());
    run_npm(command, "relock", INSTALL_TIMEOUT, &what).map(drop)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const CORPUS: &str = "# header\r\n\r\n# about [tools]\n[tools]\nknip = \"1.0.0\"  # keep\nsyncpack = \"2.0.0\"\nsherif=\"3.0.0\"\n\n[[repo]]\nname = \"x\"\nknip = \"9.9.9\"\n";
    const PACKAGE: &str = "{\n  \"description\": \"knip\",\n  \"dependencies\": {\n    \"knip\": \"1.0.0\",\n    \"sherif\": \"3.0.0\",\n    \"syncpack\": \"2.0.0\"\n  }\n}\n";

    fn fixture(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("skif-bench-tools-test-{name}-{}", std::process::id()));
        fs::create_dir_all(dir.join("tools")).unwrap();
        fs::write(dir.join("corpus.toml"), CORPUS).unwrap();
        fs::write(dir.join("tools").join("package.json"), PACKAGE).unwrap();
        dir
    }

    fn resolver(knip: &'static str, syncpack: &'static str, sherif: &'static str) -> impl Fn(&str) -> Result<String> {
        move |tool| match tool {
            "knip" => Ok(knip.to_owned()),
            "syncpack" => Ok(syncpack.to_owned()),
            "sherif" => Ok(sherif.to_owned()),
            other => bail!("unexpected {other}"),
        }
    }

    #[test]
    fn compares_by_semver_precedence() {
        let behind =
            |a: &str, b: &str| compare(&parse_version(a).unwrap(), &parse_version(b).unwrap()) == Ordering::Less;
        assert!(behind("1.9.0", "1.10.0"));
        assert!(behind("2.0.0-beta.2", "2.0.0"));
        assert!(behind("2.0.0-beta.2", "2.0.0-beta.10"));
        assert!(!behind("2.0.0", "2.0.0-beta.2"));
        assert!(!behind("2.0.0+build.1", "2.0.0"));
        assert!(!behind("3.0.0", "2.9.9"));
        assert!(parse_version("1.2").is_none());
        assert!(parse_version("latest").is_none());
    }

    #[test]
    fn the_tools_table_is_rewritten_and_the_rest_is_kept() {
        let rewritten =
            rewrite_tools_table(CORPUS, &[("knip", "1.1.0"), ("syncpack", "2.0.0"), ("sherif", "3.0.1")]).unwrap();
        assert_eq!(
            rewritten,
            CORPUS
                .replace("knip = \"1.0.0\"", "knip = \"1.1.0\"")
                .replace("sherif=\"3.0.0\"", "sherif=\"3.0.1\"")
        );
        assert!(rewritten.contains("\"1.1.0\"  # keep"));
        assert!(
            rewritten.contains("knip = \"9.9.9\""),
            "a key of another table is untouched"
        );
        let missing = rewrite_tools_table("[tools]\nknip = \"1.0.0\"\n", &[("knip", "1.1.0"), ("sherif", "3.0.0")]);
        assert!(missing.unwrap_err().to_string().contains("no version line for sherif"));
    }

    #[test]
    fn the_package_json_versions_are_rewritten_and_the_rest_is_kept() {
        let rewritten = rewrite_package_json(
            PACKAGE,
            &[("knip", "1.1.0"), ("syncpack", "2.0.0"), ("sherif", "3.0.1")],
        )
        .unwrap();
        assert_eq!(
            rewritten,
            PACKAGE
                .replace("\"knip\": \"1.0.0\"", "\"knip\": \"1.1.0\"")
                .replace("\"sherif\": \"3.0.0\"", "\"sherif\": \"3.0.1\"")
        );
    }

    #[test]
    fn check_passes_when_current_or_ahead_and_lists_the_tools_behind() {
        let dir = fixture("check");
        let file = dir.join("corpus.toml");
        let mut out = Vec::new();
        check(&file, &resolver("1.0.0", "2.0.0", "2.9.9"), &mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().contains("sherif 3.0.0 is current"));
        let error = check(&file, &resolver("1.2.0", "2.0.0", "3.0.1"), &mut Vec::new())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("knip pinned 1.0.0, latest 1.2.0\nsherif pinned 3.0.0, latest 3.0.1"),
            "{error}"
        );
        assert!(!error.contains("syncpack"), "{error}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_lookup_is_an_error_not_a_pass() {
        let dir = fixture("offline");
        let offline = |_: &str| -> Result<String> { bail!("getaddrinfo ENOTFOUND registry.npmjs.org") };
        let error = format!(
            "{:#}",
            check(&dir.join("corpus.toml"), &offline, &mut Vec::new()).unwrap_err()
        );
        assert!(
            error.contains("looking up the latest knip") && error.contains("ENOTFOUND"),
            "{error}"
        );
        let relock = |_: &Path| -> Result<()> { bail!("must not run") };
        assert!(update(&dir.join("corpus.toml"), &offline, &relock, &mut Vec::new()).is_err());
        assert_eq!(fs::read_to_string(dir.join("corpus.toml")).unwrap(), CORPUS);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_relock_restores_the_files() {
        let dir = fixture("relock-fails");
        let file = dir.join("corpus.toml");
        let lock = dir.join("tools").join("package-lock.json");
        fs::write(&lock, "lock").unwrap();
        let relock = |lock_dir: &Path| -> Result<()> {
            fs::write(lock_dir.join("package-lock.json"), "half written").unwrap();
            bail!("npm ERR! network")
        };
        let error = format!(
            "{:#}",
            update(&file, &resolver("1.1.0", "2.0.0", "3.0.0"), &relock, &mut Vec::new()).unwrap_err()
        );
        assert!(
            error.contains("restored") && error.contains("npm ERR! network"),
            "{error}"
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), CORPUS);
        assert_eq!(
            fs::read_to_string(dir.join("tools").join("package.json")).unwrap(),
            PACKAGE
        );
        assert_eq!(fs::read_to_string(&lock).unwrap(), "lock");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn update_rewrites_both_files_and_relocks_only_when_behind() {
        let dir = fixture("update");
        let file = dir.join("corpus.toml");
        let relocked = std::cell::RefCell::new(Vec::new());
        let relock = |lock_dir: &Path| -> Result<()> {
            relocked.borrow_mut().push(lock_dir.to_owned());
            Ok(())
        };
        update(&file, &resolver("1.0.0", "2.0.0", "3.0.0"), &relock, &mut Vec::new()).unwrap();
        assert!(relocked.borrow().is_empty());
        assert_eq!(fs::read_to_string(&file).unwrap(), CORPUS);

        let mut out = Vec::new();
        update(&file, &resolver("1.1.0", "2.0.0", "3.0.0"), &relock, &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains("knip 1.0.0 -> 1.1.0"), "{out}");
        assert_eq!(*relocked.borrow(), [dir.join("tools")]);
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            CORPUS.replace("knip = \"1.0.0\"", "knip = \"1.1.0\"")
        );
        assert_eq!(
            fs::read_to_string(dir.join("tools").join("package.json")).unwrap(),
            PACKAGE.replace("\"knip\": \"1.0.0\"", "\"knip\": \"1.1.0\"")
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
