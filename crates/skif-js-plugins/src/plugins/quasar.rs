//! Quasar: the app's source files (`sourceFiles` overrides the defaults), the boot files and the CSS files named in
//! `quasar.config.*`, and the fixed files of its Electron, SSR and browser-extension modes.

use std::borrow::Cow;

use skif_js_syntax::{ConfigValue, extract_property_values};

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::package_name;

/// The `quasar` plugin.
pub struct Quasar;

const SOURCE_FILES: &[(&str, &[&str])] = &[
    ("rootComponent", &["src/App.vue"]),
    ("router", &["src/router/index.{js,ts}"]),
    ("store", &["src/stores/index.{js,ts}", "src/store/index.{js,ts}"]),
    (
        "pwaRegisterServiceWorker",
        &["src-pwa/register-service-worker.{js,ts}", "src-pwa/register-sw.{js,ts}"],
    ),
    (
        "pwaServiceWorker",
        &["src-pwa/custom-service-worker.{js,ts}", "src-pwa/sw/custom-sw.{js,ts}"],
    ),
    ("electronMain", &["src-electron/electron-main.{js,ts}"]),
];

const STATIC_ENTRIES: &[&str] = &[
    "src-electron/electron-preload.{js,ts}",
    "src-ssr/server.{js,ts}",
    "src-ssr/middlewares/*.{js,ts}",
    "src-bex/**/*.{js,ts}",
];

static SPEC: PluginSpec = PluginSpec {
    id: "quasar",
    enablers: &[
        Enabler::Dependency("@quasar/app"),
        Enabler::Dependency("@quasar/app-vite"),
        Enabler::Dependency("@quasar/app-webpack"),
    ],
    config: &["quasar.config.{js,cjs,mjs,ts}", "quasar.conf.js"],
    production: &[
        "src/App.vue",
        "src/router/index.{js,ts}",
        "src/stores/index.{js,ts}",
        "src/store/index.{js,ts}",
        "src-pwa/register-service-worker.{js,ts}",
        "src-pwa/register-sw.{js,ts}",
        "src-pwa/custom-service-worker.{js,ts}",
        "src-pwa/sw/custom-sw.{js,ts}",
        "src-electron/electron-main.{js,ts}",
        "src-electron/electron-preload.{js,ts}",
        "src-ssr/server.{js,ts}",
        "src-ssr/middlewares/*.{js,ts}",
        "src-bex/**/*.{js,ts}",
    ],
    binaries: &["quasar"],
    ..PluginSpec::DEFAULT
};

impl Plugin for Quasar {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    /// The config is usually `configure((ctx) => ({ … }))`, so its options are read wherever they are written.
    fn reads_config(&self) -> bool {
        false
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        // The first value of the right shape: a nested `css: { … }` of the build options is not the CSS list.
        let first = |key: &str, array: bool| {
            extract_property_values(config.source, config.path, key)
                .into_iter()
                .find(|value| matches!(value, ConfigValue::Array(_)) == array)
        };
        let source_files = first("sourceFiles", false).unwrap_or(ConfigValue::Null);
        let mut out = Vec::new();
        for (key, defaults) in SOURCE_FILES {
            match source_files.get(key).and_then(ConfigValue::as_str) {
                Some(file) => out.push(PluginInput::ProductionEntry(config.join(&with_extensions(file)))),
                None => out.extend(
                    defaults
                        .iter()
                        .map(|file| PluginInput::ProductionEntry(config.join(file))),
                ),
            }
        }
        out.extend(
            STATIC_ENTRIES
                .iter()
                .map(|file| PluginInput::ProductionEntry(config.join(file))),
        );
        if let Some(ConfigValue::Array(boot)) = first("boot", true) {
            for item in &boot {
                let file = item.as_str().or_else(|| item.get("path").and_then(ConfigValue::as_str));
                if let Some(file) = file {
                    let path = format!("src/boot/{}", with_extensions(file));
                    out.push(PluginInput::ProductionEntry(config.join(&path)));
                }
            }
        }
        if let Some(ConfigValue::Array(css)) = first("css", true) {
            for file in css.iter().filter_map(ConfigValue::as_str) {
                match file.strip_prefix('~') {
                    Some(module) => out.extend(package_name(module).map(PluginInput::Dependency)),
                    None => out.push(PluginInput::ProductionEntry(config.join(&format!("src/css/{file}")))),
                }
            }
        }
        out
    }
}

/// A file named without an extension may be JavaScript or TypeScript.
fn with_extensions(file: &str) -> Cow<'_, str> {
    if file.rsplit('/').next().unwrap_or(file).contains('.') {
        Cow::Borrowed(file)
    } else {
        Cow::Owned(format!("{file}.{{js,ts}}"))
    }
}
