//! Instances read from manifest fields that are not dependency sections: the override sections (`resolutions`,
//! `overrides`, `pnpm.overrides`, `overrides` of `pnpm-workspace.yaml`) and the fields `[[custom-types]]` name.
//!
//! Each string version found there is an instance like a dependency entry, so version groups and semver groups
//! govern it, and its fix edits the field in place (JSON through the manifest, YAML through the workspace file).

use std::path::PathBuf;

use serde_json::{Map, Value};
use skif_config::{CustomStrategy, CustomType, DependencyType, PackageManager};
use skif_core::{FixOp, JsonAction, Location, PathSeg, YamlAction};
use skif_workspace::{Package, Workspace};

use crate::manifest::support;

/// Where a field instance lives and how a new version is written back.
#[derive(Debug, Clone)]
pub(crate) struct Field {
    /// The `pnpm-workspace.yaml` the version is in; `None` for the package's own manifest.
    yaml: Option<PathBuf>,
    /// The path to the version: JSON keys in a manifest, the route in a YAML file.
    pointer: Vec<PathSeg>,
    /// The text before the version in the string value, e.g. `pnpm@` for `"packageManager": "pnpm@9.0.0"`.
    prefix: String,
    location: Location,
}

impl Field {
    pub(crate) fn location(&self) -> Location {
        self.location.clone()
    }

    /// The JSON pointer of the version in the manifest; `None` when it is in a YAML file.
    pub(crate) fn json_pointer(&self) -> Option<Vec<PathSeg>> {
        self.yaml.is_none().then(|| self.pointer.clone())
    }

    /// The operation writing `specifier` as the version.
    pub(crate) fn set(&self, package: &Package, specifier: &str) -> FixOp {
        let value = Value::String(format!("{}{specifier}", self.prefix));
        match &self.yaml {
            Some(path) => FixOp::yaml(path.clone(), self.pointer.clone(), YamlAction::Set(value)),
            None => support::json_op(package, self.pointer.clone(), JsonAction::Set(value)),
        }
    }

    /// The operation removing the version (the whole field for a `name@version` string).
    pub(crate) fn remove(&self, package: &Package) -> FixOp {
        match &self.yaml {
            Some(path) => FixOp::yaml(path.clone(), self.pointer.clone(), YamlAction::Remove),
            None => support::json_op(package, self.pointer.clone(), JsonAction::Remove),
        }
    }
}

/// A version found in a field, before it becomes an instance.
#[derive(Debug)]
pub(crate) struct Found {
    pub(crate) ty: DependencyType,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) field: Field,
}

/// The dependency an override selector replaces: the package name of its last segment, without the version selector.
///
/// Handles `name`, `name@range` (the range may start with an operator: `react@>=18`), pnpm's `parent>child` and
/// `parent@1>child@2`, yarn's `**/name` and `parent/**/name`, and scoped names (`@scope/name@^1`).
pub(crate) fn override_name(selector: &str) -> &str {
    let last = after_last_parent_separator(selector);
    let name = &last[..version_start(last)];
    last_package_of_path(name)
}

/// The part of `selector` after its last `>` that separates a parent from a child. A `>` that follows `@`, a space or
/// another operator belongs to a version range, as in `react@>=18` or `react@>1 <3`.
fn after_last_parent_separator(selector: &str) -> &str {
    let mut start = 0;
    let mut previous = None;
    for (index, c) in selector.char_indices() {
        if c == '>' && previous.is_some_and(|before| !matches!(before, '@' | ' ' | '<' | '>' | '=' | '|')) {
            start = index + 1;
        }
        previous = Some(c);
    }
    &selector[start..]
}

/// Where the version selector of `segment` begins: its first `@` that does not start a scope.
fn version_start(segment: &str) -> usize {
    segment
        .char_indices()
        .find(|&(index, c)| c == '@' && index > 0 && !segment[..index].ends_with('/'))
        .map_or(segment.len(), |(index, _)| index)
}

