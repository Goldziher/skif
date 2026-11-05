//! Paths a `package.json` publishes and how built output maps back to source.

use std::path::{Path, PathBuf};

use serde_json::Value;
use skif_core::FileSystem;

use crate::resolve::{is_code_file, join_normalized};

/// Conditions an import from a package can match, in the order the Node.js and TypeScript resolvers apply them.
const CONDITIONS: &[&str] = &["import", "require", "node", "default", "types"];

/// Directories that hold build output; their source twin is `src`. A suffix after `-`, `_` or `.` names the flavour
/// (`lib-commonjs`, `dist-esm`, `esm5`, `lib.es6`).
const OUT_DIRS: &[&str] = &[
    "dist", "build", "lib", "out", "esm", "cjs", "esm5", "esm2015", "es", "es5", "es6", "es2015", "es2017", "es2020",
    "commonjs", "umd", "amd", "types",
];

/// Whether `directory`, the first segment of a published path, is build output.
pub(crate) fn is_out_dir(directory: &str) -> bool {
    let base = directory.split(['-', '_', '.']).next().unwrap_or(directory);
    OUT_DIRS.contains(&base)
}

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
        && is_out_dir(first)
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
        && is_out_dir(first)
    {
        patterns.push(format!("src/{rest}.{extensions}"));
    }
    patterns.push(format!("{stem}.{extensions}"));
    patterns
}

/// Widens the `*` of an `exports` leaf the way the `exports` field matches it, where `*` also matches `/`: a trailing
/// `/*` becomes `/**`, the first `/*.` becomes `/**/*.` and the first `/*/` becomes `/**/`. Other patterns are kept.
pub(crate) fn crossing_stars(pattern: &str) -> String {
    let padded = format!("/{pattern}");
    let widened = if let Some(head) = padded.strip_suffix("/*") {
        format!("{head}/**")
    } else if let Some(at) = padded.find("/*.") {
        format!("{}/**/*.{}", &padded[..at], &padded[at + 3..])
    } else if let Some(at) = padded.find("/*/") {
        format!("{}/**/{}", &padded[..at], &padded[at + 3..])
    } else {
        padded
    };
    widened.strip_prefix('/').unwrap_or(&widened).to_owned()
}

/// The files a bin stub loads. A published `bin` file without a script extension (`bin/tool`, a `#!/usr/bin/env node`
/// stub) is not a module, so the relative specifiers it `require`s or imports are read from its text and mapped back to
/// source like any published path.
pub(crate) fn bin_stub_sources(fs: &dyn FileSystem, package_dir: &Path, target: &str) -> Vec<PathBuf> {
    let stub = package_dir.join(target.trim_start_matches("./"));
    if is_code_file(&stub) {
        return Vec::new();
    }
    let Ok(text) = fs.read_to_string(&stub) else {
        return Vec::new();
    };
    if !text.starts_with("#!") || !text.lines().next().is_some_and(|line| line.contains("node")) {
        return Vec::new();
    }
    let directory = stub.parent().unwrap_or(package_dir);
    relative_specifiers(&text)
        .into_iter()
        .filter_map(|specifier| {
            let loaded = join_normalized(directory, &specifier);
            let published = loaded
                .strip_prefix(package_dir)
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            map_to_source(fs, package_dir, &published)
                .or_else(|| map_to_source(fs, package_dir, &format!("{published}/index")))
        })
        .collect()
}

