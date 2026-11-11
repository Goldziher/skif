//! Nx: executors, plugins and generators are packages; `run-commands` targets are scripts, and target options name
//! build entries and the configs of other plugins.

use skif_js_syntax::ConfigValue;

use crate::config_load::load_config;
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginId, PluginInput, PluginSpec};
use crate::plugins::{cypress, eslint, jest, playwright, storybook, typescript, vite, webpack};
use crate::support::{join_rel, package_name, path_within, relative_path, strings};

/// The `nx` plugin.
pub struct Nx;

/// The plugin's identifier.
pub const ID: PluginId = "nx";

static SPEC: PluginSpec = PluginSpec {
    id: ID,
    enablers: &[
        Enabler::Dependency("nx"),
        Enabler::DependencyPrefix("@nrwl/"),
        Enabler::DependencyPrefix("@nx/"),
    ],
    // The `project.json` of each project the package owns is read as well (ADR 0109).
    config: &["nx.json", "project.json"],
    package_json_key: Some("nx"),
    binaries: &["nx"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Nx {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if config.file_name() == "nx.json" {
            if let Some(ConfigValue::Object(defaults)) = config.value.get("targetDefaults") {
                out.extend(
                    defaults
                        .iter()
                        .filter(|(key, _)| key.starts_with('@') && key.contains(':'))
                        .filter_map(|(key, _)| executor_package(key)),
                );
            }
            if let Some(ConfigValue::Array(plugins)) = config.value.get("plugins") {
                out.extend(
                    plugins
                        .iter()
                        .filter_map(|plugin| match plugin {
                            ConfigValue::String(name) => Some(name.as_str()),
                            other => other.get("plugin").and_then(ConfigValue::as_str),
                        })
                        .filter_map(package_name)
                        .map(PluginInput::Dependency),
                );
            }
            if let Some(ConfigValue::Object(generators)) = config.value.get("generators") {
                out.extend(generators.iter().filter_map(|(key, _)| executor_package(key)));
            }
        }
        if let Some(ConfigValue::Object(targets)) = config.value.get("targets") {
            let roots = Roots::new(cx, config.base);
            for (_, target) in targets {
                out.extend(target_inputs(target, &roots));
            }
        }
        out
    }
}

/// `@nx/vite:build` names the package `@nx/vite`; `./tools:build` names a local executor.
fn executor_package(executor: &str) -> Option<PluginInput> {
    let name = executor.split(':').next()?;
    if name.starts_with('.') {
        return None;
    }
    package_name(name).map(PluginInput::Dependency)
}

/// Where the paths of a project's targets are read from: Nx gives them relative to the workspace root, and the
/// package reading the `project.json` may lie below it.
struct Roots<'a> {
    /// The package directory, relative to the workspace root; `""` for the root package.
    package: &'a str,
    /// The project directory, relative to the workspace root.
    project: String,
}

impl<'a> Roots<'a> {
    /// The roots of the project in `dir`, relative to the package directory of `cx`, which lies at
    /// [`NxProjects::path`](crate::NxProjects::path) in the workspace (the root without [`PluginContext::nx`]).
    fn new(cx: &PluginContext<'a>, dir: &str) -> Self {
        let package = cx.nx.map_or("", |projects| projects.path.as_str());
        Self {
            package,
            project: join_rel(package, dir),
        }
    }

    /// `path`, a path an option gives relative to the workspace root, relative to the workspace root with its tokens
    /// replaced; `""` for the root itself. `None` when it is absolute, leaves the workspace or holds a token that is
    /// not known statically (`{options.outputPath}`).
    fn in_workspace(&self, path: &str) -> Option<String> {
        let path = join_rel("", &with_tokens(path.trim(), "", &self.project));
        (!path.starts_with('/') && !path.starts_with("..") && !path.contains(['{', '}', '$'])).then_some(path)
    }

    /// `path`, a path an option gives relative to the workspace root, relative to the package directory; `None` when
    /// [`Roots::in_workspace`] gives none, or it is the package directory or outside it.
    fn option_path(&self, path: &str) -> Option<String> {
        self.in_workspace(path)
            .and_then(|path| path_within(&path, self.package))
            .filter(|path| !path.is_empty())
    }
}

/// The paths the option `key` of the targets of the project in `dir` (relative to the package directory) gives, relative
/// to the package directory.
pub(crate) fn target_option_paths(cx: &PluginContext<'_>, dir: &str, key: &str) -> Vec<String> {
    let path = cx.dir.join(join_rel(dir, "project.json"));
    let Some(project) = cx
        .fs
        .read_to_string(&path)
        .ok()
        .and_then(|source| load_config(&path, &source))
    else {
        return Vec::new();
    };
    let Some(ConfigValue::Object(targets)) = project.get("targets") else {
        return Vec::new();
    };
    let roots = Roots::new(cx, dir);
    targets
        .iter()
        .filter_map(|(_, target)| target.get("options")?.get(key)?.as_str().map(str::to_owned))
        .filter_map(|value| roots.option_path(&value))
        .collect()
}

/// A target option that names a config of another plugin, relative to the workspace root.
struct LinkedOption {
    key: &'static str,
    plugin: PluginId,
    /// The Nx plugin whose executors give the option this meaning (`@nx/<name>` or `@nrwl/<name>`); `None` for any
    /// executor, as the name is the tool's own.
    nx_plugin: Option<&'static str>,
    /// The config files in the directory the option names; `None` when it names the file.
    in_dir: Option<&'static str>,
}

