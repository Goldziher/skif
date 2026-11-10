//! The `allow` option of the rules that report files a project may keep on purpose.

use std::path::{Component, Path};

use serde::Deserialize;
use skif_config::{ConfigError, IgnoreItem, LOCAL, Selector};
use skif_core::{Location, RuleMeta, diagnostic, slash};
use skif_lint::{Emitter, LintContext, OptionsError, parse_options};

/// Options of `stray-lockfile` and `nested-workspace-root`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct AllowListOptions {
    /// Files, relative to the workspace root, that are kept on purpose and not reported: a path, matched literally
    /// first, or a glob (`packages/*/package-lock.json`, where `*` also crosses `/`), or a table of `name` (the path
    /// or glob) and `reason`, as the entries of the `[js]` ignore lists take.
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

/// Parses the options and rejects a blank `reason`, as the `[js]` ignore lists do, and an entry that is not a valid
/// glob, starts with `!` or is the package selector `$LOCAL`.
///
/// # Errors
/// [`OptionsError`] for options that do not parse, or naming the first entry with a blank `reason` or a bad pattern.
pub fn validate(options: &toml::Table) -> Result<(), OptionsError> {
    let parsed = parse_options::<AllowListOptions>(options)?;
    for (index, item) in parsed.allow.iter().enumerate() {
        if item.reason.as_deref().is_some_and(|reason| reason.trim().is_empty()) {
            return Err(OptionsError::for_key(
                "allow",
                format!("allow[{index}]: `reason` is empty; say why or remove it"),
            ));
        }
        compile(&item.name).map_err(|message| OptionsError::for_key("allow", format!("allow[{index}]: {message}")))?;
    }
    Ok(())
}

/// The path or glob an entry names, `/`-separated and without a leading `./`.
fn normalized(name: &str) -> String {
    let name = name.replace('\\', "/");
    name.strip_prefix("./").unwrap_or(&name).to_owned()
}

/// The selector of the entry `name`; an exclusion would allow every other file, and `$LOCAL` selects packages, not
/// files, so both are refused.
fn compile(name: &str) -> Result<Selector, String> {
    if name.starts_with('!') {
        return Err(format!("`{name}` cannot exclude; list the files to allow"));
    }
    if normalized(name) == LOCAL {
        return Err(format!(
            "`{name}` selects workspace packages, not files; list the files to allow"
        ));
    }
    Selector::new([normalized(name)]).map_err(|error| match error {
        ConfigError::Invalid { message, .. } => message,
        other => other.to_string(),
    })
}

/// One `allow` entry: as configured, its normalized path and its compiled glob.
struct Entry {
    item: IgnoreItem,
    path: String,
    glob: Selector,
}

impl Entry {
    /// Whether the entry names `path`, a `/`-separated workspace-relative file: literally, so a directory such as
    /// `app/[id]` needs no escaping, or else as a glob.
    fn matches(&self, path: &str) -> bool {
        self.path == path || self.glob.matches(path, false)
    }
}

/// The `allow` entries of one rule, as configured, each with its pattern compiled.
pub struct AllowList {
    entries: Vec<Entry>,
}

impl AllowList {
    /// The entries configured for `meta`; none when the options do not parse, and no entry whose pattern does not
    /// compile (validation reported both already).
    #[must_use]
    pub fn of(cx: &LintContext<'_>, meta: &RuleMeta) -> Self {
        let entries = cx
            .config
            .rule(meta.id)
            .and_then(|rule| parse_options::<AllowListOptions>(&rule.options).ok())
            .unwrap_or_default()
            .allow
            .into_iter()
            .filter_map(|item| {
                let glob = compile(&item.name).ok()?;
                let path = normalized(&item.name);
                Some(Entry { item, path, glob })
            })
            .collect();
        Self { entries }
    }

    /// Whether an entry matches `path`, a workspace-relative file.
    #[must_use]
    pub fn allows(&self, path: &Path) -> bool {
        let path = slash(path);
        self.entries.iter().any(|entry| entry.matches(&path))
    }

    /// Reports every entry that matches none of `found`, the files the rule would report without the list, as stale.
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
            if found.iter().any(|path| entry.matches(path)) {
                continue;
            }
            out.emit(
                diagnostic(
                    meta,
                    format!("`{}` in `rules.{}.allow` names no {noun}", entry.item.name, meta.id),
                    Location::new(cx.config_file().to_path_buf(), None),
                )
                .help("remove the entry, or correct the path if the file moved")
                .build(),
            );
        }
    }
}