/// The last package of a yarn resolution path such as `**/name`, `parent/**/name` or `parent/@scope/name`.
fn last_package_of_path(path: &str) -> &str {
    let (mut start, mut offset, mut scope_open) = (0, 0, false);
    for piece in path.split('/') {
        if scope_open {
            scope_open = false;
        } else {
            start = offset;
            scope_open = piece.starts_with('@');
        }
        offset += piece.len() + 1;
    }
    &path[start..]
}

/// Whether an override value points at something else instead of naming a version: npm's `$name` (the version of the
/// direct dependency `name`) and pnpm's `-` (remove the override).
fn is_reference(value: &str) -> bool {
    value.starts_with('$') || value == "-"
}

/// Splits `name@version`; the name of a scoped package starts with an `@` of its own.
fn split_name_version(value: &str) -> Option<(&str, &str)> {
    let at = value.get(1..)?.rfind('@')? + 1;
    let (name, version) = (&value[..at], &value[at + 1..]);
    (!name.is_empty() && !version.is_empty()).then_some((name, version))
}

fn key(name: &str) -> PathSeg {
    PathSeg::Key(name.to_owned())
}

/// The location of the node at `pointer`, or of the deepest enclosing node the span table records.
fn location_of(package: &Package, pointer: &[PathSeg]) -> Location {
    let span = (1..=pointer.len())
        .rev()
        .find_map(|length| package.manifest.field_span(&pointer[..length]));
    Location::new(package.manifest.path.clone(), span)
}

/// Every version `package` declares in an override section or a custom type, and, for the root package, in the
/// `overrides` of `pnpm-workspace.yaml`.
pub(crate) fn collect(workspace: &Workspace, package: &Package, custom: &[CustomType]) -> Vec<Found> {
    let root = package.manifest.value();
    let mut found = Vec::new();
    let sections: [(DependencyType, &[&str]); 3] = [
        (DependencyType::Resolutions, &["resolutions"][..]),
        (DependencyType::Overrides, &["overrides"][..]),
        (DependencyType::PnpmOverrides, &["pnpm", "overrides"][..]),
    ];
    for (ty, path) in sections {
        let Some(entries) = path
            .iter()
            .try_fold(root, |value, segment| value.get(*segment))
            .and_then(Value::as_object)
        else {
            continue;
        };
        let mut pointer: Vec<PathSeg> = path.iter().map(|segment| key(segment)).collect();
        walk_overrides(package, &ty, entries, &mut pointer, None, &mut found);
    }
    // Only pnpm reads the `overrides` of `pnpm-workspace.yaml`: a stale file in a Yarn or npm repository declares none.
    let reads_pnpm_workspace = matches!(workspace.package_manager, None | Some(PackageManager::Pnpm));
    if package.is_root
        && reads_pnpm_workspace
        && let Some(yaml) = &workspace.pnpm_workspace
    {
        for (selector, version) in &yaml.overrides {
            found.push(Found {
                ty: DependencyType::PnpmOverrides,
                name: override_name(selector).to_owned(),
                version: version.clone(),
                field: Field {
                    yaml: Some(yaml.path.clone()),
                    pointer: vec![key("overrides"), key(selector)],
                    prefix: String::new(),
                    location: Location::new(yaml.path.clone(), yaml.override_span(selector)),
                },
            });
        }
    }
    for definition in custom {
        collect_custom(package, definition, &mut found);
    }
    found
}

