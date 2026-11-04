//! Angular CLI: the builder packages of `angular.json` and the files its targets build or test.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, package_name, strings};

/// The `angular` plugin.
pub struct Angular;

static SPEC: PluginSpec = PluginSpec {
    id: "angular",
    enablers: &[Enabler::Dependency("@angular/cli")],
    config: &["angular.json"],
    binaries: &["ng"],
    ..PluginSpec::DEFAULT
};

/// Options that name a single source file or package.
const FILE_OPTIONS: &[&str] = &["main", "browser", "server", "ssr", "index", "serviceWorker"];

/// The packages the stock Karma builder loads when a target sets no `karmaConfig`.
const DEFAULT_KARMA_PACKAGES: &[&str] = &[
    "karma",
    "karma-jasmine",
    "karma-chrome-launcher",
    "karma-jasmine-html-reporter",
    "karma-coverage",
];

impl Plugin for Angular {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        let Some(projects) = config.value.get("projects").and_then(ConfigValue::as_object) else {
            return out;
        };
        for (_, project) in projects {
            let targets = project.get("architect").or_else(|| project.get("targets"));
            let Some(targets) = targets.and_then(ConfigValue::as_object) else {
                continue;
            };
            for (name, target) in targets {
                out.extend(target_inputs(config, cx, name, target));
            }
        }
        out
    }
}

fn target_inputs(config: &ConfigAst<'_>, cx: &PluginContext<'_>, name: &str, target: &ConfigValue) -> Vec<PluginInput> {
    let mut out = Vec::new();
    let builder = target
        .get("builder")
        .or_else(|| target.get("executor"))
        .and_then(ConfigValue::as_str);
    if let Some(package) = builder
        .and_then(|builder| builder.split(':').next())
        .filter(|package| !package.starts_with('.'))
        .and_then(package_name)
    {
        out.push(PluginInput::Dependency(package));
    }
    let production = name == "build";
    let mut option_sets: Vec<&ConfigValue> = target.get("options").into_iter().collect();
    if let Some(ConfigValue::Object(configurations)) = target.get("configurations") {
        option_sets.extend(configurations.iter().map(|(_, options)| options));
    }
    for options in &option_sets {
        out.extend(option_entries(config, cx, options, production));
    }
    if builder.is_some_and(|builder| builder.ends_with(":karma")) {
        let options = target.get("options");
        match options
            .and_then(|options| options.get("karmaConfig"))
            .and_then(ConfigValue::as_str)
        {
            Some(karma_config) => out.push(PluginInput::ConfigFile(config.join(karma_config))),
            None => out.extend(
                DEFAULT_KARMA_PACKAGES
                    .iter()
                    .map(|package| PluginInput::OptionalDependency((*package).to_owned())),
            ),
        }
        let include = options.and_then(|options| options.get("include")).map(strings);
        let patterns = include.filter(|patterns| !patterns.is_empty());
        for pattern in patterns.unwrap_or_else(|| vec!["**/*.spec.ts"]) {
            out.extend(entry_glob(config.base, pattern).map(PluginInput::Entry));
        }
    }
    out
}

fn option_entries(
    config: &ConfigAst<'_>,
    cx: &PluginContext<'_>,
    options: &ConfigValue,
    production: bool,
) -> Vec<PluginInput> {
    let mut paths: Vec<&str> = FILE_OPTIONS
        .iter()
        .filter_map(|key| options.get(key).and_then(ConfigValue::as_str))
        .collect();
    for key in ["scripts", "styles"] {
        if let Some(ConfigValue::Array(items)) = options.get(key) {
            paths.extend(items.iter().filter_map(|item| match item {
                ConfigValue::String(path) => Some(path.as_str()),
                other => other.get("input").and_then(ConfigValue::as_str),
            }));
        }
    }
    let mut out = Vec::new();
    if let Some(polyfills) = options.get("polyfills") {
        for polyfill in strings(polyfills) {
            if cx.fs.is_file(&cx.dir.join(config.join(polyfill))) {
                paths.push(polyfill);
            } else if let Some(package) = package_name(polyfill) {
                out.push(PluginInput::Dependency(package));
            }
        }
    }
    if let Some(ConfigValue::Array(replacements)) = options.get("fileReplacements") {
        paths.extend(replacements.iter().filter_map(|replacement| {
            replacement
                .get("with")
                .or_else(|| replacement.get("replaceWith"))
                .and_then(ConfigValue::as_str)
        }));
    }
    out.extend(
        paths
            .into_iter()
            .filter_map(|path| entry_glob(config.base, path))
            .map(|path| {
                if production {
                    PluginInput::ProductionEntry(path)
                } else {
                    PluginInput::Entry(path)
                }
            }),
    );
    out
}
