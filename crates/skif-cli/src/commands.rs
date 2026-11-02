//! The `rules`, `schema` and `init` subcommands.

use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};

use serde::Serialize;
use serde_json::json;
use skif::{CONFIG_FILE_NAMES, Config, Linter, RuleMeta};

use crate::args::{Format, InitArgs, RulesArgs, RulesFormat};
use crate::error::{CliError, Kind, display};
use crate::output::{emit_error, write_stdout};

const CLEAN: u8 = 0;
const COULD_NOT_RUN: u8 = 2;

const STARTER_CONFIG: &str = "\
#:schema https://raw.githubusercontent.com/Goldziher/skif/main/schema/skif.schema.json

# skif configuration. Every key is optional; the values below are the defaults.

[lint]
# Exit 1 when only warnings were reported (the same as `--max-warnings 0`).
fail-on-warnings = false

[policy]
# The version that alignment fixes target: \"highest\" or \"lowest\".
prefer = \"highest\"

# [workspace]
# ignore-packages = [\"@scope/legacy-*\"]

# [rules]
# unsorted-dependencies = \"off\"
";

fn finish(result: Result<(), CliError>, format: Format) -> u8 {
    match result {
        Ok(()) => CLEAN,
        Err(error) => {
            emit_error(&error, format);
            COULD_NOT_RUN
        }
    }
}

fn label<T: Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn rules_table(metas: &[&'static RuleMeta]) -> String {
    let width = metas
        .iter()
        .map(|meta| meta.id.len())
        .max()
        .unwrap_or(0)
        .max("RULE".len());
    let mut table = format!(
        "{:<width$}  {:<9}  {:<7}  {:<9}  SUMMARY\n",
        "RULE", "CATEGORY", "DEFAULT", "FIXABLE"
    );
    for meta in metas {
        let default = meta
            .default
            .map_or_else(|| "off".to_owned(), |severity| label(&severity));
        let _ = writeln!(
            table,
            "{:<width$}  {:<9}  {:<7}  {:<9}  {}",
            meta.id,
            label(&meta.category),
            default,
            label(&meta.fixable),
            meta.summary
        );
    }
    table
}

fn list_rules(format: RulesFormat) -> Result<(), CliError> {
    let linter = Linter::new(&Config::default())?;
    let metas: Vec<&'static RuleMeta> = linter.rules().collect();
    let text = match format {
        RulesFormat::Human => rules_table(&metas),
        RulesFormat::Json => {
            let mut text = serde_json::to_string_pretty(&json!({
                "version": 1,
                "skif": env!("CARGO_PKG_VERSION"),
                "rules": metas,
            }))
            .map_err(|error| CliError::new(Kind::Io, error.to_string()))?;
            text.push('\n');
            text
        }
    };
    write_stdout(text.as_bytes())
}

pub fn rules(args: &RulesArgs) -> u8 {
    finish(list_rules(args.format), args.format.into())
}

pub fn schema() -> u8 {
    finish(write_stdout(skif::config_schema().as_bytes()), Format::Human)
}

fn write_starter(args: &InitArgs) -> Result<(), CliError> {
    let dir = std::path::absolute(&args.path).map_err(|error| CliError::io("cannot resolve", &args.path, &error))?;
    if !dir.is_dir() {
        return Err(CliError::new(Kind::Io, format!("{} is not a directory", dir.display())));
    }
    if let Some(existing) = CONFIG_FILE_NAMES
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.exists())
    {
        return Err(CliError {
            path: Some(display(&existing)),
            ..CliError::new(
                Kind::Config,
                format!("{} already exists; refusing to overwrite it", existing.display()),
            )
        });
    }
    let target = dir.join(CONFIG_FILE_NAMES[0]);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(|error| match error.kind() {
            ErrorKind::AlreadyExists => CliError::new(
                Kind::Config,
                format!("{} already exists; refusing to overwrite it", target.display()),
            ),
            _ => CliError::io("cannot create", &target, &error),
        })?;
    file.write_all(STARTER_CONFIG.as_bytes())
        .map_err(|error| CliError::io("cannot write", &target, &error))?;
    write_stdout(format!("Created {}\n", crate::error::display(&target)).as_bytes())
}

pub fn init(args: &InitArgs) -> u8 {
    finish(write_starter(args), Format::Human)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starter_config_is_valid_configuration() {
        let config = Config::from_toml_str(STARTER_CONFIG).unwrap();
        assert_eq!(config, Config::default());
    }
}
