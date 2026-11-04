//! Browser extensions: the scripts and pages a `manifest.json` with a `manifest_version` loads.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{get_in, html_entries, join_rel, objects, strings};

/// The `browser-extension` plugin.
pub struct BrowserExtension;

/// The manifest sits at the package root, in `public/` or `src/`, or in a first-level directory of the same layout
/// (an extension kept in a folder of a package, or in a directory without a `package.json`).
const MANIFEST: &str = "{,*/}{,public/,src/}manifest.json";

static SPEC: PluginSpec = PluginSpec {
    id: "browser-extension",
    enablers: &[Enabler::File(MANIFEST)],
    config: &[MANIFEST],
    ..PluginSpec::DEFAULT
};

/// Build output directories whose prefix is dropped to find the source of a built path.
const BUILD_DIRS: &[&str] = &["dist", "build", "out"];

const SCRIPT_EXTENSIONS: &[&str] = &["js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx"];

/// `SCRIPT_EXTENSIONS` as a brace group.
const SOURCE_EXTENSIONS: &str = "{js,jsx,ts,tsx,mjs,mts,cjs,cts}";

/// Manifest keys holding a single html page.
const PAGE_KEYS: &[&[&str]] = &[
    &["action", "default_popup"],
    &["browser_action", "default_popup"],
    &["page_action", "default_popup"],
    &["options_page"],
    &["options_ui", "page"],
    &["side_panel", "default_path"],
    &["devtools_page"],
    &["background", "page"],
    &["sidebar_action", "default_panel"],
    &["chrome_url_overrides", "newtab"],
    &["chrome_url_overrides", "history"],
    &["chrome_url_overrides", "bookmarks"],
    &["sandbox", "pages"],
];

impl Plugin for BrowserExtension {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let manifest = config.value;
        if manifest.get("manifest_version").is_none() {
            return Vec::new();
        }
        let mut scripts: Vec<&str> = Vec::new();
        if let Some(background) = manifest.get("background") {
            scripts.extend(background.get("service_worker").map(strings).unwrap_or_default());
            scripts.extend(background.get("scripts").map(strings).unwrap_or_default());
        }
        for content in manifest.get("content_scripts").map(objects).unwrap_or_default() {
            scripts.extend(content.get("js").map(strings).unwrap_or_default());
        }
        scripts.extend(web_accessible_scripts(manifest));
        let mut out: Vec<PluginInput> = scripts
            .into_iter()
            .flat_map(|script| source_candidates(config.base, script))
            .map(PluginInput::Entry)
            .collect();
        for key in PAGE_KEYS {
            for page in get_in(manifest, key).map(strings).unwrap_or_default() {
                for candidate in page_candidates(config.base, page) {
                    out.extend(
                        html_entries(cx, "", &candidate)
                            .into_iter()
                            .flat_map(html_script_entries),
                    );
                }
            }
        }
        out
    }
}

/// A script an HTML page loads is an entry, and so is the source it is built from.
fn html_script_entries(input: PluginInput) -> Vec<PluginInput> {
    match input {
        PluginInput::ProductionEntry(path) => {
            let sources = source_candidates("", &path).into_iter().map(PluginInput::Entry);
            std::iter::once(PluginInput::Entry(path)).chain(sources).collect()
        }
        other => vec![other],
    }
}

/// `web_accessible_resources` of manifest v2 (strings) and v3 (objects with `resources`) that are scripts.
fn web_accessible_scripts(manifest: &ConfigValue) -> Vec<&str> {
    let Some(resources) = manifest.get("web_accessible_resources") else {
        return Vec::new();
    };
    let listed = match resources {
        ConfigValue::Array(items) => items
            .iter()
            .flat_map(|item| match item {
                ConfigValue::Object(_) => item.get("resources").map(strings).unwrap_or_default(),
                other => strings(other),
            })
            .collect(),
        _ => Vec::new(),
    };
    listed
        .into_iter()
        .filter(|path| {
            path.rsplit_once('.')
                .is_some_and(|(_, extension)| SCRIPT_EXTENSIONS.contains(&extension))
        })
        .collect()
}

/// The paths a manifest-relative `path` may come from: the path itself, and without its build directory prefix
/// also under `src/`. A script may be built from a file of that name or from the `index` file of a directory of that
/// name (`dist/background.js` from `src/background.ts` or `src/background/index.ts`), with any script extension.
fn source_candidates(base: &str, path: &str) -> Vec<String> {
    let path = path.trim_start_matches('/');
    let mut paths = vec![join_rel(base, path)];
    if let Some(tail) = after_build_dir(path) {
        paths.push(tail.to_string());
        paths.push(join_rel("src", tail));
    } else if base != "src" {
        paths.push(join_rel("src", path));
    }
    paths.sort();
    paths.dedup();
    paths.iter().flat_map(|candidate| script_sources(candidate)).collect()
}

/// What follows the first build directory (`dist`, `build`, `out`) of a path.
fn after_build_dir(path: &str) -> Option<&str> {
    let mut rest = path;
    while let Some((first, tail)) = rest.split_once('/') {
        if BUILD_DIRS.contains(&first) {
            return Some(tail);
        }
        rest = tail;
    }
    None
}

/// The source files a script path may be built from; a path that is not a script is kept as it is.
fn script_sources(path: &str) -> Vec<String> {
    match path.rsplit_once('.') {
        Some((stem, extension)) if SCRIPT_EXTENSIONS.contains(&extension) => vec![
            format!("{stem}.{SOURCE_EXTENSIONS}"),
            format!("{stem}/index.{SOURCE_EXTENSIONS}"),
        ],
        _ => vec![path.to_string()],
    }
}

/// The places a popup or options page may live: next to the manifest, the workspace root or `src/`.
fn page_candidates(base: &str, page: &str) -> Vec<String> {
    let page = page.trim_start_matches('/');
    let mut paths = vec![join_rel(base, page), page.to_string(), join_rel("src", page)];
    if let Some((first, rest)) = page.split_once('/')
        && BUILD_DIRS.contains(&first)
    {
        paths.push(rest.to_string());
        paths.push(join_rel("src", rest));
    }
    paths.sort();
    paths.dedup();
    paths
}
