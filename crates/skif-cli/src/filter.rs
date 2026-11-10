//! knip's run-time filters: which workspace packages (`--workspace`) and which rules (`--include`, `--exclude` and
//! the shortcuts `--dependencies`, `--exports`, `--files`, `--cycles`) a run reports, and `--tags`.
//!
//! The analysis still covers every package of the root, so a finding in a selected package that depends on the
//! others (an export only another package imports) is the same as in a full run; only the report is narrowed. The
//! filter is applied before fixing, so `--fix` only fixes what is reported.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use skif::{Config, Diagnostic, Level, Linter, Package, Report, RuleConfig, Selector, Workspace, slash};
use skif_migrate::{knip_issue_type_rules, knip_shortcut_types};

use crate::args::LintArgs;
use crate::error::{CliError, Kind};
use crate::roots::RootRun;

/// What a run reports, from the filter flags.
#[derive(Debug, Default)]
pub struct Filter {
    /// `--workspace`: package names or directories (globs, `!` to exclude), or `None` for every package.
    workspaces: Option<Selector>,
    /// `--include` and the shortcuts as rule ids, or `None` for every rule.
    include: Option<BTreeSet<String>>,
    /// `--exclude` as rule ids.
    exclude: BTreeSet<String>,
    /// The included rules the run turns on where they are off, with whether each is on by default: every included
    /// rule that no flag turned off.
    enable: BTreeMap<String, bool>,
    /// `--tags` without the leading `-`: tags whose exports are not reported.
    ignore_tags: Vec<String>,
}

/// Every registered rule id, with whether it is on by default.
fn registry() -> Result<BTreeMap<&'static str, bool>, CliError> {
    let linter = Linter::new(&Config::default())?;
    Ok(linter.rules().map(|meta| (meta.id, meta.default.is_some())).collect())
}

/// The rule ids `value` names: a knip issue type or a skif rule id.
fn rule_ids(flag: &str, value: &str, known: &BTreeMap<&'static str, bool>) -> Result<Vec<String>, CliError> {
    if let Some(rules) = knip_issue_type_rules(value) {
        if rules.is_empty() {
            return Err(CliError::new(
                Kind::Usage,
                format!("{flag}: skif has no rule for the knip issue type `{value}`"),
            ));
        }
        return Ok(rules.into_iter().map(str::to_owned).collect());
    }
    if known.contains_key(value) {
        return Ok(vec![value.to_owned()]);
    }
    Err(CliError::new(
        Kind::Usage,
        format!("{flag}: `{value}` is neither a knip issue type nor a skif rule id (see `skif rules`)"),
    ))
}

/// The knip issue types the shortcut flags select.
fn shortcut_types(args: &LintArgs) -> Vec<&'static str> {
    let production = args.production || args.strict;
    [
        ("dependencies", args.dependencies),
        ("exports", args.exports),
        ("files", args.files),
        ("cycles", args.cycles),
    ]
    .into_iter()
    .filter(|(_, given)| *given)
    .filter_map(|(flag, _)| knip_shortcut_types(flag, production))
    .flatten()
    .collect()
}

/// `--tags` values: knip's `-TAG` excludes the exports with that tag; `+TAG` (only those exports) has no
/// counterpart in skif.
fn ignore_tags(values: &[String]) -> Result<Vec<String>, CliError> {
    values
        .iter()
        .map(|value| match value.strip_prefix('-') {
            Some(tag) if !tag.is_empty() => Ok(tag.trim_start_matches('@').to_owned()),
            _ => Err(CliError::new(
                Kind::Usage,
                format!(
                    "--tags: `{value}` is not supported; skif can only leave out the exports with a tag, written `-TAG`"
                ),
            )),
        })
        .collect()
}

/// A `--workspace` value as a selector pattern: directories are written relative to the root, with or without a
/// leading `./` and a trailing `/`, and the root itself is `.`.
fn workspace_pattern(value: &str) -> String {
    let (negated, body) = value.strip_prefix('!').map_or((false, value), |body| (true, body));
    let body = body.strip_prefix("./").unwrap_or(body);
    let body = body.trim_end_matches('/');
    let body = if body.is_empty() { "." } else { body };
    if negated { format!("!{body}") } else { body.to_owned() }
}

