//! Nx: executors, plugins and generators are packages, or local modules whose files and collections are followed;
//! `run-commands` targets are scripts, and target options name build entries and the configs of other plugins.

use std::cell::OnceCell;
use std::path::PathBuf;

use skif_js_syntax::ConfigValue;

use crate::config_load::load_config;
use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginId, PluginInput, PluginSpec};
use crate::plugins::{cypress, eslint, expo_router, jest, playwright, storybook, typescript, vite, webpack};
use crate::support::{is_relative, join_lexically, join_rel, package_name, path_within, relative_path, strings};

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
        let roots = Roots::new(cx, config.base);
        let local = Local::new(cx, &roots);
        if config.file_name() == "nx.json" {
            if let Some(ConfigValue::Object(defaults)) = config.value.get("targetDefaults") {
                for (key, _) in defaults
                    .iter()
                    .filter(|(key, _)| key.starts_with('@') && key.contains(':'))
                {
                    out.extend(local.executor(key));
                }
            }
            if let Some(ConfigValue::Array(plugins)) = config.value.get("plugins") {
                let specifiers = plugins.iter().filter_map(|plugin| match plugin {
                    ConfigValue::String(name) => Some(name.as_str()),
                    other => other.get("plugin").and_then(ConfigValue::as_str),
                });
                for specifier in specifiers {
                    out.extend(local.plugin(specifier));
                }
            }
            if let Some(ConfigValue::Object(generators)) = config.value.get("generators") {
                for (key, _) in generators {
                    out.extend(local.executor(key));
                }
            }
        }
        if let Some(ConfigValue::Object(targets)) = config.value.get("targets") {
            for (_, target) in targets {
                out.extend(target_inputs(target, &local));
            }
        }
        out
    }

    /// A local Nx plugin that is a workspace package names its `executors`, `builders`, `generators` and
    /// `schematics` collections in its own `package.json`.
    fn resolve_workspace(&self, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let roots = Roots::new(cx, "");
        let local = Local::new(cx, &roots);
        local
            .read(&join_rel(roots.package, "package.json"))
            .map(|manifest| local.collections_in(roots.package, &manifest))
            .unwrap_or_default()
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

/// The keys of a `package.json` that name a collection of Nx executors or generators (or Angular builders and
/// schematics), which are also the keys of the collection file that hold its members.
const COLLECTION_KEYS: &[&str] = &["executors", "builders", "generators", "schematics"];

/// The keys of a collection member that name the module Nx loads to run it.
const IMPLEMENTATION_KEYS: &[&str] = &["implementation", "batchImplementation", "factory"];

/// The extensions a module path without one is tried with.
const MODULE_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// Resolves the plugins, executors and generators a config names as Nx does: a relative path from the workspace root
/// or a `paths` alias of the root `tsconfig.base.json` (else `tsconfig.json`) is a module of the workspace, followed
/// to its file and its collections; any other specifier is a package.
struct Local<'c, 'a> {
    cx: &'c PluginContext<'a>,
    roots: &'c Roots<'a>,
    /// The `paths` of the root tsconfig: each pattern with its first target, relative to the workspace root.
    aliases: OnceCell<Vec<(String, String)>>,
}

impl<'c, 'a> Local<'c, 'a> {
    fn new(cx: &'c PluginContext<'a>, roots: &'c Roots<'a>) -> Self {
        Self {
            cx,
            roots,
            aliases: OnceCell::new(),
        }
    }

    /// What a `plugins` entry of `nx.json` names: a local plugin's module and its collections, or a package.
    fn plugin(&self, specifier: &str) -> Vec<PluginInput> {
        let Some(target) = self.target(specifier) else {
            return package_name(specifier)
                .map(PluginInput::Dependency)
                .into_iter()
                .collect();
        };
        let mut out: Vec<PluginInput> = self
            .module_file(&target)
            .and_then(|file| self.roots.option_path(&file))
            .map(PluginInput::Entry)
            .into_iter()
            .collect();
        out.extend(self.collections(&target));
        out
    }

    /// What an executor or generator (`@fx/nxp:run`, `./tools/executors/echo:echo`, `@nx/react`) names: the
    /// collections of a local plugin, as Nx reads only its `package.json` and collections for it, or a package.
    fn executor(&self, executor: &str) -> Vec<PluginInput> {
        let name = executor.split(':').next().unwrap_or_default();
        match self.target(name) {
            Some(target) => self.collections(&target),
            None => executor_package(executor).into_iter().collect(),
        }
    }

    /// The module `specifier` names in the workspace, relative to its root: a relative path, or the target of the
    /// first `paths` alias that matches it when that exists. `None` for a package.
    fn target(&self, specifier: &str) -> Option<String> {
        let specifier = specifier.trim();
        if is_relative(specifier) {
            return self.roots.in_workspace(specifier);
        }
        self.aliases().iter().find_map(|(pattern, target)| {
            let found = match pattern.split_once('*') {
                None => (pattern == specifier).then(|| target.clone()),
                Some((prefix, suffix)) => specifier
                    .strip_prefix(prefix)
                    .and_then(|rest| rest.strip_suffix(suffix))
                    .filter(|matched| !matched.is_empty())
                    .map(|matched| target.replacen('*', matched, 1)),
            };
            found
                .and_then(|path| self.roots.in_workspace(&path))
                .filter(|path| self.cx.fs.is_dir(&self.absolute(path)) || self.file_or_with_extension(path).is_some())
        })
    }

    fn aliases(&self) -> &[(String, String)] {
        self.aliases.get_or_init(|| {
            ["tsconfig.base.json", "tsconfig.json"]
                .into_iter()
                .find_map(|name| self.read(name))
                .and_then(|tsconfig| tsconfig.get("compilerOptions").map(tsconfig_aliases))
                .unwrap_or_default()
        })
    }

    /// The file a module path (relative to the workspace root) loads: the file, the file with an extension, the
    /// `main` of a directory's `package.json` or its `index` file.
    fn module_file(&self, path: &str) -> Option<String> {
        self.file_or_with_extension(path).or_else(|| {
            self.read(&join_rel(path, "package.json"))
                .and_then(|manifest| manifest.get("main").and_then(ConfigValue::as_str).map(str::to_owned))
                .and_then(|main| self.file_or_with_extension(&join_rel(path, &main)))
                .or_else(|| self.with_extension(&join_rel(path, "index")))
        })
    }

    fn file_or_with_extension(&self, path: &str) -> Option<String> {
        if self.is_file(path) {
            Some(path.to_owned())
        } else {
            self.with_extension(path)
        }
    }

    fn with_extension(&self, path: &str) -> Option<String> {
        MODULE_EXTENSIONS
            .iter()
            .map(|extension| format!("{path}.{extension}"))
            .find(|candidate| self.is_file(candidate))
    }

    /// The `implementation`, `batchImplementation` and `factory` files of the collections that the `package.json` of
    /// the local plugin at `target` names, as entries: the manifest in `target` itself, or else the closest one above
    /// it below the workspace root.
    fn collections(&self, target: &str) -> Vec<PluginInput> {
        self.plugin_manifest(target)
            .map(|(dir, manifest)| self.collections_in(&dir, &manifest))
            .unwrap_or_default()
    }

    /// The members' files of the collections that `manifest`, the `package.json` in `dir` (relative to the workspace
    /// root), names.
    fn collections_in(&self, dir: &str, manifest: &ConfigValue) -> Vec<PluginInput> {
        let mut out = Vec::new();
        for key in COLLECTION_KEYS {
            let Some(collection) = manifest.get(key).and_then(ConfigValue::as_str) else {
                continue;
            };
            let collection = join_rel(dir, collection);
            let Some(members) = self.read(&collection) else {
                continue;
            };
            let collection_dir = collection.rsplit_once('/').map_or("", |(parent, _)| parent);
            let modules = COLLECTION_KEYS
                .iter()
                .filter_map(|key| match members.get(key) {
                    Some(ConfigValue::Object(entries)) => Some(entries),
                    _ => None,
                })
                .flatten()
                .flat_map(|(_, member)| {
                    IMPLEMENTATION_KEYS
                        .iter()
                        .filter_map(move |key| member.get(key)?.as_str())
                });
            for module in modules {
                let path = module.split('#').next().unwrap_or_default();
                let entry = self
                    .module_file(&join_rel(collection_dir, path))
                    .and_then(|file| self.roots.option_path(&file))
                    .map(PluginInput::Entry);
                if let Some(entry) = entry
                    && !out.contains(&entry)
                {
                    out.push(entry);
                }
            }
        }
        out
    }

    /// The directory, relative to the workspace root, and the value of the `package.json` of the local plugin at
    /// `target`.
    fn plugin_manifest(&self, target: &str) -> Option<(String, ConfigValue)> {
        let mut dir = if self.is_file(target) {
            target.rsplit_once('/').map_or("", |(parent, _)| parent)
        } else {
            target
        };
        while !dir.is_empty() {
            if let Some(manifest) = self.read(&join_rel(dir, "package.json")) {
                return Some((dir.to_owned(), manifest));
            }
            dir = dir.rsplit_once('/').map_or("", |(parent, _)| parent);
        }
        None
    }

    /// The absolute path of `path`, relative to the workspace root.
    fn absolute(&self, path: &str) -> PathBuf {
        join_lexically(self.cx.dir, &relative_path(self.roots.package, path))
    }

    fn is_file(&self, path: &str) -> bool {
        self.cx.fs.is_file(&self.absolute(path))
    }

    /// The config file at `path`, relative to the workspace root, when it can be read.
    fn read(&self, path: &str) -> Option<ConfigValue> {
        let path = self.absolute(path);
        let source = self.cx.fs.read_to_string(&path).ok()?;
        load_config(&path, &source)
    }
}

/// The `paths` of tsconfig `compilerOptions`, each pattern with its first target relative to `baseUrl` (by default the
/// tsconfig's directory, the workspace root).
fn tsconfig_aliases(options: &ConfigValue) -> Vec<(String, String)> {
    let base = options.get("baseUrl").and_then(ConfigValue::as_str).unwrap_or(".");
    let Some(ConfigValue::Object(paths)) = options.get("paths") else {
        return Vec::new();
    };
    paths
        .iter()
        .filter_map(|(pattern, targets)| {
            let target = strings(targets).into_iter().next()?;
            Some((pattern.clone(), join_rel(base, target)))
        })
        .collect()
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
    executor.and_then(nx_plugin_of).is_some_and(|plugin| plugin == name)
}

/// The Nx plugin an executor comes from: `vite` for `@nx/vite:build` and `@nrwl/vite:build`.
fn nx_plugin_of(executor: &str) -> Option<&str> {
    let package = executor.split(':').next()?;
    package.strip_prefix("@nx/").or_else(|| package.strip_prefix("@nrwl/"))
}

/// Plugins whose tool the executors of an Nx plugin of another name run.
const EXECUTOR_ALIASES: &[(PluginId, &str)] = &[(expo_router::ID, "expo")];

/// The Nx plugins whose executors the targets of the `project.json` in the directory of `cx` run (`vite` for
/// `@nx/vite:build`), sorted.
pub(crate) fn executor_plugins(cx: &PluginContext<'_>) -> Vec<String> {
    let path = cx.dir.join("project.json");
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
    let mut names: Vec<String> = targets
        .iter()
        .filter_map(|(_, target)| nx_plugin_of(target.get("executor")?.as_str()?))
        .map(str::to_owned)
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Whether one of `executor_plugins` ([`executor_plugins`]) runs the tool of the plugin `plugin` (`@nx/vite:build`
/// runs Vite), with the tool's defaults when the project has no config file of it.
pub(crate) fn runs_tool_of(executor_plugins: &[String], plugin: PluginId) -> bool {
    executor_plugins.iter().any(|name| {
        name == plugin
            || EXECUTOR_ALIASES
                .iter()
                .any(|(id, alias)| *id == plugin && name == alias)
    })
}

/// What a target of the project names: its executor's package or local collections, its commands, the build entries
/// of `main` and `additionalEntryPoints` and the configs of [`LINKED_OPTIONS`].
fn target_inputs(target: &ConfigValue, local: &Local<'_, '_>) -> Vec<PluginInput> {
    let roots = local.roots;
    let mut out = Vec::new();
    let executor = target.get("executor").and_then(ConfigValue::as_str);
    if let Some(executor) = executor {
        out.extend(local.executor(executor));
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
