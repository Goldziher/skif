//! Storybook: story globs, addons, framework and builder packages named in `.storybook/main`.

use skif_js_syntax::ConfigValue;

use crate::plugin::{
    ConfigAst, Enabler, IncompleteCause, IncompleteScope, Plugin, PluginContext, PluginInput, PluginSpec,
};
use crate::support::{Entry, Spreads, entry_glob, join_rel, last_entry, module_input, objects, strings};

/// The `storybook` plugin.
pub struct Storybook;

/// The plugin's identifier.
pub const ID: crate::plugin::PluginId = "storybook";

static SPEC: PluginSpec = PluginSpec {
    id: ID,
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
        for listed in value.get("addons").map_or_default(ConfigValue::alternatives) {
            match listed {
                ConfigValue::Array(listed) => {
                    for addon in listed {
                        for name in packages_named(addon, &mut out) {
                            out.extend(module_input(config.base, name, false));
                            addons.push(name);
                        }
                    }
                }
                unknown if unknown.is_unknown() => out.push(PluginInput::EntriesIncomplete),
                _ => {}
            }
        }
        if let Some(core) = value.get("core") {
            if core.is_unknown() || last_entry(core, "builder", Spreads::Any) == Some(Entry::Spread) {
                out.push(PluginInput::EntriesIncomplete);
            }
            let builders = core
                .get("builder")
                .map_or_default(|builder| packages_named(builder, &mut out));
            for builder in builders {
                if builder.starts_with("webpack") {
                    out.push(PluginInput::Dependency(format!("@storybook/builder-{builder}")));
                    out.push(PluginInput::Dependency(format!("@storybook/manager-{builder}")));
                } else {
                    out.extend(module_input(config.base, builder, false));
                }
            }
        }
        for framework in value
            .get("framework")
            .map_or_default(|framework| packages_named(framework, &mut out))
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

    /// What the config hides when it names an addon, the framework or the builder in a way that is not known
    /// statically is a package, whose dependencies are then not judged, or a local addon, which lives in the
    /// Storybook config directory; any other file of the package is judged. A config unknown as a whole, or one that
    /// spreads or merges in an unknown value, may name stories anywhere.
    fn incomplete_scope(&self, cause: IncompleteCause, config: &ConfigAst<'_>) -> IncompleteScope {
        if cause != IncompleteCause::ComputedEntries {
            return IncompleteScope::Package;
        }
        let mut globs = vec![join_rel(config.base, "**")];
        globs.extend(SPEC.project.iter().map(|glob| (*glob).to_owned()));
        IncompleteScope::Within(globs)
    }
}

/// The packages an addon, the framework or the builder names: a string, or the `name` of an object with options, for
/// each of the values it may be ([`ConfigValue::alternatives`]). When a name is not known statically, or a spread may
/// set it, the packages the config needs are not all known, so [`PluginInput::EntriesIncomplete`] is added to `out`.
fn packages_named<'v>(value: &'v ConfigValue, out: &mut Vec<PluginInput>) -> Vec<&'v str> {
    let mut names = Vec::new();
    for alternative in value.alternatives() {
        if last_entry(alternative, "name", Spreads::Any) == Some(Entry::Spread) {
            out.push(PluginInput::EntriesIncomplete);
        }
        let name = if alternative.as_object().is_some() {
            alternative.get("name")
        } else {
            Some(alternative)
        };
        for name in name.map_or_default(ConfigValue::alternatives) {
            match name {
                ConfigValue::String(name) => names.push(name.as_str()),
                unknown if unknown.is_unknown() => out.push(PluginInput::EntriesIncomplete),
                _ => {}
            }
        }
    }
    names
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
