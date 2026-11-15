//! Telling a `package.json` that makes a package from one that only marks the files below it.

use std::path::Path;

use skif_core::FileSystem;

/// Fields a `package.json` may hold and still be only a marker for the files below it (module type, side effects,
/// subpath imports), not a package of its own.
const MARKER_FIELDS: &[&str] = &["type", "sideEffects", "imports"];

/// The entry fields of a subpath directory written before `exports` (`pkg/server/package.json` holding
/// `{"main": "index.js"}`), through which `pkg/server` resolves to a file of `pkg`.
const SUBPATH_FIELDS: &[&str] = &["main", "module", "types", "typings", "browser"];

/// Whether `dir/package.json` makes a package of its own.
///
/// A manifest whose fields are all marker fields (`type`, `sideEffects`, `imports`; `{}` too) only marks the files
/// below it, and so does one that adds only subpath entry fields (`main`, `module`, `types`, `typings`, `browser`) in
/// a directory the `package.json` of the parent directory publishes: a `./<dir>` key of its `exports`, or `<dir>`,
/// `<dir>/`, `<dir>/*` or `<dir>/**` in its `files`. Their files belong to the package above. A missing, unreadable
/// or invalid manifest, or one that is not a JSON object, counts as a package.
#[must_use]
pub fn is_package_manifest(fs: &dyn FileSystem, dir: &Path) -> bool {
    let Ok(text) = fs.read_to_string(&dir.join("package.json")) else {
        return true;
    };
    let Ok(serde_json::Value::Object(fields)) = serde_json::from_str::<serde_json::Value>(&text) else {
        return true;
    };
    let own: Vec<&str> = fields
        .keys()
        .map(String::as_str)
        .filter(|key| !MARKER_FIELDS.contains(key))
        .collect();
    if own.is_empty() {
        return false;
    }
    !(own.iter().all(|key| SUBPATH_FIELDS.contains(key)) && is_published_subpath(fs, dir))
}

/// Whether the package in the parent directory of `dir` publishes `dir` as a subpath of its own
/// ([`published_subpaths`]).
fn is_published_subpath(fs: &dyn FileSystem, dir: &Path) -> bool {
    let (Some(parent), Some(name)) = (dir.parent(), dir.file_name().and_then(|name| name.to_str())) else {
        return false;
    };
    let Ok(text) = fs.read_to_string(&parent.join("package.json")) else {
        return false;
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    published_subpaths(&manifest).any(|subpath| subpath == name)
}

/// The child directories `manifest` publishes as subpaths of its package: the `./<name>` keys of its `exports` and
/// the directories its `files` lists (`server`, `server/`, `server/*`, `server/**`).
fn published_subpaths(manifest: &serde_json::Value) -> impl Iterator<Item = &str> {
    let exported = manifest
        .get("exports")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flat_map(serde_json::Map::keys)
        .filter_map(|key| key.strip_prefix("./"));
    let listed = manifest
        .get("files")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(|file| {
            let file = file.trim_start_matches("./");
            file.strip_suffix("/**")
                .or_else(|| file.strip_suffix("/*"))
                .unwrap_or(file)
                .trim_end_matches('/')
        });
    exported
        .chain(listed)
        .filter(|name| !name.is_empty() && !name.contains(['/', '*']))
}

/// The paths, relative to the package directory `dir`, that the subpath directories the package's `manifest`
/// publishes point at through a `package.json` of their own that makes no package ([`is_package_manifest`]): the
/// `main`, `module`, `types`, `typings` and `browser` of `server/package.json` (`./server/index.js`), which load when
/// `pkg/server` is imported.
#[must_use]
pub fn subpath_entries(fs: &dyn FileSystem, dir: &Path, manifest: &serde_json::Value) -> Vec<String> {
    let mut entries = Vec::new();
    for name in published_subpaths(manifest) {
        let subdir = dir.join(name);
        if is_package_manifest(fs, &subdir) {
            continue;
        }
        let Some(nested) = fs
            .read_to_string(&subdir.join("package.json"))
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        else {
            continue;
        };
        for path in SUBPATH_FIELDS
            .iter()
            .filter_map(|field| nested.get(field).and_then(serde_json::Value::as_str))
        {
            let entry = format!("./{name}/{}", path.trim_start_matches("./"));
            if !entries.contains(&entry) {
                entries.push(entry);
            }
        }
    }
    entries
}
