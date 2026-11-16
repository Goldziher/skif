//! Heft: the packages its config files extend and the plugin packages they load, and the configs of the rig that a
//! project's `config/rig.json` names (ADR 0008).
//!
//! A rig package ships `profiles/<profile>/config/*.json` and `profiles/<profile>/tsconfig*.json` for the projects
//! that name it; in the rig itself these profiles are its own configs.

use std::path::Path;

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::plugins::typescript;
use crate::support::{is_relative, objects, package_name, values_of};

/// The `heft` plugin.
pub struct Heft;

/// The plugin's identifier.
pub const ID: crate::plugin::PluginId = "heft";

/// The package of Heft itself.
const HEFT: &str = "@rushstack/heft";

/// The file a project names its rig in.
const RIG_JSON: &str = "rig.json";

/// The profile a `config/rig.json` without `rigProfile` selects.
const DEFAULT_PROFILE: &str = "default";

/// Heft plugin packages that load another package the project provides, which they credit without requiring it.
const LOADED_BY_PLUGIN: &[(&str, &[&str])] = &[("@rushstack/heft-sass-plugin", &["sass"])];

/// Heft plugin packages whose task `options` name packages the plugin runs, with the option keys naming them and
/// whether the project needs the package itself. The Storybook plugin resolves its storykit from the project and the
/// CLI package from the storykit, so the CLI is the storykit's dependency: a listing in the project is used, a missing
/// one is no unlisted dependency.
const NAMED_BY_OPTIONS: &[(&str, &[(&str, Need)])] = &[(
    "@rushstack/heft-storybook-plugin",
    &[("storykitPackageName", Need::Project), ("cliPackageName", Need::Other)],
)];

/// Who resolves a package a task plugin's option names.
#[derive(Clone, Copy)]
enum Need {
    /// The project: it must list the package.
    Project,
    /// Another package the plugin resolves first.
    Other,
}

/// The Jest plugin of Heft, which runs the compiled tests of the project.
const JEST_PLUGIN: &str = "@rushstack/heft-jest-plugin";

/// The sources of the tests the Jest plugin runs by default: its shared config matches `lib/**/*.test.{cjs,js}`, the
/// output of these files, and Jest loads the manual mocks next to them.
const JEST_ENTRIES: &[&str] = &[
    "src/**/*.test.{ts,tsx,mts,cts,js,jsx,mjs,cjs}",
    "src/**/__mocks__/**/*.{ts,tsx,js,jsx}",
];