/// The directory of a package as `--workspace` matches it: `.` for the root.
fn package_dir(package: &Package) -> String {
    if package.dir.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        slash(&package.dir)
    }
}

/// The package a diagnostic belongs to: the one it names, else the innermost package whose directory holds its file,
/// else the root. A finding of no package (the root manifest, `pnpm-workspace.yaml`, a catalog) belongs to the root,
/// so `--workspace` reports and fixes it only when it selects the root, and `-W @scope/pkg --fix` leaves root files
/// alone.
fn owner<'w>(workspace: &'w Workspace, diagnostic: &Diagnostic) -> &'w Package {
    if let Some(package) = diagnostic.package_id.and_then(|id| workspace.package(id)) {
        return package;
    }
    let path: &Path = &diagnostic.location.path;
    workspace
        .all_packages()
        .filter(|package| path.starts_with(&package.dir))
        .max_by_key(|package| package.dir.components().count())
        .unwrap_or_else(|| workspace.root_package())
}

impl Filter {
    /// The filter the flags ask for.
    ///
    /// # Errors
    /// A usage error when a `--workspace` pattern is not a valid glob.
    pub fn new(args: &LintArgs) -> Result<Self, CliError> {
        let workspaces = if args.workspace.is_empty() {
            None
        } else {
            let patterns = args.workspace.iter().map(|value| workspace_pattern(value));
            Some(Selector::new(patterns).map_err(|error| CliError::new(Kind::Usage, format!("--workspace: {error}")))?)
        };
        let shortcuts = shortcut_types(args);
        let mut filter = Self {
            workspaces,
            ignore_tags: ignore_tags(&args.tags)?,
            ..Self::default()
        };
        if args.include.is_empty() && args.exclude.is_empty() && shortcuts.is_empty() {
            return Ok(filter);
        }
        let known = registry()?;
        let mut include = BTreeSet::new();
        for value in args.include.iter().map(String::as_str).chain(shortcuts) {
            include.extend(rule_ids("--include", value, &known)?);
        }
        for value in &args.exclude {
            filter.exclude.extend(rule_ids("--exclude", value, &known)?);
        }
        // A rule a flag turned off stays off; `--include` turns on the others, whether the default or the
        // configuration turned them off.
        let turned_off = |id: &str| {
            args.ignore_rule.iter().any(|rule| rule == id)
                || args
                    .rules
                    .iter()
                    .any(|(rule, level)| rule == id && *level == Level::Off)
        };
        filter.enable = include
            .iter()
            .filter(|id| !turned_off(id))
            .filter_map(|id| known.get(id.as_str()).map(|on| (id.clone(), *on)))
            .collect();
        if !include.is_empty() {
            filter.include = Some(include);
        }
        Ok(filter)
    }

    /// Applies the flags that change what the rules check to the configuration of one root: `--include` turns on the
    /// rules it names that are off, and `--tags` adds to the `ignore-tags` of `unused-exports`.
    ///
    /// A rule that is on by default and turned off by the configuration goes back to its default severity; one that
    /// is off by default runs at `error`.
    ///
    /// # Errors
    /// A configuration error when `--tags` is given and the configuration sets `ignore-tags` to something other than
    /// a list.
    pub fn configure(&self, config: &mut Config) -> Result<(), CliError> {
        for (id, on_by_default) in &self.enable {
            let rule = config.rules.entry(id.clone()).or_default();
            match rule.severity {
                Some(Level::Off) if *on_by_default => rule.severity = None,
                Some(Level::Off) | None if !on_by_default => rule.severity = Some(Level::Error),
                _ => {}
            }
        }
        if !self.ignore_tags.is_empty() {
            let rule: &mut RuleConfig = config.rules.entry("unused-exports".to_owned()).or_default();
            let tags = rule
                .options
                .entry("ignore-tags")
                .or_insert_with(|| toml::Value::Array(Vec::new()));
            let toml::Value::Array(list) = tags else {
                return Err(CliError::new(
                    Kind::Config,
                    "--tags: the `ignore-tags` option of `unused-exports` must be a list of tags to add to",
                ));
            };
            list.extend(self.ignore_tags.iter().cloned().map(toml::Value::String));
        }
        Ok(())
    }

