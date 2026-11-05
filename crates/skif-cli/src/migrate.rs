//! `skif migrate`: translate the configuration of sherif, syncpack and knip into a `skif.toml`.
//!
//! The command reads files and writes one; the translation itself is `skif-migrate`, which is pure. Nothing is ever
//! deleted.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};
use skif::{CONFIG_FILE_NAMES, Config, Linter, slash};
use skif_migrate::{
    Input, MigrateError, Migration, Options, Profile, RuleInfo, Tool, ToolInput, config_file_names, migrate,
    package_json_mentions, script_config_path,
};

use crate::args::{Format, MigrateArgs, MigrateFrom, MigrateProfile, RulesFormat};
use crate::error::{CliError, Kind};
use crate::output::{emit_error, stderr_line, write_stdout};
use crate::write::write_atomic;

const CLEAN: u8 = 0;
const NEEDS_HUMAN: u8 = 1;
const COULD_NOT_RUN: u8 = 2;

pub fn run(args: &MigrateArgs) -> u8 {
    let format = Format::from(args.format);
    match execute(args, args.format) {
        Ok(code) => code,
        Err(error) => {
            emit_error(&error, format);
            COULD_NOT_RUN
        }
    }
}

fn read_text(path: &Path) -> Result<String, CliError> {
    fs::read_to_string(path).map_err(|error| CliError::io("cannot read", path, &error))
}

fn read_optional(path: &Path) -> Result<Option<String>, CliError> {
    if path.is_file() {
        read_text(path).map(Some)
    } else {
        Ok(None)
    }
}

fn known_rules() -> Result<Vec<RuleInfo>, CliError> {
    let linter = Linter::new(&Config::default())?;
    Ok(linter
        .rules()
        .map(|meta| RuleInfo::new(meta.id.to_owned(), meta.default.is_some()))
        .collect())
}

#[allow(clippy::needless_pass_by_value, reason = "used as `map_err(migrate_error)`")]
fn migrate_error(error: MigrateError) -> CliError {
    match error {
        MigrateError::Unreadable { ref path, .. } => CliError {
            path: Some(path.clone()),
            ..CliError::new(Kind::Config, error.to_string())
        },
        MigrateError::Existing(_) => CliError::new(Kind::Config, error.to_string()),
        _ => CliError::new(Kind::Internal, error.to_string()),
    }
}

/// The workspace patterns of the root: `package.json` `workspaces`, or `pnpm-workspace.yaml` `packages`.
fn workspace_patterns(root: &Path, package_json: Option<&str>) -> Vec<String> {
    let from_package = package_json
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|package| {
            let workspaces = package.get("workspaces")?;
            let list = workspaces.get("packages").unwrap_or(workspaces);
            Some(
                list.as_array()?
                    .iter()
                    .filter_map(|pattern| pattern.as_str().map(str::to_owned))
                    .collect::<Vec<String>>(),
            )
        });
    if let Some(patterns) = from_package {
        return patterns;
    }
    let Ok(text) = fs::read_to_string(root.join("pnpm-workspace.yaml")) else {
        return Vec::new();
    };
    pnpm_packages(&text)
}

/// The `packages:` list of a `pnpm-workspace.yaml`, read line by line: the file is a short block sequence.
fn pnpm_packages(text: &str) -> Vec<String> {
    let mut patterns = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !line.starts_with([' ', '\t', '-']) {
            if let Some(flow) = trimmed.strip_prefix("packages:").map(str::trim)
                && flow.starts_with('[')
            {
                patterns.extend(flow_sequence(flow));
                inside = false;
            } else {
                inside = trimmed.trim_end() == "packages:";
            }
            continue;
        }
        if inside && let Some(entry) = trimmed.strip_prefix('-') {
            let entry = scalar(entry);
            if !entry.is_empty() {
                patterns.push(entry.to_owned());
            }
        }
    }
    patterns
}

/// A YAML scalar of a sequence entry: the text inside its quotes, or the plain text up to a trailing `#` comment.
fn scalar(entry: &str) -> &str {
    let entry = entry.trim();
    for quote in ['\'', '"'] {
        if let Some(rest) = entry.strip_prefix(quote) {
            return rest.split(quote).next().unwrap_or(rest);
        }
    }
    entry.split(" #").next().unwrap_or(entry).trim()
}

/// The entries of a one-line YAML flow sequence such as `["apps/*", 'packages/*']`.
fn flow_sequence(text: &str) -> Vec<String> {
    let inner = text.trim().trim_start_matches('[');
    let inner = inner.split(']').next().unwrap_or(inner);
    inner
        .split(',')
        .map(|entry| entry.trim().trim_matches(['\'', '"']))
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect()
}