const fn linked(key: &'static str, plugin: PluginId, nx_plugin: Option<&'static str>) -> LinkedOption {
    LinkedOption {
        key,
        plugin,
        nx_plugin,
        in_dir: None,
    }
}

/// The options of the executors of `@nx/js`, `@nx/jest`, `@nx/webpack`, `@nx/cypress`, `@nx/eslint`, `@nx/vite`,
/// `@nx/playwright` and `@nx/storybook` that name a config.
const LINKED_OPTIONS: &[LinkedOption] = &[
    linked("tsConfig", typescript::ID, None),
    linked("jestConfig", jest::ID, None),
    linked("webpackConfig", webpack::ID, None),
    linked("cypressConfig", cypress::ID, None),
    linked("eslintConfig", eslint::ID, None),
    linked("configFile", vite::ID, Some("vite")),
    linked("config", playwright::ID, Some("playwright")),
    LinkedOption {
        in_dir: Some("main.{js,ts,mts,cjs,mjs}"),
        ..linked("configDir", storybook::ID, Some("storybook"))
    },
];

/// Whether the target's `executor` comes from the Nx plugin `name` (`@nx/vite:build` from `vite`).
fn runs_nx_plugin(executor: Option<&str>, name: &str) -> bool {
    executor
        .and_then(|executor| executor.split(':').next())
        .and_then(|package| package.strip_prefix("@nx/").or_else(|| package.strip_prefix("@nrwl/")))
        .is_some_and(|plugin| plugin == name)
}

/// What a target of the project at `roots` names: its executor's package, its commands, the build entries of `main`
/// and `additionalEntryPoints` and the configs of [`LINKED_OPTIONS`].
fn target_inputs(target: &ConfigValue, roots: &Roots<'_>) -> Vec<PluginInput> {
    let mut out = Vec::new();
    let executor = target.get("executor").and_then(ConfigValue::as_str);
    if let Some(executor) = executor {
        out.extend(executor_package(executor));
    }
    let options = target.get("options");
    let mut commands: Vec<&str> = target.get("command").map(strings).unwrap_or_default();
    if let Some(options) = options {
        commands.extend(options.get("command").map(strings).unwrap_or_default());
        if let Some(ConfigValue::Array(list)) = options.get("commands") {
            for command in list {
                match command {
                    ConfigValue::String(script) => commands.push(script),
                    other => commands.extend(other.get("command").map(strings).unwrap_or_default()),
                }
            }
        }
        out.extend(option_inputs(options, executor, roots));
    }
    // `run-commands` runs its commands in the workspace root, or in `cwd` relative to it, where `{workspaceRoot}` is
    // the way back to the root.
    let run_dir = options
        .and_then(|options| options.get("cwd"))
        .and_then(ConfigValue::as_str)
        .and_then(|cwd| roots.in_workspace(cwd))
        .unwrap_or_default();
    let from_package = relative_path(roots.package, &run_dir);
    let workspace_root = relative_path(&run_dir, "");
    for command in commands {
        let script = with_tokens(command, &workspace_root, &roots.project);
        out.push(PluginInput::Script(if from_package.is_empty() {
            script
        } else {
            format!("cd {} && {script}", shell_word(&from_package))
        }));
    }
    out
}

/// The entries and linked configs that the `options` of a target with `executor` name.
fn option_inputs(options: &ConfigValue, executor: Option<&str>, roots: &Roots<'_>) -> Vec<PluginInput> {
    let mut out = Vec::new();
    let mut entries: Vec<&str> = options.get("main").map(strings).unwrap_or_default();
    entries.extend(options.get("additionalEntryPoints").map(strings).unwrap_or_default());
    out.extend(
        entries
            .into_iter()
            .filter_map(|path| roots.option_path(path))
            .map(PluginInput::ProductionEntry),
    );
    for option in LINKED_OPTIONS {
        if !option.nx_plugin.is_none_or(|name| runs_nx_plugin(executor, name)) {
            continue;
        }
        if let Some(path) = options
            .get(option.key)
            .and_then(ConfigValue::as_str)
            .and_then(|path| roots.option_path(path))
        {
            let path = match option.in_dir {
                Some(files) => join_rel(&path, files),
                None => path,
            };
            out.push(PluginInput::LinkedConfig {
                plugin: option.plugin,
                path,
            });
        }
    }
    out
}

/// `text` with Nx's `{workspaceRoot}` replaced by `workspace_root`, the way to the workspace root from where `text`
/// is read, and `{projectRoot}` by `project_root`, the project's directory relative to the workspace root, as Nx
/// replaces it. An empty value leaves the path after the token as it is.
fn with_tokens(text: &str, workspace_root: &str, project_root: &str) -> String {
    let text = replace_token(text, "{workspaceRoot}", workspace_root);
    replace_token(&text, "{projectRoot}", project_root)
}

fn replace_token(text: &str, token: &str, value: &str) -> String {
    if value.is_empty() {
        text.replace(&format!("{token}/"), "").replace(token, ".")
    } else {
        text.replace(token, value)
    }
}

/// `word` quoted for a shell when it holds anything but path characters.
fn shell_word(word: &str) -> String {
    if word
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '@' | '+'))
    {
        word.to_owned()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}