    /// Whether the rule flags report `rule`.
    fn reports_rule(&self, rule: &str) -> bool {
        !self.exclude.contains(rule) && self.include.as_ref().is_none_or(|include| include.contains(rule))
    }

    /// Whether `--workspace` selects `package`.
    fn selects(&self, package: &Package) -> bool {
        self.workspaces.as_ref().is_none_or(|selector| {
            let dir = package_dir(package);
            let names = package.name.as_deref().into_iter().chain(std::iter::once(dir.as_str()));
            selector.matches_any(names, false)
        })
    }

    /// Narrows the report of one root to what the flags select. The configuration judgements stay, so the findings
    /// about a configuration file several roots read can be decided again after fixing.
    pub fn apply(&self, workspace: &Workspace, report: Report) -> Report {
        if self.workspaces.is_none() && self.include.is_none() && self.exclude.is_empty() {
            return report;
        }
        let packages = if self.workspaces.is_some() {
            workspace.all_packages().filter(|package| self.selects(package)).count()
        } else {
            report.summary.packages
        };
        let diagnostics = report
            .diagnostics
            .into_iter()
            .filter(|diagnostic| self.reports_rule(diagnostic.rule.as_str()))
            .filter(|diagnostic| self.selects(owner(workspace, diagnostic)))
            .collect();
        Report::new(diagnostics, packages).with_config_judgements(report.config_judgements)
    }

    /// Narrows every root's report, after checking that `--workspace` selects a package in at least one of them.
    ///
    /// # Errors
    /// A usage error when `--workspace` was given and selects no package.
    pub fn apply_runs(&self, runs: &mut [RootRun]) -> Result<(), CliError> {
        if let Some(selector) = &self.workspaces {
            let any = runs
                .iter()
                .any(|run| run.workspace.all_packages().any(|package| self.selects(package)));
            if !any && !runs.is_empty() {
                return Err(CliError::new(
                    Kind::Usage,
                    format!(
                        "--workspace {} matches no workspace package (by name or directory)",
                        selector.patterns().join(", ")
                    ),
                ));
            }
        }
        for run in runs {
            let report = std::mem::replace(&mut run.report, Report::new(Vec::new(), 0));
            run.report = self.apply(&run.workspace, report);
        }
        Ok(())
    }
}

/// `--max-show-issues`: at most `max` findings of each rule, counted over the roots in order. Each report keeps the
/// summary of all its findings. Returns the reports and how many findings were left out.
pub fn truncate_shown(reports: Vec<Cow<'_, Report>>, max: usize) -> (Vec<Cow<'_, Report>>, usize) {
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut hidden = 0;
    let shown = reports
        .into_iter()
        .map(|report| {
            let summary = report.summary;
            let diagnostics: Vec<Diagnostic> = report
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    let count = seen.entry(diagnostic.rule.as_str().to_owned()).or_default();
                    *count += 1;
                    let keep = *count <= max;
                    hidden += usize::from(!keep);
                    keep
                })
                .cloned()
                .collect();
            if diagnostics.len() == report.diagnostics.len() {
                return report;
            }
            let mut truncated = Report::new(diagnostics, summary.packages);
            truncated.summary = summary;
            Cow::Owned(truncated)
        })
        .collect();
    (shown, hidden)
}

#[cfg(test)]
mod tests {
    use super::workspace_pattern;

    #[test]
    fn workspace_values_become_patterns() {
        assert_eq!(workspace_pattern("./packages/a/"), "packages/a");
        assert_eq!(workspace_pattern("."), ".");
        assert_eq!(workspace_pattern("./"), ".");
        assert_eq!(workspace_pattern("@scope/a"), "@scope/a");
        assert_eq!(workspace_pattern("!./packages/*"), "!packages/*");
    }
}
