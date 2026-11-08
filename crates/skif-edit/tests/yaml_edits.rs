//! YAML edit tests: comments and layout outside the change survive.

#![cfg(feature = "yaml")]

mod common;

use common::yaml_op;
use serde_json::json;
use skif_core::{FixOp, YamlAction};
use skif_edit::{EditPlanError, FileKind, apply, plan_edits};

const WORKSPACE: &str = "# workspace\npackages:\n  - 'apps/*' # apps\n  - 'libs/*'\n\ncatalog:\n  react: ^18.0.0 # pinned\n  vue: ^3.0.0\n# end\n";

fn run(cases: Vec<(&str, Vec<FixOp>, &str)>) {
    for (name, ops, expected) in cases {
        let refs: Vec<&FixOp> = ops.iter().collect();
        let actual = apply(WORKSPACE, FileKind::Yaml, &refs).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(actual, expected, "case `{name}`");
    }
}

#[test]
fn yaml_edits_keep_comments() {
    run(vec![
        (
            "set existing value",
            vec![yaml_op(&["catalog", "react"], YamlAction::Set(json!("^19.0.0")))],
            "# workspace\npackages:\n  - 'apps/*' # apps\n  - 'libs/*'\n\ncatalog:\n  react: ^19.0.0 # pinned\n  vue: ^3.0.0\n# end\n",
        ),
        (
            "set adds a missing key",
            vec![yaml_op(&["catalog", "zod"], YamlAction::Set(json!("^3.0.0")))],
            "# workspace\npackages:\n  - 'apps/*' # apps\n  - 'libs/*'\n\ncatalog:\n  react: ^18.0.0 # pinned\n  vue: ^3.0.0\n  zod: ^3.0.0\n# end\n",
        ),
        (
            "remove a key",
            vec![yaml_op(&["catalog", "vue"], YamlAction::Remove)],
            "# workspace\npackages:\n  - 'apps/*' # apps\n  - 'libs/*'\n\ncatalog:\n  react: ^18.0.0 # pinned\n# end\n",
        ),
        (
            "remove a sequence item",
            vec![yaml_op(&["packages", "#1"], YamlAction::Remove)],
            "# workspace\npackages:\n  - 'apps/*' # apps\n\ncatalog:\n  react: ^18.0.0 # pinned\n  vue: ^3.0.0\n# end\n",
        ),
        (
            "append to a sequence",
            vec![yaml_op(&["packages"], YamlAction::Append(json!("tools/*")))],
            "# workspace\npackages:\n  - 'apps/*' # apps\n  - 'libs/*'\n  - tools/*\n\ncatalog:\n  react: ^18.0.0 # pinned\n  vue: ^3.0.0\n# end\n",
        ),
        (
            "several ops see each other's results",
            vec![
                yaml_op(&["catalog", "react"], YamlAction::Set(json!("^19.0.0"))),
                yaml_op(&["catalog", "vue"], YamlAction::Remove),
                yaml_op(&["packages"], YamlAction::Append(json!("tools/*"))),
            ],
            "# workspace\npackages:\n  - 'apps/*' # apps\n  - 'libs/*'\n  - tools/*\n\ncatalog:\n  react: ^19.0.0 # pinned\n# end\n",
        ),
    ]);
}

#[test]
fn removing_a_key_deletes_whole_lines() {
    let source = "packages:\n  - a\ncatalog:\n  x: ^1.0.0\ncatalogs:\n  old:\n    y: ^1.0.0\n";
    let deleted = |route: &[&str]| {
        let edits = plan_edits(source, FileKind::Yaml, &[&yaml_op(route, YamlAction::Remove)]).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].replacement, "");
        source[edits[0].start as usize..edits[0].end as usize].to_owned()
    };
    // `catalog` is followed by `catalogs`, so the deleted text could also start inside the line; it must not, or it
    // would overlap the removal of `catalogs`.
    assert_eq!(deleted(&["catalog"]), "catalog:\n  x: ^1.0.0\n");
    assert_eq!(deleted(&["catalogs"]), "catalogs:\n  old:\n    y: ^1.0.0\n");
}

