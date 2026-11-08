//! Static validation of the `[js]` configuration: globs and regular expressions that do not compile (plugin ids that
//! do not exist are rejected by [`Config::validate_plugins`]). Settings that depend on the workspace (a `[js.workspaces]` key that selects no package, an
//! ignore entry that matches nothing) are reported by the `stale-js-config` rule instead.

use globset::Glob;
use indexmap::IndexMap;
use skif_config::{Config, ConfigError, PluginConfig, PluginOptions};

use crate::listed;

type Problem = Result<(), ConfigError>;

fn invalid(message: String) -> Problem {
    Err(ConfigError::Invalid { path: None, message })
}

/// Whether `pattern` compiles as a glob, with the `!` negation and production markers and the empty alternatives
/// (`{,a}`) the plugin dialect allows.
fn glob_problem(pattern: &str) -> Option<String> {
    let body = pattern.strip_prefix('!').unwrap_or(pattern);
    let body = body.strip_suffix('!').unwrap_or(body);
    skif_js_plugins::glob::expand_empty_alternatives(body)
        .iter()
        .find_map(|variant| Glob::new(variant).err().map(|err| err.kind().to_string()))
}

/// `[js] tsconfig` normalised to a `/`-separated path relative to the workspace root, or `None` when it is empty,
/// absolute or leaves the root. Every reader of the setting goes through this, so they agree on which file it names.
pub(crate) fn relative_tsconfig(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() || path.starts_with(['/', '\\']) || std::path::Path::new(path).is_absolute() {
        return None;
    }
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split(['/', '\\']) {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            other => segments.push(other),
        }
    }
    // A Windows drive (`C:`) is absolute there and never a file name here.
    if segments.is_empty() || segments[0].contains(':') {
        return None;
    }
    Some(segments.join("/"))
}

fn globs(key: &str, patterns: &[String]) -> Problem {
    for (index, pattern) in patterns.iter().enumerate() {
        if let Some(message) = glob_problem(pattern) {
            return invalid(format!("{key}[{index}]: invalid glob `{pattern}`: {message}"));
        }
    }
    Ok(())
}

/// Entries of an ignore list: exact names, globs or regular expressions, optionally ending in `!`.
fn names(key: &str, entries: &[String]) -> Problem {
    for (index, entry) in entries.iter().enumerate() {
        let body = entry.strip_suffix('!').unwrap_or(entry);
        if let Err(message) = listed::check(body) {
            return invalid(format!("{key}[{index}]: invalid pattern `{entry}`: {message}"));
        }
    }
    Ok(())
}

/// The globs of the options of the plugins in `table`.
fn plugin_globs(key: &str, table: &IndexMap<String, PluginConfig>) -> Problem {
    for (id, plugin) in table {
        if let PluginConfig::Options(PluginOptions {
            config, entry, project, ..
        }) = plugin
        {
            globs(&format!("{key}.{id}.config"), config)?;
            globs(&format!("{key}.{id}.entry"), entry)?;
            globs(&format!("{key}.{id}.project"), project)?;
        }
    }
    Ok(())
}

