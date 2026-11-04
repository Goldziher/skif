//! Helpers shared by the manifest rules: locations, fix operations and package selection.

use serde_json::{Map, Value};
use skif_core::{DependencyKind, FixOp, InsertPosition, JsonAction, Location, PathSeg};
use skif_workspace::{DependencyEntry, Package, Workspace};

pub fn key(name: &str) -> PathSeg {
    PathSeg::Key(name.to_owned())
}

/// Pointer to a dependency section, e.g. `["devDependencies"]`.
pub fn section(kind: DependencyKind) -> Vec<PathSeg> {
    vec![key(kind.field())]
}

/// Every package the workspace does not ignore, the root first.
pub fn active_packages(workspace: &Workspace) -> impl Iterator<Item = &Package> {
    workspace
        .iter()
        .filter(move |(id, _)| !workspace.is_ignored(*id))
        .map(|(_, package)| package)
}

/// Every member package the workspace does not ignore.
pub fn active_members(workspace: &Workspace) -> impl Iterator<Item = &Package> {
    active_packages(workspace).filter(|package| !package.is_root)
}

/// The node at `pointer` in the package's manifest.
pub fn field_location(package: &Package, pointer: &[PathSeg]) -> Location {
    Location::new(
        package.manifest.path.clone(),
        package.manifest.field_span(pointer),
    )
}

/// The opening brace of the manifest: the anchor for a field that is missing.
pub fn document_location(package: &Package) -> Location {
    let span = package.manifest.field_span(&[]).map(|root| {
        package
            .manifest
            .line_index()
            .span(root.start, root.start.saturating_add(1))
    });
    Location::new(
        package.manifest.path.clone(),
        span,
    )
}

/// A dependency entry from its key through its value.
pub fn entry_location(package: &Package, entry: &DependencyEntry) -> Location {
    Location::new(
        package.manifest.path.clone(),
        Some(
            package
                .manifest
                .line_index()
                .span(entry.key_span.start, entry.value_span.end),
        ),
    )
}

/// The specifier string of a dependency entry.
pub fn value_location(package: &Package, entry: &DependencyEntry) -> Location {
    Location::new(
        package.manifest.path.clone(),
        Some(entry.value_span),
    )
}

/// A JSON edit of the package's own manifest.
pub fn json_op(package: &Package, pointer: Vec<PathSeg>, action: JsonAction) -> FixOp {
    FixOp::json(
        package.manifest.path.clone(),
        pointer,
        action,
    )
}

/// A dependency map with its keys in byte order.
fn sorted(map: Map<String, Value>) -> Map<String, Value> {
    let mut entries: Vec<(String, Value)> = map.into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.into_iter().collect()
}

/// Operations moving the named `dependencies` entries into `devDependencies`, keeping every specifier as written.
///
/// `None` when there is nothing to move, when `devDependencies` is not an object, or when `devDependencies` already
/// holds one of the names with a different specifier (the move would silently drop one of the two).
pub fn move_to_dev_dependencies(package: &Package, names: &[&str]) -> Option<Vec<FixOp>> {
    let prod = package.dependency_map(DependencyKind::Dependencies)?;
    let moved: Map<String, Value> = names
        .iter()
        .filter_map(|name| prod.get(*name).map(|value| ((*name).to_owned(), value.clone())))
        .collect();
    if moved.is_empty() {
        return None;
    }
    let all = moved.len() == prod.len();
    let dev_field = DependencyKind::DevDependencies.field();
    let dev = package.dependency_map(DependencyKind::DevDependencies);
    if dev.is_none() && package.manifest.value().get(dev_field).is_some() {
        return None;
    }
    let mut ops = Vec::new();
    match dev {
        None if all => {
            return Some(vec![json_op(
                package,
                section(DependencyKind::Dependencies),
                JsonAction::RenameKey(dev_field.to_owned()),
            )]);
        }
        None => ops.push(json_op(
            package,
            Vec::new(),
            JsonAction::insert(
                dev_field.to_owned(),
                Value::Object(sorted(moved.clone())),
                InsertPosition::End,
            ),
        )),
        Some(dev) => {
            if moved
                .iter()
                .any(|(name, value)| dev.get(name).is_some_and(|existing| existing != value))
            {
                return None;
            }
            if dev.is_empty() {
                ops.push(json_op(
                    package,
                    section(DependencyKind::DevDependencies),
                    JsonAction::Set(Value::Object(sorted(moved.clone()))),
                ));
            } else {
                ops.extend(
                    moved
                        .iter()
                        .filter(|(name, _)| !dev.contains_key(*name))
                        .map(|(name, value)| {
                            json_op(
                                package,
                                section(DependencyKind::DevDependencies),
                                JsonAction::insert(
                                    name.clone(),
                                    value.clone(),
                                    InsertPosition::Sorted,
                                ),
                            )
                        }),
                );
            }
        }
    }
    if all {
        ops.push(json_op(
            package,
            section(DependencyKind::Dependencies),
            JsonAction::Remove,
        ));
    } else {
        ops.extend(moved.keys().map(|name| {
            json_op(
                package,
                vec![key(DependencyKind::Dependencies.field()), key(name)],
                JsonAction::Remove,
            )
        }));
    }
    Some(ops)
}