#[test]
fn the_plan_is_one_minimal_edit() {
    let op = yaml_op(&["catalog", "react"], YamlAction::Set(json!("^19.0.0")));
    let edits = plan_edits(WORKSPACE, FileKind::Yaml, &[&op]).unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].replacement, "9");
    let none = plan_edits(WORKSPACE, FileKind::Yaml, &[]).unwrap();
    assert_eq!(none.len(), 0);
}

#[test]
fn yaml_errors() {
    let missing = yaml_op(&["nope", "x"], YamlAction::Set(json!(1)));
    assert!(matches!(
        plan_edits(WORKSPACE, FileKind::Yaml, &[&missing]).unwrap_err(),
        EditPlanError::Yaml { .. }
    ));
    let root = yaml_op(&[], YamlAction::Set(json!(1)));
    assert!(matches!(
        plan_edits(WORKSPACE, FileKind::Yaml, &[&root]).unwrap_err(),
        EditPlanError::Yaml { .. }
    ));
    let json = common::set(&["a"], json!(1));
    assert!(matches!(
        plan_edits(WORKSPACE, FileKind::Yaml, &[&json]).unwrap_err(),
        EditPlanError::Unsupported { .. }
    ));
    assert!(matches!(
        plan_edits("a: [", FileKind::Yaml, &[&missing]).unwrap_err(),
        EditPlanError::Parse { .. }
    ));
}

const SEQUENCE: &str = "items:\n  - a\n  - b\n  - c\n  - d\n  - e\nother: 1\n";

#[test]
fn removals_in_one_sequence_address_the_original_document() {
    let cases: [(&str, Vec<FixOp>, &str); 6] = [
        (
            "two removals ascending",
            vec![
                yaml_op(&["items", "#0"], YamlAction::Remove),
                yaml_op(&["items", "#2"], YamlAction::Remove),
            ],
            "items:\n  - b\n  - d\n  - e\nother: 1\n",
        ),
        (
            "two removals descending",
            vec![
                yaml_op(&["items", "#2"], YamlAction::Remove),
                yaml_op(&["items", "#0"], YamlAction::Remove),
            ],
            "items:\n  - b\n  - d\n  - e\nother: 1\n",
        ),
        (
            "three removals including the last",
            vec![
                yaml_op(&["items", "#1"], YamlAction::Remove),
                yaml_op(&["items", "#3"], YamlAction::Remove),
                yaml_op(&["items", "#4"], YamlAction::Remove),
            ],
            "items:\n  - a\n  - c\nother: 1\n",
        ),
        (
            "adjacent removals",
            vec![
                yaml_op(&["items", "#1"], YamlAction::Remove),
                yaml_op(&["items", "#2"], YamlAction::Remove),
            ],
            "items:\n  - a\n  - d\n  - e\nother: 1\n",
        ),
        (
            "set after a removal targets the original element",
            vec![
                yaml_op(&["items", "#0"], YamlAction::Remove),
                yaml_op(&["items", "#3"], YamlAction::Set(json!("D"))),
            ],
            "items:\n  - b\n  - c\n  - D\n  - e\nother: 1\n",
        ),
        (
            "removing a key does not shift indices",
            vec![
                yaml_op(&["other"], YamlAction::Remove),
                yaml_op(&["items", "#4"], YamlAction::Remove),
            ],
            "items:\n  - a\n  - b\n  - c\n  - d\n",
        ),
    ];
    for (name, ops, expected) in cases {
        let refs: Vec<&FixOp> = ops.iter().collect();
        let actual = apply(SEQUENCE, FileKind::Yaml, &refs).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(actual, expected, "case `{name}`");
    }
}

#[test]
fn removing_the_same_element_twice_is_an_error() {
    let first = yaml_op(&["items", "#1"], YamlAction::Remove);
    let again = yaml_op(&["items", "#1"], YamlAction::Remove);
    assert!(matches!(
        plan_edits(SEQUENCE, FileKind::Yaml, &[&first, &again]).unwrap_err(),
        EditPlanError::Yaml { .. }
    ));
}