fn tool_of(from: MigrateFrom) -> Option<Tool> {
    match from {
        MigrateFrom::Sherif => Some(Tool::Sherif),
        MigrateFrom::Syncpack => Some(Tool::Syncpack),
        MigrateFrom::Knip => Some(Tool::Knip),
        MigrateFrom::Auto => None,
    }
}

/// The configuration file of `tool` in `root`: `explicit`, else the file a script names, else the first of the
/// tool's usual names that exists. The path is relative to `root` when it is inside it.
fn find_config(root: &Path, tool: Tool, explicit: Option<&Path>, package_json: Option<&str>) -> Option<PathBuf> {
    if let Some(path) = explicit {
        return Some(path.to_path_buf());
    }
    let from_script = package_json
        .and_then(|text| script_config_path(tool, text))
        .map(PathBuf::from)
        .filter(|path| root.join(path).is_file());
    from_script.or_else(|| {
        config_file_names(tool)
            .iter()
            .map(PathBuf::from)
            .find(|name| root.join(name).is_file())
    })
}

fn detected(root: &Path, tool: Tool, package_json: Option<&str>) -> bool {
    find_config(root, tool, None, package_json).is_some()
        || package_json.is_some_and(|text| package_json_mentions(tool, text))
}

fn selected_tools(args: &MigrateArgs, root: &Path, package_json: Option<&str>) -> Result<Vec<Tool>, CliError> {
    let mut tools: Vec<Tool> = Vec::new();
    for from in &args.from {
        match tool_of(*from) {
            Some(tool) => {
                if !tools.contains(&tool) {
                    tools.push(tool);
                }
            }
            None => {
                for tool in Tool::ALL {
                    if !tools.contains(&tool) && detected(root, tool, package_json) {
                        tools.push(tool);
                    }
                }
            }
        }
    }
    if tools.is_empty() {
        return Err(CliError::new(
            Kind::Usage,
            format!(
                "nothing to migrate: {} has no sherif, syncpack or knip configuration, package.json key or script; \
                 name the tool with --from",
                slash(root)
            ),
        ));
    }
    if args.source.is_some() && tools.len() != 1 {
        return Err(CliError::new(
            Kind::Usage,
            "--source names one configuration file, so --from must name exactly one tool",
        ));
    }
    Ok(tools)
}

fn tool_input(
    args: &MigrateArgs,
    root: &Path,
    tool: Tool,
    package_json: Option<&Input>,
    patterns: &[String],
) -> Result<ToolInput, CliError> {
    let mut input = ToolInput::new(tool);
    input.package_json = package_json.cloned();
    input.workspace_patterns = patterns.to_vec();
    if tool == Tool::Sherif {
        return Ok(input);
    }
    let explicit = args.source.as_deref();
    let package_text = package_json.map(|file| file.text.as_str());
    if let Some(path) = find_config(root, tool, explicit, package_text) {
        let absolute = if path.is_absolute() {
            path.clone()
        } else if explicit.is_some() {
            std::path::absolute(&path).map_err(|error| CliError::io("cannot resolve", &path, &error))?
        } else {
            root.join(&path)
        };
        input.config = Some(Input::new(slash(&path), read_text(&absolute)?));
    }
    Ok(input)
}

/// The existing configuration the output would collide with.
fn existing_config(root: &Path, target: &Path, default_target: bool) -> Option<PathBuf> {
    if !default_target {
        return target.exists().then(|| target.to_path_buf());
    }
    CONFIG_FILE_NAMES
        .iter()
        .map(|name| root.join(name))
        .find(|path| path.exists())
}

struct Plan {
    /// Where the configuration goes, or `None` for stdout.
    target: Option<PathBuf>,
    toml: String,
    kept_existing: Vec<String>,
}

fn plan_output(args: &MigrateArgs, root: &Path, migration: &Migration) -> Result<Plan, CliError> {
    let to_stdout = args.dry_run || args.output.as_deref() == Some(Path::new("-"));
    let default_target = args.output.is_none();
    let target = match &args.output {
        Some(path) if path != Path::new("-") => {
            std::path::absolute(path).map_err(|error| CliError::io("cannot resolve", path, &error))?
        }
        _ => root.join(CONFIG_FILE_NAMES[0]),
    };
    let existing = existing_config(root, &target, default_target);
    let mut toml = migration.toml.clone();
    let mut kept_existing = Vec::new();
    let mut destination = target;
    if let Some(existing) = &existing
        && args.merge
    {
        let text = read_text(existing)?;
        let merged = migration.merge_into(&text).map_err(migrate_error)?;
        kept_existing = merged.kept_existing;
        toml = merged.toml;
        destination.clone_from(existing);
    } else if let Some(existing) = &existing {
        if !args.force && !to_stdout {
            return Err(CliError {
                path: Some(slash(existing)),
                ..CliError::new(
                    Kind::Config,
                    format!(
                        "{} already exists; pass --merge to combine the migration with it, --force to replace it, or --output to write elsewhere",
                        existing.display()
                    ),
                )
            });
        }
        destination.clone_from(existing);
    }
    Ok(Plan {
        target: (!to_stdout).then_some(destination),
        toml,
        kept_existing,
    })
}