/// Validates every `[js]` setting that can be checked without the workspace.
///
/// # Errors
/// [`ConfigError::Invalid`] naming the first key whose value is not a valid glob, not a valid regular expression, or
/// is empty where a value is required.
pub(crate) fn validate(config: &Config) -> Problem {
    let js = &config.js;
    globs("js.entry", &js.entry)?;
    globs("js.project", &js.project)?;
    globs("js.ignore", &js.ignore)?;
    globs("js.extend-ignore", &js.extend_ignore)?;
    names("js.ignore-dependencies", &js.ignore_dependencies)?;
    names("js.extend-ignore-dependencies", &js.extend_ignore_dependencies)?;
    names("js.ignore-binaries", &js.ignore_binaries)?;
    names("js.extend-ignore-binaries", &js.extend_ignore_binaries)?;
    names("js.ignore-unresolved", &js.ignore_unresolved)?;
    names("js.extend-ignore-unresolved", &js.extend_ignore_unresolved)?;
    names("js.ignore-members", &js.ignore_members)?;
    names("js.ignore-workspaces", &js.ignore_workspaces)?;
    plugin_globs("js.plugins", &js.plugins)?;
    if let Some(path) = &js.tsconfig
        && relative_tsconfig(path).is_none()
    {
        return invalid(format!(
            "js.tsconfig: `{path}` is not a path relative to the workspace root that stays inside it"
        ));
    }
    for (dir, workspace) in &js.workspaces {
        let key = format!("js.workspaces.\"{dir}\"");
        if let Some(message) = glob_problem(dir) {
            return invalid(format!("{key}: invalid workspace glob: {message}"));
        }
        globs(&format!("{key}.entry"), &workspace.entry)?;
        globs(&format!("{key}.project"), &workspace.project)?;
        globs(&format!("{key}.ignore"), &workspace.ignore)?;
        names(&format!("{key}.ignore-dependencies"), &workspace.ignore_dependencies)?;
        names(&format!("{key}.ignore-binaries"), &workspace.ignore_binaries)?;
        names(&format!("{key}.ignore-unresolved"), &workspace.ignore_unresolved)?;
        names(&format!("{key}.ignore-members"), &workspace.ignore_members)?;
        for (alias, targets) in &workspace.paths {
            if alias.trim().is_empty() || targets.is_empty() || targets.iter().any(|target| target.trim().is_empty()) {
                return invalid(format!(
                    "{key}.paths.\"{alias}\": an alias needs a name and at least one non-empty target"
                ));
            }
        }
        plugin_globs(&format!("{key}.plugins"), &workspace.plugins)?;
    }
    for (index, scoped) in config.overrides.iter().enumerate() {
        let key = format!("overrides[{index}].js");
        let scoped = &scoped.js;
        globs(&format!("{key}.entry"), scoped.entry.as_deref().unwrap_or_default())?;
        globs(&format!("{key}.project"), scoped.project.as_deref().unwrap_or_default())?;
        globs(&format!("{key}.ignore"), scoped.ignore.as_deref().unwrap_or_default())?;
        globs(&format!("{key}.extend-ignore"), &scoped.extend_ignore)?;
        names(
            &format!("{key}.ignore-dependencies"),
            scoped.ignore_dependencies.as_deref().unwrap_or_default(),
        )?;
        names(
            &format!("{key}.extend-ignore-dependencies"),
            &scoped.extend_ignore_dependencies,
        )?;
        names(
            &format!("{key}.ignore-binaries"),
            scoped.ignore_binaries.as_deref().unwrap_or_default(),
        )?;
        names(&format!("{key}.extend-ignore-binaries"), &scoped.extend_ignore_binaries)?;
        names(
            &format!("{key}.ignore-unresolved"),
            scoped.ignore_unresolved.as_deref().unwrap_or_default(),
        )?;
        names(
            &format!("{key}.extend-ignore-unresolved"),
            &scoped.extend_ignore_unresolved,
        )?;
        plugin_globs(&format!("{key}.plugins"), &scoped.plugins)?;
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn check(toml: &str) -> String {
        let config = Config::from_toml_str(toml).unwrap();
        validate(&config).unwrap_err().to_string()
    }

    #[test]
    fn a_valid_configuration_passes() {
        let config = Config::from_toml_str(
            "[js]\nentry = [\"src/{,sub/}main.ts!\", \"!skip/**\"]\nignore-dependencies = [\"@types/*\", \"/^@org\\\\/.+$/\", \"x!\"]\nignore-unresolved = [\"#internal/*\"]\n[js.plugins]\nvitest = true\n[js.workspaces.\".\"]\nentry = [\"a.ts\"]\n[js.workspaces.\"packages/*\".paths]\n\"@/*\" = [\"./src/*\"]\n",
        )
        .unwrap();
        assert!(validate(&config).is_ok());
    }

    #[test]
    fn invalid_globs_are_reported_with_their_key() {
        let message = check("[js]\nentry = [\"ok.ts\", \"src/[bad\"]\n");
        assert!(message.contains("js.entry[1]: invalid glob `src/[bad`: "), "{message}");
        assert!(check("[js.workspaces.\"apps/[x\"]\nentry = []\n").contains("js.workspaces.\"apps/[x\""));
        assert!(
            check("[[overrides]]\npackages = [\"a\"]\n[overrides.js]\nignore = [\"[\"]\n")
                .contains("overrides[0].js.ignore[0]")
        );
    }

    #[test]
    fn invalid_ignore_patterns_are_reported() {
        let message = check("[js]\nignore-dependencies = [\"/(/\"]\n");
        assert!(
            message.contains("js.ignore-dependencies[0]: invalid pattern `/(/`"),
            "{message}"
        );
        assert!(check("[js]\nignore-unresolved = [\"[bad\"]\n").contains("js.ignore-unresolved[0]"));
        assert!(check("[js]\nignore-workspaces = [\"a(\"]\n").contains("js.ignore-workspaces[0]"));
    }

    #[test]
    fn tsconfig_paths_stay_inside_the_workspace_root() {
        assert_eq!(
            relative_tsconfig("./config/../tsconfig.app.json").as_deref(),
            Some("tsconfig.app.json")
        );
        assert_eq!(
            relative_tsconfig("config\\tsconfig.json").as_deref(),
            Some("config/tsconfig.json")
        );
        for bad in [
            "",
            "  ",
            ".",
            "/abs/tsconfig.json",
            "\\abs.json",
            "C:/x.json",
            "../tsconfig.json",
            "a/../../b.json",
        ] {
            assert_eq!(relative_tsconfig(bad), None, "{bad}");
        }
        let message = check("[js]\ntsconfig = \"../tsconfig.json\"\n");
        assert!(message.contains("js.tsconfig: `../tsconfig.json`"), "{message}");
        assert!(check("[js]\ntsconfig = \"/etc/tsconfig.json\"\n").contains("js.tsconfig"));
    }

    #[test]
    fn empty_path_aliases_are_reported() {
        assert!(check("[js.workspaces.\"a\".paths]\n\"@/*\" = []\n").contains("paths.\"@/*\""));
    }
}
