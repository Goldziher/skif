//! Package files: a `skif.toml` in a workspace member directory, applied as an override scoped to that package.

use std::path::Path;

use toml::{Table, Value};

use crate::error::ConfigError;
use crate::layer::{ParsedFile, ROOT_SCOPED_JS};
use crate::model::Override;

/// Reads a package file as the `[[overrides]]` block `packages = ["./"]` with the same settings.
///
/// A package file uses the top-level spelling of the keys an override has: `[rules]` (levels only), `[[ignore]]`,
/// `[js]` and `[lint] max-level`. Anything else is root-scoped and rejected with the key and its position.
///
/// The returned override has no [`scope`](Override::scope) yet; the caller anchors it at the file's directory.
pub(crate) fn to_override(file: &ParsedFile, path: &Path) -> Result<Override, ConfigError> {
    let reject = |key: String, hint: String| {
        let position = file.position(&key);
        ConfigError::scope(key, Some(path), position, hint)
    };
    let mut body = Table::new();
    body.insert(
        "packages".to_owned(),
        Value::Array(vec![Value::String("./".to_owned())]),
    );
    for (key, value) in &file.table {
        match (key.as_str(), value) {
            ("rules", Value::Table(rules)) => {
                for (id, rule) in rules {
                    let Value::Table(options) = rule else { continue };
                    if let Some(option) = options
                        .keys()
                        .find(|option| !matches!(option.as_str(), "level" | "severity"))
                    {
                        return Err(reject(
                            format!("rules.{id}.{option}"),
                            "rule options are root-scoped; a package file can only set rule levels, so set the options in `[rules]` of the root's configuration file".to_owned(),
                        ));
                    }
                }
                body.insert(key.clone(), value.clone());
            }
            ("js", Value::Table(js)) => {
                if let Some(name) = js.keys().find(|name| ROOT_SCOPED_JS.contains(&name.as_str())) {
                    return Err(reject(
                        format!("js.{name}"),
                        format!(
                            "`js.{name}` applies to the whole root; set it in `[js]` of the root's configuration file"
                        ),
                    ));
                }
                body.insert(key.clone(), value.clone());
            }
            ("lint", Value::Table(lint)) => {
                for (name, setting) in lint {
                    if name == "max-level" {
                        body.insert(name.clone(), setting.clone());
                    } else {
                        return Err(reject(
                            format!("lint.{name}"),
                            "a package file can cap severity with `max-level`; other `[lint]` settings apply to the whole root".to_owned(),
                        ));
                    }
                }
            }
            ("ignore", _) => {
                body.insert(key.clone(), value.clone());
            }
            (other, _) => {
                return Err(reject(
                    other.to_owned(),
                    match other {
                        "root" | "extends" | "reset" | "overrides" => {
                            format!("`{other}` is not supported in a package file")
                        }
                        _ => format!(
                            "`{other}` applies to the whole root; set it at the top level of the root's configuration file"
                        ),
                    },
                ));
            }
        }
    }
    let scoped: Override = Value::Table(body)
        .try_into()
        .map_err(|error: toml::de::Error| ConfigError::invalid(Some(path), error.message()))?;
    if let Some(message) = scoped.inconsistency() {
        return Err(ConfigError::invalid(Some(path), message));
    }
    Ok(scoped)
}
