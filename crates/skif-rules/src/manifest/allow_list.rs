//! The `allow` option of the rules that report files a project may keep on purpose.

use std::path::{Component, Path};

use serde::Deserialize;
use skif_config::IgnoreItem;
use skif_core::{Location, RuleMeta, diagnostic, slash};
use skif_lint::{Emitter, LintContext, OptionsError, parse_options};

/// Options of `stray-lockfile` and `nested-workspace-root`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct AllowListOptions {
    /// Files, relative to the workspace root, that are kept on purpose and not reported: a path, or a table of `name`
    /// (the path) and `reason`, as the entries of the `[js]` ignore lists take.
    pub allow: Vec<IgnoreItem>,
}

/// The JSON Schema of [`AllowListOptions`].
#[must_use]
pub fn schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "allow": {
                "type": "array",
                "default": [],
                "items": {
                    "anyOf": [
                        { "type": "string" },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["name"],
                            "properties": { "name": { "type": "string" }, "reason": { "type": "string" } },
                        },
                    ],
                },
            },
        },
    })
}

/// Parses the options and rejects a blank `reason`, as the `[js]` ignore lists do.
///
/// # Errors
/// [`OptionsError`] for options that do not parse, or naming the first entry with a blank `reason`.
pub fn validate(options: &toml::Table) -> Result<(), OptionsError> {
    let parsed = parse_options::<AllowListOptions>(options)?;
    match parsed
        .allow
        .iter()
        .position(|item| item.reason.as_deref().is_some_and(|reason| reason.trim().is_empty()))
    {
        Some(index) => Err(OptionsError::for_key(
            "allow",
            format!("allow[{index}]: `reason` is empty; say why or remove it"),
        )),
        None => Ok(()),
    }
}

/// The path an entry names, `/`-separated and without a leading `./`.
fn normalized(name: &str) -> String {
    let name = name.replace('\\', "/");
    name.strip_prefix("./").unwrap_or(&name).to_owned()
}

/// The `allow` entries of one rule, as configured.
pub struct AllowList {
    entries: Vec<IgnoreItem>,
}

impl AllowList {
    /// The entries configured for `meta`; none when the options do not parse (validation reported that already).
    #[must_use]
    pub fn of(cx: &LintContext<'_>, meta: &RuleMeta) -> Self {
        let entries = cx
            .config
            .rule(meta.id)
            .and_then(|rule| parse_options::<AllowListOptions>(&rule.options).ok())
            .unwrap_or_default()
            .allow;
        Self { entries }
    }

    /// Whether an entry names `path`, a workspace-relative file.
    #[must_use]
    pub fn allows(&self, path: &Path) -> bool {
        let path = slash(path);
        self.entries.iter().any(|entry| normalized(&entry.name) == path)
    }

    /// Reports every entry that names none of `found`, the files the rule would report without the list, as stale.
    ///
    /// Nothing is reported when the configuration was inherited from a directory above the root: such a file also
    /// configures the other roots below it, so an entry naming the file of one of them is not stale here.
    pub fn report_stale(
        &self,
        cx: &LintContext<'_>,
        meta: &'static RuleMeta,
        found: &[&Path],
        noun: &str,
        out: &mut Emitter<'_>,
    ) {
        let inherited = cx
            .config_path
            .is_some_and(|path| path.components().next() == Some(Component::ParentDir));
        if inherited {
            return;
        }
        let found: Vec<String> = found.iter().map(|path| slash(path)).collect();
        for entry in &self.entries {
            if found.contains(&normalized(&entry.name)) {
                continue;
            }
            out.emit(
                diagnostic(
                    meta,
                    format!("`{}` in `rules.{}.allow` names no {noun}", entry.name, meta.id),
                    Location::new(cx.config_file().to_path_buf(), None),
                )
                .help("remove the entry, or correct the path if the file moved")
                .build(),
            );
        }
    }
}
