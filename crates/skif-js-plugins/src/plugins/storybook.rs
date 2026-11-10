//! Storybook: story globs, addons, framework and builder packages named in `.storybook/main`.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{entry_glob, module_input, objects, shallow_key_unknown, strings};

/// The `storybook` plugin.
pub struct Storybook;

static SPEC: PluginSpec = PluginSpec {
    id: "storybook",
    enablers: &[
        Enabler::DependencyPrefix("@storybook/"),
        Enabler::Dependency("@nrwl/storybook"),
    ],
    config: &[".{storybook,rnstorybook}/{main,test-runner}.{js,ts,mts,cjs,mjs}"],
    entry: &[".{storybook,rnstorybook}/{manager,preview,index,vitest.setup}.{js,jsx,ts,tsx}"],
    project: &[".{storybook,rnstorybook}/**/*.{js,jsx,ts,tsx,mts}"],
    binaries: &["storybook", "sb", "start-storybook", "build-storybook"],
    ..PluginSpec::DEFAULT
};

const DEFAULT_STORIES: &str = "**/*.{mdx,stories.{mdx,js,jsx,mjs,ts,tsx}}";

impl Plugin for Storybook {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_without_config(&self, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        vec![PluginInput::Entry(DEFAULT_STORIES.to_string())]
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let value = config.value;
        let mut out = Vec::new();
        let patterns = stories(value);
        if patterns.is_empty() {
            out.push(PluginInput::Entry(DEFAULT_STORIES.to_string()));
        }
        // Story globs are written relative to the config directory.
        for pattern in patterns {
            out.extend(entry_glob(config.base, &pattern).map(PluginInput::Entry));
        }
        let mut addons = Vec::new();
        match value.get("addons") {
            Some(ConfigValue::Unknown) => out.push(PluginInput::EntriesIncomplete),
            Some(ConfigValue::Array(listed)) => {
                for addon in listed {
                    if let Some(name) = package_named(addon, &mut out) {
                        out.extend(module_input(config.base, name, false));
                        addons.push(name);
                    }
                }
            }
            _ => {}
        }
        if let Some(core) = value.get("core") {
            if shallow_key_unknown(core, "builder") {
                out.push(PluginInput::EntriesIncomplete);
            }
            if let Some(builder) = core.get("builder").and_then(|builder| package_named(builder, &mut out)) {
                if builder.starts_with("webpack") {
                    out.push(PluginInput::Dependency(format!("@storybook/builder-{builder}")));
                    out.push(PluginInput::Dependency(format!("@storybook/manager-{builder}")));
                } else {
                    out.extend(module_input(config.base, builder, false));
                }
            }
        }
        if let Some(framework) = value
            .get("framework")
            .and_then(|framework| package_named(framework, &mut out))
        {
            out.extend(module_input(config.base, framework, false));
        }
        if addons.contains(&"@storybook/addon-vitest") {
            for provider in ["v8", "istanbul"] {
                out.push(PluginInput::OptionalDependency(format!("@vitest/coverage-{provider}")));
            }
        }
        out
    }
}

/// The package an addon, the framework or the builder names: a string, or the `name` of an object with options. When
/// the name is not known statically, the packages the config needs are not all known, so
/// [`PluginInput::EntriesIncomplete`] is added to `out`.
fn package_named<'v>(value: &'v ConfigValue, out: &mut Vec<PluginInput>) -> Option<&'v str> {
    if shallow_key_unknown(value, "name") {
        out.push(PluginInput::EntriesIncomplete);
    }
    value
        .as_str()
        .or_else(|| value.get("name").and_then(ConfigValue::as_str))
}

/// Story globs, with `{ directory, files }` objects expanded.
fn stories(main: &ConfigValue) -> Vec<String> {
    let Some(stories) = main.get("stories") else {
        return Vec::new();
    };
    let mut out: Vec<String> = strings(stories).into_iter().map(str::to_string).collect();
    for object in objects(stories) {
        if let Some(directory) = object.get("directory").and_then(ConfigValue::as_str) {
            let files = object
                .get("files")
                .and_then(ConfigValue::as_str)
                .unwrap_or(DEFAULT_STORIES);
            out.push(format!("{}/{files}", directory.trim_end_matches('/')));
        }
    }
    out
}