static SPEC: PluginSpec = PluginSpec {
    id: ID,
    enablers: &[Enabler::Dependency(HEFT), Enabler::File("config/rig.json")],
    config: &[
        "config/heft.json",
        "config/rig.json",
        "config/typescript.json",
        "config/jest.config.json",
        "profiles/*/config/*.json",
    ],
    binaries: &["heft"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Heft {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let name = config.file_name();
        // The `tsconfig` a linked one extends is queued for this plugin, which has nothing to read in it.
        if name.starts_with("tsconfig") {
            return Vec::new();
        }
        if name == RIG_JSON && config.base == "config" {
            return rig_inputs(config.value, cx);
        }
        let mut out = extends_inputs(config);
        if name == "heft.json" {
            let packages = plugin_packages(config.value);
            if packages.contains(&JEST_PLUGIN) {
                out.extend(JEST_ENTRIES.iter().map(|glob| PluginInput::Entry((*glob).to_owned())));
            }
            out.extend(packages.into_iter().flat_map(plugin_inputs));
            out.extend(option_packages(config.value));
            // A rig provides Heft to the projects that use it.
            if is_profile_config(config.base) {
                out.push(PluginInput::OptionalDependency(HEFT.to_owned()));
            }
        }
        out
    }

    /// A rig's profile `tsconfig` files are configs it ships for the projects that extend them.
    fn resolve_workspace(&self, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        vec![PluginInput::SharedConfig {
            plugin: typescript::ID,
            path: "profiles/*/tsconfig*.json".to_owned(),
        }]
    }
}

/// Whether a config in `base`, relative to the package, is one of a rig profile (`profiles/<profile>/config`).
fn is_profile_config(base: &str) -> bool {
    let mut parts = base.split('/');
    matches!(
        (parts.next(), parts.next(), parts.next(), parts.next()),
        (Some("profiles"), Some(profile), Some("config"), None) if !profile.is_empty()
    )
}

/// The `extends` of a Heft config file: a relative path is a further config, else the package it names.
fn extends_inputs(config: &ConfigAst<'_>) -> Vec<PluginInput> {
    let Some(extends) = config.value.get("extends").and_then(ConfigValue::as_str) else {
        return Vec::new();
    };
    if is_relative(extends) {
        return vec![PluginInput::ConfigFile(config.join(extends))];
    }
    match package_name(extends) {
        Some(name) => vec![PluginInput::Dependency(name)],
        None => Vec::new(),
    }
}

/// The `pluginPackage` of every lifecycle plugin (`heftPlugins`) and of every task's plugin
/// (`phasesByName.<phase>.tasksByName.<task>.taskPlugin`).
fn plugin_packages(value: &ConfigValue) -> Vec<&str> {
    let lifecycle = value.get("heftPlugins").map_or_default(objects);
    lifecycle
        .into_iter()
        .chain(task_plugins(value))
        .filter_map(|plugin| plugin.get("pluginPackage").and_then(ConfigValue::as_str))
        .collect()
}

/// The `taskPlugin` of every task (`phasesByName.<phase>.tasksByName.<task>`).
fn task_plugins(value: &ConfigValue) -> Vec<&ConfigValue> {
    value
        .get("phasesByName")
        .map_or_default(values_of)
        .into_iter()
        .filter_map(|phase| phase.get("tasksByName"))
        .flat_map(values_of)
        .filter_map(|task| task.get("taskPlugin"))
        .collect()
}

/// The packages a task plugin's `options` name for it to run ([`NAMED_BY_OPTIONS`]), such as the storykit and CLI
/// packages of the Storybook plugin.
fn option_packages(value: &ConfigValue) -> Vec<PluginInput> {
    task_plugins(value)
        .into_iter()
        .filter_map(|plugin| {
            let package = plugin.get("pluginPackage").and_then(ConfigValue::as_str)?;
            let keys = NAMED_BY_OPTIONS.iter().find(|(name, _)| *name == package)?.1;
            Some((plugin.get("options")?, keys))
        })
        .flat_map(|(options, keys)| {
            keys.iter()
                .filter_map(|(key, need)| Some((options.get(key)?.as_str()?, *need)))
        })
        .filter_map(|(value, need)| {
            let name = package_name(value)?;
            Some(match need {
                Need::Project => PluginInput::Dependency(name),
                Need::Other => PluginInput::OptionalDependency(name),
            })
        })
        .collect()
}

/// The package a `pluginPackage` names, and the packages that plugin loads ([`LOADED_BY_PLUGIN`]).
fn plugin_inputs(specifier: &str) -> Vec<PluginInput> {
    let Some(name) = package_name(specifier) else {
        return Vec::new();
    };
    let loaded = LOADED_BY_PLUGIN
        .iter()
        .filter(|(plugin, _)| *plugin == name)
        .flat_map(|(_, loaded)| loaded.iter())
        .map(|loaded| PluginInput::OptionalDependency((*loaded).to_owned()));
    std::iter::once(PluginInput::Dependency(name.clone()))
        .chain(loaded)
        .collect()
}

/// A project's `config/rig.json`: the rig package it needs, and the configs of the selected profile of the installed
/// rig (`node_modules/<rigPackageName>/profiles/<rigProfile>`), which are the project's own.
fn rig_inputs(value: &ConfigValue, cx: &PluginContext<'_>) -> Vec<PluginInput> {
    let Some(rig) = value
        .get("rigPackageName")
        .and_then(ConfigValue::as_str)
        .filter(|rig| package_name(rig).as_deref() == Some(*rig))
    else {
        return Vec::new();
    };
    let mut out = vec![PluginInput::Dependency(rig.to_owned())];
    let profile = value
        .get("rigProfile")
        .and_then(ConfigValue::as_str)
        .unwrap_or(DEFAULT_PROFILE);
    if is_directory_name(profile) {
        out.extend(profile_configs(cx, &format!("node_modules/{rig}/profiles/{profile}")));
    }
    out
}

/// Whether `name` is one directory name, neither empty nor `.` or `..`, so that a profile stays inside the rig.
fn is_directory_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\'])
}

/// The configs of the rig profile in `profile`, relative to the package: each `config/*.json` the project does not
/// have itself, as a project's own config wins over its rig's, and each `tsconfig*.json`, read by `typescript`.
fn profile_configs(cx: &PluginContext<'_>, profile: &str) -> Vec<PluginInput> {
    let configs = json_files(cx, &format!("{profile}/config"))
        .into_iter()
        .filter(|name| !cx.fs.is_file(&cx.dir.join("config").join(name)))
        .map(|name| PluginInput::LinkedConfig {
            plugin: ID,
            path: format!("{profile}/config/{name}"),
        });
    let tsconfigs = json_files(cx, profile)
        .into_iter()
        .filter(|name| name.starts_with("tsconfig"))
        .map(|name| PluginInput::LinkedConfig {
            plugin: typescript::ID,
            path: format!("{profile}/{name}"),
        });
    configs.chain(tsconfigs).collect()
}

/// The names of the `.json` files directly in `dir`, relative to the package, in name order.
fn json_files(cx: &PluginContext<'_>, dir: &str) -> Vec<String> {
    let Ok(entries) = cx.fs.read_dir_shared(&cx.dir.join(dir)) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .iter()
        .filter(|entry| entry.is_file)
        .filter_map(|entry| entry.path.file_name()?.to_str().map(str::to_owned))
        .filter(|name| {
            Path::new(name)
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
        })
        .collect();
    names.sort();
    names
}