/// Adds the string entries of `entries`, descending into the nested objects npm uses to scope an override to a
/// parent. The `.` entry of a nested object overrides the parent itself.
fn walk_overrides(
    package: &Package,
    ty: &DependencyType,
    entries: &Map<String, Value>,
    pointer: &mut Vec<PathSeg>,
    parent: Option<&str>,
    found: &mut Vec<Found>,
) {
    for (selector, value) in entries {
        pointer.push(key(selector));
        let name = if selector == "." {
            parent
        } else {
            Some(override_name(selector))
        };
        match (value, name) {
            (Value::String(version), Some(name)) if !is_reference(version) => found.push(Found {
                ty: ty.clone(),
                name: name.to_owned(),
                version: version.clone(),
                field: Field {
                    yaml: None,
                    pointer: pointer.clone(),
                    prefix: String::new(),
                    // The `.` entry has no name of its own: it is located at the entry of its parent.
                    location: location_of(package, &pointer[..pointer.len() - usize::from(selector == ".")]),
                },
            }),
            (Value::Object(nested), _) => walk_overrides(package, ty, nested, pointer, name, found),
            _ => {}
        }
        pointer.pop();
    }
}

fn collect_custom(package: &Package, definition: &CustomType, found: &mut Vec<Found>) {
    let Some(target) = definition
        .segments()
        .try_fold(package.manifest.value(), |value, segment| value.get(segment))
    else {
        return;
    };
    let base: Vec<PathSeg> = definition.segments().map(key).collect();
    let ty = DependencyType::Custom(definition.name.clone());
    let mut push = |name: &str, version: &str, prefix: String, pointer: Vec<PathSeg>| {
        found.push(Found {
            ty: ty.clone(),
            name: name.to_owned(),
            version: version.to_owned(),
            field: Field {
                yaml: None,
                prefix,
                location: location_of(package, &pointer),
                pointer,
            },
        });
    };
    match (definition.strategy, target) {
        (CustomStrategy::Version, Value::String(version)) => push(&definition.name, version, String::new(), base),
        (CustomStrategy::NameAtVersion, Value::String(text)) => {
            if let Some((name, version)) = split_name_version(text) {
                push(name, version, format!("{name}@"), base);
            }
        }
        (CustomStrategy::VersionsByName, Value::Object(entries)) => {
            for (name, value) in entries {
                if let Value::String(version) = value {
                    let mut pointer = base.clone();
                    pointer.push(key(name));
                    push(name, version, String::new(), pointer);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selectors_name_the_dependency_they_replace() {
        for (selector, name) in [
            ("foo", "foo"),
            ("foo@^1", "foo"),
            ("@scope/foo", "@scope/foo"),
            ("@scope/foo@1.2.3", "@scope/foo"),
            ("parent>foo", "foo"),
            ("@a/parent@1>@b/foo@2", "@b/foo"),
            ("**/foo", "foo"),
            ("parent/**/foo", "foo"),
            ("react@>=18", "react"),
            ("react@>1.0.0 <3", "react"),
            ("react@^17 || >=18", "react"),
            ("@scope/name@^1", "@scope/name"),
            ("@scope/name@>=1", "@scope/name"),
            ("parent@1>child@2", "child"),
            ("parent@>=1>child@>=2", "child"),
            ("parent>@scope/child@^1", "@scope/child"),
            ("**/@scope/name", "@scope/name"),
            ("a/**/@scope/name@1", "@scope/name"),
            ("a/@scope/name", "@scope/name"),
            ("foo@npm:@scope/bar@1", "foo"),
            ("foo@github:user/repo", "foo"),
        ] {
            assert_eq!(override_name(selector), name, "{selector}");
        }
    }

    #[test]
    fn name_at_version_splits_at_the_last_at() {
        assert_eq!(split_name_version("pnpm@9.0.0"), Some(("pnpm", "9.0.0")));
        assert_eq!(
            split_name_version("pnpm@9.0.0+sha512.abc"),
            Some(("pnpm", "9.0.0+sha512.abc"))
        );
        assert_eq!(split_name_version("@scope/tool@1.0.0"), Some(("@scope/tool", "1.0.0")));
        for text in ["pnpm", "@scope/tool", "@1.0.0", "pnpm@", ""] {
            assert_eq!(split_name_version(text), None, "{text}");
        }
    }
}