/// The `./` and `../` string literals that follow `require(`, `import(`, `import` or `from` in `text`.
fn relative_specifiers(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for marker in ["require(", "import(", "import ", "from "] {
        for (at, _) in text.match_indices(marker) {
            let rest = text[at + marker.len()..].trim_start();
            let Some(quote) = rest.chars().next().filter(|c| matches!(*c, '\'' | '"' | '`')) else {
                continue;
            };
            let body = &rest[1..];
            if let Some(end) = body.find(quote) {
                let specifier = &body[..end];
                if specifier.starts_with("./") || specifier.starts_with("../") {
                    found.push(specifier.to_owned());
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// The values of the `bin` field of a manifest.
pub(crate) fn bin_targets(manifest: &Value) -> Vec<String> {
    let mut targets = Vec::new();
    if let Some(bin) = manifest.get("bin") {
        collect_strings(bin, &mut targets);
    }
    targets
}

/// Whether a published path is build output or a bin file, i.e. it should have a source file behind it.
pub(crate) fn should_have_source(target: &str) -> bool {
    let clean = target.trim_start_matches("./");
    !has_other_extension(clean)
        && clean
            .split_once('/')
            .is_some_and(|(first, _)| is_out_dir(first) || first == "bin")
}

fn collect_strings(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(text) if !text.is_empty() => out.push(text.clone()),
        Value::Array(items) => items.iter().for_each(|item| collect_strings(item, out)),
        Value::Object(map) => map.values().for_each(|item| collect_strings(item, out)),
        _ => {}
    }
}

/// A file path a manifest publishes and whether it is a leaf of the `exports` field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestTarget {
    pub(crate) path: String,
    pub(crate) from_exports: bool,
}

/// Every file path a manifest publishes: `main`, `module`, `types`, `bin` and the leaves of `exports`. The catch-all
/// `./*` export is not a target.
pub(crate) fn manifest_targets(manifest: &Value) -> Vec<ManifestTarget> {
    let mut paths = Vec::new();
    for field in ENTRY_FIELDS {
        if let Some(Value::String(text)) = manifest.get(*field) {
            paths.push(text.clone());
        }
    }
    if let Some(Value::String(text)) = manifest.get("browser") {
        paths.push(text.clone());
    }
    if let Some(bin) = manifest.get("bin") {
        collect_strings(bin, &mut paths);
    }
    let mut targets: Vec<ManifestTarget> = paths
        .into_iter()
        .map(|path| ManifestTarget {
            path,
            from_exports: false,
        })
        .collect();
    if let Some(exports) = manifest.get("exports") {
        let mut leaves = Vec::new();
        collect_strings(exports, &mut leaves);
        targets.extend(
            leaves
                .into_iter()
                .filter(|leaf| leaf.starts_with("./") && leaf != "./*")
                .map(|path| ManifestTarget {
                    path,
                    from_exports: true,
                }),
        );
    }
    targets.retain(|target| !target.path.is_empty());
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
    fn flavoured_output_directories_map_to_src() {
        for directory in ["lib-commonjs", "lib-esm", "dist-cjs", "esm5", "lib.es6"] {
            let candidates = source_candidates(&format!("./{directory}/start.js"));
            assert_eq!(
                candidates.first().map(String::as_str),
                Some("src/start.ts"),
                "{directory}"
            );
        }
        assert_eq!(source_candidates("./library/start.js")[0], "library/start.ts");
    }

    #[test]
    fn bin_stub_requires_map_to_source() {
        let fs = MemoryFs::new()
            .with_file(
                "/p/bin/tool",
                "#!/usr/bin/env node\nrequire('../lib-commonjs/start');\n",
            )
            .with_file("/p/src/start.ts", "");
        assert_eq!(
            bin_stub_sources(&fs, Path::new("/p"), "./bin/tool"),
            [PathBuf::from("/p/src/start.ts")]
        );
        let not_node = MemoryFs::new().with_file("/p/bin/tool", "#!/bin/sh\nrequire('../lib/start');\n");
        assert_eq!(
            bin_stub_sources(&not_node, Path::new("/p"), "bin/tool"),
            [] as [std::path::PathBuf; 0]
        );
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
        assert_eq!(
            resolve_exports(&exports, ".", &[]).as_deref(),
            Some("./dist/index.d.ts")
        );
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
        assert_eq!(resolve_exports(&exports, ".", &[]).as_deref(), Some("./src/index.ts"));
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
            assert!(targets.iter().any(|t| t.path == expected), "{expected}");
        }
        let from_exports: Vec<&str> = targets
            .iter()
            .filter(|t| t.from_exports)
            .map(|t| t.path.as_str())
            .collect();
        assert_eq!(from_exports, vec!["./dist/index.js", "./dist/a.js"]);
    }

    #[test]
    fn catch_all_export_is_not_a_target() {
        let manifest = json!({ "exports": { "./*": "./*", "./a/*": "./a/*.js" } });
        let paths: Vec<String> = manifest_targets(&manifest).into_iter().map(|t| t.path).collect();
        assert_eq!(paths, vec!["./a/*.js".to_owned()]);
    }

    #[test]
    fn patterns_for_wildcards() {
        let patterns = source_patterns("./dist/feature/*.js");
        assert_eq!(patterns[0], "src/feature/*.{ts,tsx,js,jsx,mts,cts,mjs,cjs}");
    }

    #[test]
    fn exports_stars_cross_directories() {
        assert_eq!(crossing_stars("src/*.ts"), "src/**/*.ts");
        assert_eq!(crossing_stars("*.ts"), "**/*.ts");
        assert_eq!(crossing_stars("dist/*/index.js"), "dist/**/index.js");
        assert_eq!(crossing_stars("dist/*"), "dist/**");
        assert_eq!(crossing_stars("src/index.ts"), "src/index.ts");
        assert_eq!(crossing_stars("src/foo-*.ts"), "src/foo-*.ts");
        assert_eq!(crossing_stars("a/*/b/*/c.js"), "a/**/b/*/c.js");
    }
}
