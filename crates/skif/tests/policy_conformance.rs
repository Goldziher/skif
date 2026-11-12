//! The policy and catalog fixtures of `fixtures/policy`, with every fix planned by [`skif::apply_fixes`].
//!
//! The harness is the one of `skif-rules` (`crates/skif-rules/tests/common`); only the planning differs. There each
//! fix of a file is planned together with the others, which hides edits that cannot be combined. Here every fix is
//! planned on its own and combined as the CLI does, so a fix the facade has to skip as conflicting fails the case.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../skif-rules/tests/common/mod.rs"]
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use skif::{Alternatives, FileChange, FixSafety, FixSelection, MemoryFs, Workspace, apply_fixes};
use skif_config::Config;
use skif_lint::{Registry, RunOptions, run};

use common::{CATALOG_GROUPS, JOINT_GROUPS, POLICY_GROUPS, Planned, rule_ids, run_groups_with};

const ROOT: &str = "/ws";

/// Plans the first fix of every diagnostic of `family` through the facade, at the `Unsafe` ceiling; the harness
/// compares which fixes are safe through `expected.json` (ADR 0009).
fn through_the_facade(
    files: &BTreeMap<String, String>,
    config: &Config,
    family: &BTreeSet<&'static str>,
) -> Result<Planned, String> {
    let mut fs = MemoryFs::new();
    for (path, content) in files {
        fs.insert(Path::new(ROOT).join(path), content);
    }
    let workspace =
        Workspace::discover_with_fs(Path::new(ROOT), config, &fs).map_err(|error| format!("workspace: {error}"))?;
    let registry = Registry::new(skif_rules::providers()).map_err(|error| error.to_string())?;
    let report =
        run(&registry, &workspace, config, &RunOptions::default()).map_err(|error| format!("lint: {error}"))?;
    let selection = FixSelection::new(FixSafety::Unsafe)
        .with_alternatives(Alternatives::First)
        .with_rules(family.iter().map(|id| (*id).to_owned()));
    let plan = apply_fixes(&workspace, &report, &selection, &fs).map_err(|error| error.to_string())?;
    let mut planned = Planned {
        changed: BTreeMap::new(),
        deleted: BTreeSet::new(),
        skipped: plan
            .skipped
            .iter()
            .map(|skipped| format!("{} {}: {:?}", skipped.rule.as_str(), skipped.message, skipped.reason))
            .collect(),
    };
    for change in &plan.changes {
        let name = change.path().to_string_lossy().replace('\\', "/");
        match change {
            FileChange::Modify { new_contents, .. } => {
                planned.changed.insert(name, new_contents.clone());
            }
            FileChange::Create { contents, .. } => {
                planned.changed.insert(name, contents.clone());
            }
            FileChange::Delete { .. } => {
                planned.deleted.insert(name);
            }
            other => return Err(format!("unsupported change {other:?}")),
        }
    }
    Ok(planned)
}

#[test]
fn policy_fixtures_conform_through_the_facade() {
    run_groups_with(
        POLICY_GROUPS,
        &rule_ids(&skif_rules::policy_provider()),
        &through_the_facade,
    );
}

#[test]
fn catalog_fixtures_conform_through_the_facade() {
    run_groups_with(
        CATALOG_GROUPS,
        &rule_ids(&skif_rules::catalog_provider()),
        &through_the_facade,
    );
}

#[test]
fn catalog_and_version_group_fixtures_conform_through_the_facade() {
    let mut rules = rule_ids(&skif_rules::catalog_provider());
    rules.extend(rule_ids(&skif_rules::policy_provider()));
    run_groups_with(JOINT_GROUPS, &rules, &through_the_facade);
}

/// Removes the banned `left-pad` and `lodash` from a package whose dependencies are `dependencies`.
fn remove_banned(dependencies: &str) -> Planned {
    const ROOT_MANIFEST: &str = "{\n  \"name\": \"monorepo\",\n  \"version\": \"0.0.0\",\n  \"private\": true,\n  \"packageManager\": \"pnpm@9.15.0\",\n  \"workspaces\": [\"packages/*\"]\n}\n";
    let app = format!(
        "{{\n  \"name\": \"app\",\n  \"version\": \"1.0.0\",\n  \"private\": true,\n  \"dependencies\": {{\n{dependencies}\n  }}\n}}\n"
    );
    let config =
        Config::from_toml_str("[[version-groups]]\ndependencies = [\"left-pad\", \"lodash\"]\npolicy = \"banned\"\n")
            .unwrap();
    let files = BTreeMap::from([
        ("package.json".to_owned(), ROOT_MANIFEST.to_owned()),
        ("packages/app/package.json".to_owned(), app),
    ]);
    let planned = through_the_facade(&files, &config, &rule_ids(&skif_rules::policy_provider())).unwrap();
    assert_eq!(
        planned.skipped,
        Vec::<String>::new(),
        "no removal is skipped as conflicting"
    );
    planned
}

#[test]
fn adjacent_banned_dependencies_are_removed_in_one_pass() {
    let planned = remove_banned(
        "    \"keep\": \"^1.0.0\",\n    \"left-pad\": \"^1.3.0\",\n    \"lodash\": \"^4.17.21\",\n    \"zod\": \"^3.23.8\"",
    );
    assert_eq!(
        planned.changed["packages/app/package.json"],
        "{\n  \"name\": \"app\",\n  \"version\": \"1.0.0\",\n  \"private\": true,\n  \"dependencies\": {\n    \"keep\": \"^1.0.0\",\n    \"zod\": \"^3.23.8\"\n  }\n}\n"
    );
}

/// The last two members of an object are removed by two fixes that know nothing of each other; the document they
/// leave must be valid JSON.
#[test]
fn adjacent_banned_dependencies_at_the_end_leave_valid_json() {
    let planned =
        remove_banned("    \"keep\": \"^1.0.0\",\n    \"left-pad\": \"^1.3.0\",\n    \"lodash\": \"^4.17.21\"");
    let fixed = &planned.changed["packages/app/package.json"];
    let parsed: serde_json::Value = serde_json::from_str(fixed).unwrap_or_else(|error| panic!("{error}: {fixed}"));
    assert_eq!(parsed["dependencies"], serde_json::json!({"keep": "^1.0.0"}));
}
