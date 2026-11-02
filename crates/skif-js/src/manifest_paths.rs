//! Paths a `package.json` publishes and how built output maps back to source.

use std::path::{Path, PathBuf};

use serde_json::Value;
use skif_core::FileSystem;

/// Conditions an import from a package can match, in the order the Node.js and TypeScript resolvers apply them.
const CONDITIONS: &[&str] = &["import", "require", "node", "default", "types"];

/// Directories that hold build output; their source twin is `src`.
const OUT_DIRS: &[&str] = &["dist", "build", "lib", "out", "esm", "cjs"];

/// Manifest fields that point at a single entry file, in order of preference for a package import.
pub(crate) const ENTRY_FIELDS: &[&str] = &["source", "module", "main", "types", "typings"];

const CODE_EXTENSIONS: &[&str] = &[".ts", ".tsx", ".js", ".jsx", ".mts", ".cts", ".mjs", ".cjs"];

const EXTENSION_PAIRS: &[(&str, &[&str])] = &[
    (".d.mts", &[".mts", ".d.mts"]),
    (".d.cts", &[".cts", ".d.cts"]),
    (".d.ts", &[".ts", ".tsx", ".d.ts"]),
    (".mjs", &[".mts", ".mjs"]),
    (".cjs", &[".cts", ".cjs"]),
    (".jsx", &[".tsx", ".jsx"]),
    (".js", &[".ts", ".tsx", ".js", ".jsx"]),
    (".tsx", &[".tsx"]),
    (".mts", &[".mts"]),
    (".cts", &[".cts"]),
    (".ts", &[".ts"]),
];

/// Splits a path into stem and the extension that has source twins. Other extensions stay part of the stem.
fn split_extension(path: &str) -> (&str, Option<&'static [&'static str]>) {
    for (suffix, variants) in EXTENSION_PAIRS {
        if let Some(stem) = path.strip_suffix(suffix) {
            return (stem, Some(variants));
        }
    }
    (path, None)
}

fn has_other_extension(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.contains('.') && split_extension(path).1.is_none()
}

/// Package-relative candidates for the source of a published file, most likely first.
///
/// `./dist/index.js` yields `src/index.ts`, `src/index.tsx`, `src/index.js`, … and then the same names without the
/// `src` rewrite. A path with a non-code extension (`./styles.css`) yields only itself.
pub(crate) fn source_candidates(target: &str) -> Vec<String> {
    let clean = target.trim_start_matches("./");
    if has_other_extension(clean) {
        return vec![clean.to_owned()];
    }
    let (stem, variants) = split_extension(clean);
    let extensions: &[&str] = variants.unwrap_or(CODE_EXTENSIONS);
    let mut bases = Vec::new();
    if let Some((first, rest)) = stem.split_once('/')
        && OUT_DIRS.contains(&first)
    {
        bases.push(format!("src/{rest}"));
    }
    bases.push(stem.to_owned());
    bases
        .iter()
        .flat_map(|base| extensions.iter().map(move |extension| format!("{base}{extension}")))
        .collect()
}

/// The existing source file behind a published path, relative to the package directory.
pub(crate) fn map_to_source(fs: &dyn FileSystem, package_dir: &Path, target: &str) -> Option<PathBuf> {
    source_candidates(target)
        .into_iter()
        .map(|candidate| package_dir.join(candidate))
        .find(|path| fs.is_file(path))
}

/// Package-relative glob patterns for a published path that contains `*`.
pub(crate) fn source_patterns(target: &str) -> Vec<String> {
    let clean = target.trim_start_matches("./");
    if has_other_extension(clean) {
        return vec![clean.to_owned()];
    }
    let (stem, _) = split_extension(clean);
    let extensions = "{ts,tsx,js,jsx,mts,cts,mjs,cjs}";
    let mut patterns = Vec::new();
    if let Some((first, rest)) = stem.split_once('/')
        && OUT_DIRS.contains(&first)
    {
        patterns.push(format!("src/{rest}.{extensions}"));
    }
    patterns.push(format!("{stem}.{extensions}"));
    patterns
}

fn collect_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) if !text.is_empty() => out.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| collect_strings(item, out)),
        Value::Object(map) => map.values().for_each(|item| collect_strings(item, out)),
        _ => {}
    }
}

/// Every file path a manifest publishes: `main`, `module`, `types`, `bin` and the leaves of `exports`.
pub(crate) fn manifest_targets(manifest: &Value) -> Vec<String> {
    let mut targets = Vec::new();
    for field in ENTRY_FIELDS {
        if let Some(Value::String(text)) = manifest.get(*field) {
            targets.push(text.clone());
        }
    }
    if let Some(Value::String(text)) = manifest.get("browser") {
        targets.push(text.clone());
    }
    if let Some(bin) = manifest.get("bin") {
        collect_strings(bin, &mut targets);
    }
    if let Some(exports) = manifest.get("exports") {
        let mut leaves = Vec::new();
        collect_strings(exports, &mut leaves);
        targets.extend(leaves.into_iter().filter(|leaf| leaf.starts_with("./")));
    }
    targets.retain(|target| !target.is_empty());
    targets
}

/// Whether an `exports` condition applies to an import: the standard ones, `source` and the project's own.
fn condition_applies(key: &str, custom: &[String]) -> bool {
    CONDITIONS.contains(&key) || key == "source" || custom.iter().any(|condition| condition == key)
}

