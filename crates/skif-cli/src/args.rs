//! The command line, as specified by ADR 0016.

use std::path::PathBuf;

use clap::{ColorChoice, Parser, Subcommand, ValueEnum};
use skif::Level;

/// Fast dependency linter for JavaScript and TypeScript monorepos.
#[derive(Debug, Parser)]
#[command(name = "skif", version, about, args_conflicts_with_subcommands = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
    #[command(flatten)]
    pub lint: LintArgs,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// List every rule with its category, default level and fixability.
    Rules(RulesArgs),
    /// Print the JSON Schema of `skif.toml`.
    Schema,
    /// Write a starter `skif.toml`; refuses to overwrite an existing configuration.
    Init(InitArgs),
}

#[derive(Debug, clap::Args)]
pub struct RulesArgs {
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub format: Format,
}

#[derive(Debug, clap::Args)]
pub struct InitArgs {
    /// Directory to write `skif.toml` into.
    #[arg(default_value = ".")]
    pub path: PathBuf,
}

/// Output format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Grouped, optionally coloured text.
    #[default]
    Human,
    /// One JSON document on stdout.
    Json,
}

/// Which version a version alignment fix targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Select {
    Highest,
    Lowest,
}

#[derive(Debug, clap::Args)]
pub struct LintArgs {
    /// A path inside the monorepo; the workspace root is the nearest ancestor that declares a workspace.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Output format [default: human].
    #[arg(long, value_enum)]
    pub format: Option<Format>,

    /// Write the rendering to this file instead of stdout.
    #[arg(long, value_name = "FILE", requires = "format")]
    pub output: Option<PathBuf>,

    /// Apply safe fixes, then lint again and run the package manager's install command.
    #[arg(long)]
    pub fix: bool,

    /// Apply safe and unsafe fixes; implies --fix.
    #[arg(long)]
    pub fix_unsafe: bool,

    /// Which version a version alignment fix targets, overriding `[policy] prefer` for this run.
    #[arg(long, value_enum)]
    pub select: Option<Select>,

    /// Do not run the package manager after fixing.
    #[arg(long)]
    pub no_install: bool,

    /// Exit 1 when there are more than N warnings.
    #[arg(long, value_name = "N")]
    pub max_warnings: Option<usize>,

    /// Same as --max-warnings 0.
    #[arg(long)]
    pub fail_on_warnings: bool,

    /// Set a rule's level, as ID=off|warn|error. Repeatable.
    #[arg(long = "rule", value_name = "ID=LEVEL", value_parser = parse_rule_level)]
    pub rules: Vec<(String, Level)>,

    /// Turn a rule off. Repeatable.
    #[arg(long, value_name = "ID")]
    pub ignore_rule: Vec<String>,

    /// Ignore a package by name or glob in every rule. Repeatable.
    #[arg(long, value_name = "NAME")]
    pub ignore_package: Vec<String>,

    /// Ignore a dependency in every rule. Repeatable.
    #[arg(long, value_name = "NAME")]
    pub ignore_dependency: Vec<String>,

    /// Ignore one rule for one package, as RULE:PACKAGE. Repeatable.
    #[arg(long, value_name = "RULE:PACKAGE", value_parser = parse_rule_package)]
    pub ignore_rule_for_package: Vec<(String, String)>,

    /// Skip JavaScript analysis.
    #[arg(long)]
    pub no_js: bool,

    /// Only analyse production code in JavaScript analysis.
    #[arg(long)]
    pub production: bool,

    /// When to colour human output.
    #[arg(long, value_enum, default_value_t = ColorChoice::Auto)]
    pub color: ColorChoice,
}

impl LintArgs {
    /// Whether any fixing flag was given.
    pub const fn fixing(&self) -> bool {
        self.fix || self.fix_unsafe
    }

    pub fn format(&self) -> Format {
        self.format.unwrap_or_default()
    }
}

fn parse_rule_level(value: &str) -> Result<(String, Level), String> {
    let (rule, level) = value
        .split_once('=')
        .filter(|(rule, _)| !rule.is_empty())
        .ok_or_else(|| "expected `ID=LEVEL`, for example `unsorted-dependencies=off`".to_owned())?;
    let level = match level {
        "off" => Level::Off,
        "warn" => Level::Warn,
        "error" => Level::Error,
        other => return Err(format!("unknown level `{other}`; expected off, warn or error")),
    };
    Ok((rule.to_owned(), level))
}

fn parse_rule_package(value: &str) -> Result<(String, String), String> {
    match value.split_once(':') {
        Some((rule, package)) if !rule.is_empty() && !package.is_empty() => Ok((rule.to_owned(), package.to_owned())),
        _ => Err("expected `RULE:PACKAGE`".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_levels_parse() {
        assert_eq!(parse_rule_level("a-b=off"), Ok(("a-b".to_owned(), Level::Off)));
        assert!(parse_rule_level("a-b=info").is_err());
        assert!(parse_rule_level("=warn").is_err());
        assert!(parse_rule_level("nolevel").is_err());
    }

    #[test]
    fn rule_package_pairs_parse() {
        assert_eq!(
            parse_rule_package("r:@scope/p"),
            Ok(("r".to_owned(), "@scope/p".to_owned()))
        );
        assert!(parse_rule_package("r:").is_err());
        assert!(parse_rule_package("r").is_err());
    }

    #[test]
    fn command_line_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
