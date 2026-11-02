#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use serde_json::json;
use skif::{
    Alternatives, Category, ConfigOverrides, Diagnostic, Emitter, FileChange, FileSystem, Fix, FixOp, FixSafety,
    FixSelection, Fixability, InsertPosition, JsonAction, LintContext, Linter, Location, MemoryFs, PathSeg, Prefer,
    Report, Rule, RuleMeta, RulesProvider, RunOptions, Severity, SkipReason, TextEdit, Workspace, apply_fixes,
    diagnostic, load_config, preferred_alternative,
};

const ROOT_MANIFEST: &str = "{\n  \"name\": \"root\",\n  \"workspaces\": [\"packages/*\"]\n}\n";
const MEMBER_MANIFEST: &str = "{\n  \"name\": \"a\",\n  \"version\": \"1.0.0\"\n}\n";

static PRIVATE: RuleMeta = meta("demo-private", Fixability::Safe);
static ALTERNATIVES: RuleMeta = meta("demo-alt", Fixability::Unsafe);
static CONFLICT: RuleMeta = meta("demo-conflict", Fixability::Unsafe);

const fn meta(id: &'static str, fixable: Fixability) -> RuleMeta {
    RuleMeta {
        id,
        category: Category::Manifest,
        default: Some(Severity::Warning),
        fixable,
        summary: "demo",
    }
}

fn fs() -> MemoryFs {
    MemoryFs::new()
        .with_file("/ws/package.json", ROOT_MANIFEST)
        .with_file("/ws/packages/a/package.json", MEMBER_MANIFEST)
}

fn workspace(fs: &MemoryFs) -> Workspace {
    let config = load_config(Path::new("/ws"), fs, &ConfigOverrides::default()).unwrap();
    Workspace::discover_with_fs(Path::new("/ws"), &config, fs).unwrap()
}

fn set_version(path: &str, version: &str) -> FixOp {
    FixOp::Json {
        path: path.into(),
        pointer: vec![PathSeg::Key("version".into())],
        action: JsonAction::Set(json!(version)),
    }
}

fn fix(title: &str, safety: FixSafety, ops: Vec<FixOp>) -> Fix {
    Fix {
        title: title.to_owned(),
        safety,
        ops,
    }
}

fn private_fix() -> Fix {
    fix(
        "set private",
        FixSafety::Safe,
        vec![FixOp::Json {
            path: "package.json".into(),
            pointer: Vec::new(),
            action: JsonAction::Insert {
                key: "private".into(),
                value: json!(true),
                position: InsertPosition::End,
            },
        }],
    )
}

fn member_fixes() -> Vec<Fix> {
    vec![
        fix(
            "highest",
            FixSafety::Unsafe,
            vec![set_version("packages/a/package.json", "2.0.0")],
        ),
        fix(
            "lowest",
            FixSafety::Unsafe,
            vec![set_version("packages/a/package.json", "0.1.0")],
        ),
    ]
}

struct DemoRule {
    meta: &'static RuleMeta,
    path: &'static str,
    fixes: Vec<Fix>,
}

impl Rule for DemoRule {
    fn meta(&self) -> &'static RuleMeta {
        self.meta
    }

    fn check(&self, _cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        let location = Location {
            path: PathBuf::from(self.path),
            span: None,
        };
        let mut builder = diagnostic(self.meta, format!("{} finding", self.meta.id), location);
        for fix in &self.fixes {
            builder = builder.fix(fix.clone());
        }
        out.emit(builder.build());
    }
}

fn demo_linter() -> Linter {
    let rules: Vec<Box<dyn Rule>> = vec![
        Box::new(DemoRule {
            meta: &PRIVATE,
            path: "package.json",
            fixes: vec![private_fix()],
        }),
        Box::new(DemoRule {
            meta: &ALTERNATIVES,
            path: "packages/a/package.json",
            fixes: member_fixes(),
        }),
        Box::new(DemoRule {
            meta: &CONFLICT,
            path: "packages/a/package.json",
            fixes: vec![fix(
                "pin 3",
                FixSafety::Unsafe,
                vec![set_version("packages/a/package.json", "3.0.0")],
            )],
        }),
    ];
    let config = load_config(Path::new("/ws"), &fs(), &ConfigOverrides::default()).unwrap();
    Linter::with_providers(&config, vec![Box::new(RulesProvider::new("demo", rules))]).unwrap()
}