fn resolve_target(value: &Value, matched: Option<&str>, custom: &[String]) -> Option<String> {
    match value {
        Value::String(text) => Some(matched.map_or_else(|| text.clone(), |m| text.replace('*', m))),
        Value::Array(items) => items.iter().find_map(|item| resolve_target(item, matched, custom)),
        Value::Object(map) => map
            .iter()
            .filter(|(key, _)| condition_applies(key, custom))
            .find_map(|(_, item)| resolve_target(item, matched, custom)),
        _ => None,
    }
}

/// Resolves `subpath` (`"."` or `"./x"`) against a package's `exports` for the conditions of an import, the
/// `source` condition and the `custom` conditions of the project.
pub(crate) fn resolve_exports(exports: &Value, subpath: &str, custom: &[String]) -> Option<String> {
    match exports {
        Value::Object(map) if map.keys().any(|key| key.starts_with('.')) => {
            if let Some(value) = map.get(subpath) {
                return resolve_target(value, None, custom);
            }
            let mut best: Option<(usize, &Value, &str)> = None;
            for (key, value) in map {
                let Some((prefix, suffix)) = key.split_once('*') else {
                    continue;
                };
                let fits = subpath.len() >= prefix.len() + suffix.len()
                    && subpath.starts_with(prefix)
                    && subpath.ends_with(suffix);
                if fits && best.is_none_or(|(length, _, _)| prefix.len() > length) {
                    best = Some((
                        prefix.len(),
                        value,
                        &subpath[prefix.len()..subpath.len() - suffix.len()],
                    ));
                }
            }
            best.and_then(|(_, value, matched)| resolve_target(value, Some(matched), custom))
        }
        Value::String(_) | Value::Array(_) | Value::Object(_) => {
            if subpath == "." {
                resolve_target(exports, None, custom)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;
    use skif_core::MemoryFs;

    use super::*;

    #[test]
    fn dist_maps_to_src() {
        let candidates = source_candidates("./dist/utils/index.js");
        assert_eq!(candidates.first().map(String::as_str), Some("src/utils/index.ts"));
        assert!(candidates.contains(&"dist/utils/index.js".to_owned()));
        assert_eq!(source_candidates("./styles.css"), ["styles.css"]);
    }

    #[test]
    fn declaration_maps_to_ts() {
        let candidates = source_candidates("./dist/index.d.ts");
        assert_eq!(candidates.first().map(String::as_str), Some("src/index.ts"));
    }

    #[test]
    fn maps_to_existing_source() {
        let fs = MemoryFs::new().with_file("/p/src/index.ts", "");
        assert_eq!(
            map_to_source(&fs, Path::new("/p"), "./dist/index.js"),
            Some(PathBuf::from("/p/src/index.ts"))
        );
        assert_eq!(map_to_source(&fs, Path::new("/p"), "./dist/missing.js"), None);
    }

    #[test]
    fn exports_resolution() {
        let exports = json!({
            ".": { "types": "./dist/index.d.ts", "import": "./dist/index.js" },
            "./feature/*": "./dist/feature/*.js",
            "./internal": null,
        });
        assert_eq!(resolve_exports(&exports, ".", &[]).as_deref(), Some("./dist/index.d.ts"));
        assert_eq!(
            resolve_exports(&exports, "./feature/a", &[]).as_deref(),
            Some("./dist/feature/a.js")
        );
        assert_eq!(resolve_exports(&exports, "./missing", &[]), None);
        assert_eq!(resolve_exports(&json!("./x.js"), ".", &[]).as_deref(), Some("./x.js"));
        assert_eq!(
            resolve_exports(&json!({"import": "./i.js"}), ".", &[]).as_deref(),
            Some("./i.js")
        );
    }

    #[test]
    fn source_and_custom_conditions_select_source_files() {
        let exports = json!({ ".": { "source": "./src/index.ts", "import": "./dist/index.js" } });
        assert_eq!(
            resolve_exports(&exports, ".", &[]).as_deref(),
            Some("./src/index.ts")
        );
        let custom = json!({ ".": { "monorepo": "./lib/a.ts", "default": "./dist/a.js" } });
        assert_eq!(resolve_exports(&custom, ".", &[]).as_deref(), Some("./dist/a.js"));
        assert_eq!(
            resolve_exports(&custom, ".", &["monorepo".to_owned()]).as_deref(),
            Some("./lib/a.ts")
        );
    }

    #[test]
    fn manifest_targets_cover_every_field() {
        let manifest = json!({
            "main": "dist/index.js",
            "bin": { "tool": "bin/tool.js" },
            "exports": { ".": "./dist/index.js", "./a": { "import": "./dist/a.js" } },
        });
        let targets = manifest_targets(&manifest);
        for expected in ["dist/index.js", "bin/tool.js", "./dist/index.js", "./dist/a.js"] {
            assert!(targets.iter().any(|t| t == expected), "{expected}");
        }
    }

    #[test]
    fn patterns_for_wildcards() {
        let patterns = source_patterns("./dist/feature/*.js");
        assert_eq!(patterns[0], "src/feature/*.{ts,tsx,js,jsx,mts,cts,mjs,cjs}");
    }
}