fn execute(args: &MigrateArgs, report_format: RulesFormat) -> Result<u8, CliError> {
    let root = std::path::absolute(&args.path).map_err(|error| CliError::io("cannot resolve", &args.path, &error))?;
    if !root.is_dir() {
        return Err(CliError::new(
            Kind::Io,
            format!("{} is not a directory", root.display()),
        ));
    }
    let package_path = root.join("package.json");
    let package_text = read_optional(&package_path)?;
    let tools = selected_tools(args, &root, package_text.as_deref())?;
    let package_input = package_text
        .as_ref()
        .map(|text| Input::new("package.json", text.clone()));
    let patterns = workspace_patterns(&root, package_text.as_deref());
    let inputs = tools
        .iter()
        .map(|tool| tool_input(args, &root, *tool, package_input.as_ref(), &patterns))
        .collect::<Result<Vec<ToolInput>, CliError>>()?;
    let profile = match args.profile {
        MigrateProfile::Exact => Profile::Exact,
        MigrateProfile::Full => Profile::Full,
    };
    let options = Options::new(profile)
        .with_known_rules(known_rules()?)
        .with_strict_hints(args.strict_hints);
    let migration = migrate(&inputs, &options).map_err(migrate_error)?;
    let plan = plan_output(args, &root, &migration)?;
    if let Some(target) = &plan.target {
        write_atomic(target, plan.toml.as_bytes()).map_err(|error| CliError::io("cannot write", target, &error))?;
    }
    report(args, report_format, &migration, &plan)?;
    Ok(if args.strict && migration.needs_human() {
        NEEDS_HUMAN
    } else {
        CLEAN
    })
}

fn report(args: &MigrateArgs, format: RulesFormat, migration: &Migration, plan: &Plan) -> Result<(), CliError> {
    match format {
        RulesFormat::Json => {
            let mut document = match migration.report() {
                Value::Object(map) => map,
                other => Map::from_iter([("report".to_owned(), other)]),
            };
            document.insert("skif".to_owned(), json!(env!("CARGO_PKG_VERSION")));
            document.insert(
                "written".to_owned(),
                plan.target
                    .as_deref()
                    .map_or(Value::Null, |target| json!(slash(target))),
            );
            document.insert("dry-run".to_owned(), json!(args.dry_run));
            if plan.target.is_none() {
                document.insert("toml".to_owned(), json!(plan.toml));
            }
            document.insert("kept-existing".to_owned(), json!(plan.kept_existing));
            let mut text = serde_json::to_string_pretty(&Value::Object(document))
                .map_err(|error| CliError::new(Kind::Internal, error.to_string()))?;
            text.push('\n');
            write_stdout(text.as_bytes())
        }
        RulesFormat::Human => {
            let mut summary = String::new();
            if let Some(target) = &plan.target {
                let _ = writeln!(summary, "Wrote {}\n", slash(target));
            }
            summary.push_str(&migration.summary());
            if !plan.kept_existing.is_empty() {
                let _ = writeln!(summary, "Kept the existing value of: {}", plan.kept_existing.join(", "));
            }
            if plan.target.is_some() {
                write_stdout(summary.as_bytes())
            } else {
                write_stdout(plan.toml.as_bytes())?;
                stderr_line(summary.trim_end());
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_packages_of_a_pnpm_workspace_are_read() {
        let text =
            "# comment\npackages:\n  - 'apps/*'\n  - \"packages/*\"\n  - '!**/test/**'\ncatalog:\n  react: ^18\n";
        assert_eq!(pnpm_packages(text), ["apps/*", "packages/*", "!**/test/**"]);
    }

    #[test]
    fn a_trailing_comment_is_not_part_of_the_pattern() {
        let text = "packages:\n  - 'apps/*' # the apps\n  - \"packages/*\"  # libraries\n  - tools/cli # plain\n";
        assert_eq!(pnpm_packages(text), ["apps/*", "packages/*", "tools/cli"]);
    }

    #[test]
    fn a_flow_style_packages_list_is_read() {
        let text = "packages: [\"apps/*\", 'packages/*', tools/cli]\ncatalog:\n  react: ^18\n";
        assert_eq!(pnpm_packages(text), ["apps/*", "packages/*", "tools/cli"]);
    }

    #[test]
    fn a_pnpm_workspace_without_packages_has_none() {
        assert_eq!(pnpm_packages("catalog:\n  - a\n"), Vec::<String>::new());
    }
}