fn report(fs: &MemoryFs) -> (Workspace, Report) {
    let workspace = workspace(fs);
    let report = demo_linter().run(&workspace).unwrap();
    (workspace, report)
}

fn index_of(report: &Report, rule: &str) -> usize {
    report.diagnostics.iter().position(|d| d.rule.as_str() == rule).unwrap()
}

fn modified<'a>(changes: &'a [FileChange], wanted: &str) -> &'a str {
    changes
        .iter()
        .find_map(|change| match change {
            FileChange::Modify {
                path: file,
                new_contents,
                ..
            } if file == Path::new(wanted) => Some(new_contents.as_str()),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no modification of {wanted}"))
}

#[test]
fn plan_is_data_and_leaves_the_file_system_untouched() {
    let fs = fs();
    let (workspace, report) = report(&fs);
    let selection = FixSelection::new(FixSafety::Unsafe).with_alternatives(Alternatives::First);
    let plan = apply_fixes(&workspace, &report, &selection, &fs).unwrap();

    let paths: Vec<&Path> = plan.paths().collect();
    assert_eq!(paths, [Path::new("package.json"), Path::new("packages/a/package.json")]);
    let root: serde_json::Value = serde_json::from_str(modified(&plan.changes, "package.json")).unwrap();
    assert_eq!(
        root,
        json!({"name": "root", "workspaces": ["packages/*"], "private": true})
    );
    assert!(modified(&plan.changes, "package.json").starts_with("{\n  \"name\": \"root\","));
    assert_eq!(
        modified(&plan.changes, "packages/a/package.json"),
        "{\n  \"name\": \"a\",\n  \"version\": \"2.0.0\"\n}\n"
    );
    assert_eq!(
        &*fs.read_to_string(Path::new("/ws/package.json")).unwrap(),
        ROOT_MANIFEST
    );
    assert_eq!(
        &*fs.read_to_string(Path::new("/ws/packages/a/package.json")).unwrap(),
        MEMBER_MANIFEST
    );
}

#[test]
fn overlapping_fix_is_skipped_and_the_earlier_one_wins() {
    let fs = fs();
    let (workspace, report) = report(&fs);
    let selection = FixSelection::new(FixSafety::Unsafe).with_alternatives(Alternatives::First);
    let plan = apply_fixes(&workspace, &report, &selection, &fs).unwrap();

    let applied: Vec<&str> = plan.applied.iter().map(|fix| fix.rule.as_str()).collect();
    assert_eq!(applied, ["demo-private", "demo-alt"]);
    let [skipped] = plan.skipped.as_slice() else {
        panic!("expected one skipped fix, got {:?}", plan.skipped);
    };
    assert_eq!(skipped.rule.as_str(), "demo-conflict");
    assert!(
        matches!(skipped.reason, SkipReason::Conflict(_)),
        "{:?}",
        skipped.reason
    );
}

#[test]
fn safety_ceiling_skips_unsafe_fixes() {
    let fs = fs();
    let (workspace, report) = report(&fs);
    let selection = FixSelection::new(FixSafety::Safe).with_alternatives(Alternatives::First);
    let plan = apply_fixes(&workspace, &report, &selection, &fs).unwrap();

    assert_eq!(plan.applied.len(), 1);
    assert_eq!(plan.applied[0].rule.as_str(), "demo-private");
    assert_eq!(plan.skipped.len(), 2);
    assert!(
        plan.skipped
            .iter()
            .all(|skipped| skipped.reason == SkipReason::Safety(FixSafety::Unsafe))
    );
}

#[test]
fn alternatives_without_a_pick_need_a_choice() {
    let fs = fs();
    let (workspace, report) = report(&fs);
    let plan = apply_fixes(&workspace, &report, &FixSelection::new(FixSafety::Unsafe), &fs).unwrap();

    let needs_choice = plan
        .skipped
        .iter()
        .find(|skipped| skipped.rule.as_str() == "demo-alt")
        .unwrap();
    assert_eq!(
        needs_choice.reason,
        SkipReason::NeedsChoice {
            alternatives: vec!["highest".to_owned(), "lowest".to_owned()]
        }
    );
    assert_eq!(
        modified(&plan.changes, "packages/a/package.json"),
        "{\n  \"name\": \"a\",\n  \"version\": \"3.0.0\"\n}\n"
    );
}

#[test]
fn configuration_picks_the_alternative_that_names_the_preferred_version() {
    for (prefer, version) in [(Prefer::Highest, "2.0.0"), (Prefer::Lowest, "0.1.0")] {
        let fs = fs();
        let (workspace, report) = report(&fs);
        let selection = FixSelection::new(FixSafety::Unsafe).with_alternatives(Alternatives::Preferred(prefer));
        let plan = apply_fixes(&workspace, &report, &selection, &fs).unwrap();
        assert!(
            modified(&plan.changes, "packages/a/package.json").contains(version),
            "{prefer:?}"
        );
        assert!(plan.skipped.iter().all(|skipped| skipped.rule.as_str() != "demo-alt"));
    }
}

#[test]
fn alternatives_that_do_not_name_the_preferred_version_still_need_a_choice() {
    let fs = fs();
    let workspace = workspace(&fs);
    let alternatives = vec![
        fix("pin one", FixSafety::Unsafe, vec![set_version("packages/a/package.json", "1.0.1")]),
        fix("pin two", FixSafety::Unsafe, vec![set_version("packages/a/package.json", "1.0.2")]),
    ];
    assert_eq!(preferred_alternative(&alternatives, Prefer::Highest), None);
    let report = report_of(vec![("demo-alt-unnamed", alternatives)]);
    let selection = FixSelection::new(FixSafety::Unsafe).with_alternatives(Alternatives::Preferred(Prefer::Highest));
    let plan = apply_fixes(&workspace, &report, &selection, &fs).unwrap();
    assert!(plan.changes.is_empty());
    assert!(matches!(plan.skipped[0].reason, SkipReason::NeedsChoice { .. }));
}

#[test]
fn explicit_choice_selects_the_alternative() {
    let fs = fs();
    let (workspace, report) = report(&fs);
    let selection = FixSelection::new(FixSafety::Unsafe).choose(index_of(&report, "demo-alt"), 1);
    let plan = apply_fixes(&workspace, &report, &selection, &fs).unwrap();

    assert_eq!(
        modified(&plan.changes, "packages/a/package.json"),
        "{\n  \"name\": \"a\",\n  \"version\": \"0.1.0\"\n}\n"
    );
}

#[test]
fn out_of_range_choice_is_skipped_not_guessed() {
    let fs = fs();
    let (workspace, report) = report(&fs);
    let selection = FixSelection::new(FixSafety::Unsafe).choose(index_of(&report, "demo-alt"), 7);
    let plan = apply_fixes(&workspace, &report, &selection, &fs).unwrap();

    let skipped = plan
        .skipped
        .iter()
        .find(|skipped| skipped.rule.as_str() == "demo-alt")
        .unwrap();
    assert!(matches!(skipped.reason, SkipReason::Failed(_)), "{:?}", skipped.reason);
}

#[test]
fn rule_filter_limits_the_plan() {
    let fs = fs();
    let (workspace, report) = report(&fs);
    let selection = FixSelection::new(FixSafety::Unsafe).with_rules(["demo-private".to_owned()]);
    let plan = apply_fixes(&workspace, &report, &selection, &fs).unwrap();

    assert_eq!(plan.paths().collect::<Vec<_>>(), [Path::new("package.json")]);
    assert_eq!(plan.skipped.len(), 0);
}

#[test]
fn creating_a_file_another_fix_edits_is_a_conflict() {
    let fs = fs();
    let workspace = workspace(&fs);
    let edit = fix(
        "edit",
        FixSafety::Safe,
        vec![set_version("packages/a/package.json", "2.0.0")],
    );
    let create = fix(
        "create",
        FixSafety::Safe,
        vec![FixOp::CreateFile {
            path: "packages/a/package.json".into(),
            contents: "{}".to_owned(),
        }],
    );
    let diagnostics: Vec<Diagnostic> = [("demo-a", edit), ("demo-b", create)]
        .into_iter()
        .map(|(id, fix)| {
            let meta: &'static RuleMeta = Box::leak(Box::new(meta(id, Fixability::Safe)));
            let location = Location {
                path: "packages/a/package.json".into(),
                span: None,
            };
            diagnostic(meta, id, location).fix(fix).build()
        })
        .collect();
    let report = Report::new(diagnostics, 2);
    let plan = apply_fixes(&workspace, &report, &FixSelection::default(), &fs).unwrap();

    assert_eq!(plan.applied.len(), 1);
    assert!(matches!(plan.skipped[0].reason, SkipReason::Conflict(_)));
}

#[test]
fn create_and_delete_become_changes() {
    let fs = fs();
    let workspace = workspace(&fs);
    let meta: &'static RuleMeta = Box::leak(Box::new(meta("demo-files", Fixability::Safe)));
    let location = Location {
        path: "package.json".into(),
        span: None,
    };
    let diagnostic = diagnostic(meta, "files", location)
        .fix(fix(
            "files",
            FixSafety::Safe,
            vec![
                FixOp::CreateFile {
                    path: "new/skif.txt".into(),
                    contents: "x".to_owned(),
                },
                FixOp::DeleteFile {
                    path: "packages/a/package.json".into(),
                },
            ],
        ))
        .build();
    let report = Report::new(vec![diagnostic], 2);
    let plan = apply_fixes(&workspace, &report, &FixSelection::default(), &fs).unwrap();

    assert_eq!(
        plan.changes,
        [
            FileChange::Create {
                path: "new/skif.txt".into(),
                contents: "x".to_owned()
            },
            FileChange::Delete {
                path: "packages/a/package.json".into()
            },
        ]
    );
}

#[test]
fn run_with_restricts_rules_and_honours_cancellation() {
    let fs = fs();
    let workspace = workspace(&fs);
    let linter = demo_linter();

    let only = ["demo-private".to_owned()];
    let report = linter
        .run_with(&workspace, &RunOptions::default().with_rules(&only))
        .unwrap();
    assert_eq!(report.diagnostics.len(), 1);
    assert_eq!(report.diagnostics[0].rule.as_str(), "demo-private");

    let unknown = ["nope".to_owned()];
    let error = linter
        .run_with(&workspace, &RunOptions::default().with_rules(&unknown))
        .unwrap_err();
    assert!(matches!(error, skif::Error::UnknownRule(_)), "{error}");

    let cancel = AtomicBool::new(true);
    let error = linter
        .run_with(&workspace, &RunOptions::default().with_cancel(&cancel))
        .unwrap_err();
    assert!(matches!(error, skif::Error::Lint(_)), "{error}");
}

#[test]
fn unknown_rule_in_configuration_fails_at_construction() {
    let fs = fs().with_file("/ws/skif.toml", "[rules]\nno-such-rule = \"error\"\n");
    let config = load_config(Path::new("/ws"), &fs, &ConfigOverrides::default()).unwrap();
    let error = Linter::with_providers(&config, Vec::new()).err().unwrap();
    assert!(matches!(error, skif::Error::Config(_)), "{error}");
}

#[test]
fn configuration_errors_carry_the_file() {
    let fs = fs().with_file("/ws/skif.toml", "[lint]\nbogus = 1\n");
    let error = load_config(Path::new("/ws"), &fs, &ConfigOverrides::default()).unwrap_err();
    match error {
        skif::ConfigError::Parse { path, line, .. } => {
            assert_eq!(path.as_deref(), Some(Path::new("/ws/skif.toml")));
            assert_eq!(line, Some(2));
        }
        other => panic!("unexpected error {other:?}"),
    }
}

#[test]
fn both_configuration_files_are_ambiguous() {
    let fs = fs().with_file("/ws/skif.toml", "").with_file("/ws/.skif.toml", "");
    let error = load_config(Path::new("/ws"), &fs, &ConfigOverrides::default()).unwrap_err();
    assert!(matches!(error, skif::ConfigError::Ambiguous { .. }), "{error}");
}

fn report_of(diagnostics: Vec<(&'static str, Vec<Fix>)>) -> Report {
    let built: Vec<Diagnostic> = diagnostics
        .into_iter()
        .map(|(id, fixes)| {
            let meta: &'static RuleMeta = Box::leak(Box::new(meta(id, Fixability::Safe)));
            let location = Location {
                path: "package.json".into(),
                span: None,
            };
            fixes
                .into_iter()
                .fold(diagnostic(meta, id, location), skif::DiagnosticBuilder::fix)
                .build()
        })
        .collect();
    Report::new(built, 2)
}

fn text_fix(title: &str, path: &str, start: u32, end: u32, replacement: &str) -> Fix {
    fix(
        title,
        FixSafety::Safe,
        vec![FixOp::Text {
            path: path.into(),
            edit: TextEdit {
                start,
                end,
                replacement: replacement.to_owned(),
            },
        }],
    )
}

#[test]
fn an_out_of_range_text_edit_skips_only_that_fix() {
    let fs = fs();
    let workspace = workspace(&fs);
    let report = report_of(vec![
        (
            "demo-bad",
            vec![text_fix("past the end", "package.json", 9_000, 9_001, "x")],
        ),
        (
            "demo-good",
            vec![text_fix("rename", "package.json", 12, 18, "\"main\"")],
        ),
    ]);
    let plan = apply_fixes(&workspace, &report, &FixSelection::default(), &fs).unwrap();

    assert_eq!(plan.applied.len(), 1);
    assert_eq!(plan.applied[0].rule.as_str(), "demo-good");
    assert!(
        matches!(plan.skipped[0].reason, SkipReason::Failed(_)),
        "{:?}",
        plan.skipped
    );
    assert!(modified(&plan.changes, "package.json").contains("\"name\": \"main\""));
}

#[test]
fn identical_operations_from_different_fixes_count_once() {
    let fs = fs();
    let workspace = workspace(&fs);
    let report = report_of(vec![
        ("demo-one", vec![private_fix()]),
        ("demo-two", vec![private_fix()]),
    ]);
    let plan = apply_fixes(&workspace, &report, &FixSelection::default(), &fs).unwrap();

    assert_eq!(plan.applied.len(), 2, "{:?}", plan.skipped);
    assert_eq!(plan.skipped, [] as [skif::SkippedFix; 0]);
    let text = modified(&plan.changes, "package.json");
    assert_eq!(text.matches("\"private\"").count(), 1, "{text}");
}

#[test]
fn alternatives_pick_the_first_one_the_safety_ceiling_allows() {
    let fs = fs();
    let workspace = workspace(&fs);
    let alternatives = vec![
        fix(
            "unsafe pin",
            FixSafety::Unsafe,
            vec![set_version("packages/a/package.json", "9.0.0")],
        ),
        fix(
            "safe pin",
            FixSafety::Safe,
            vec![set_version("packages/a/package.json", "1.0.1")],
        ),
    ];
    let report = report_of(vec![("demo-alt-safe", alternatives)]);
    let plan = apply_fixes(
        &workspace,
        &report,
        &FixSelection::new(FixSafety::Safe).with_alternatives(Alternatives::First),
        &fs,
    )
    .unwrap();

    assert_eq!(plan.applied[0].title, "safe pin");
    assert!(modified(&plan.changes, "packages/a/package.json").contains("1.0.1"));
}

#[test]
fn creating_over_an_existing_file_is_refused() {
    let fs = fs();
    let workspace = workspace(&fs);
    let create = fix(
        "create",
        FixSafety::Safe,
        vec![FixOp::CreateFile {
            path: "package.json".into(),
            contents: "{}".to_owned(),
        }],
    );
    let plan = apply_fixes(
        &workspace,
        &report_of(vec![("demo-create", vec![create])]),
        &FixSelection::default(),
        &fs,
    )
    .unwrap();

    assert_eq!(plan.changes, [] as [skif::FileChange; 0]);
    assert!(
        matches!(plan.skipped[0].reason, SkipReason::Failed(_)),
        "{:?}",
        plan.skipped
    );
}

#[test]
fn text_edits_apply_to_files_that_are_neither_json_nor_yaml() {
    let fs = fs().with_file("/ws/notes.txt", "hello world");
    let workspace = workspace(&fs);
    let report = report_of(vec![("demo-text", vec![text_fix("greet", "notes.txt", 0, 5, "bye")])]);
    let plan = apply_fixes(&workspace, &report, &FixSelection::default(), &fs).unwrap();

    assert_eq!(modified(&plan.changes, "notes.txt"), "bye world");
}
