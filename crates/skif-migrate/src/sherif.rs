//! sherif: the `sherif` key of `package.json` and the flags of the scripts that run it.

use skif_js_syntax::ConfigValue;

use crate::doc::Val;
use crate::notes::{NoteClass, Tool};
use crate::scripts::{self, Invocation};
use crate::state::{Src, State};
use crate::{MigrateError, ToolInput, tables, value};

/// The flags sherif has, with their short form.
const VALUE_FLAGS: &[(&str, Option<&str>)] = &[
    ("ignore-package", Some("p")),
    ("ignore-dependency", Some("i")),
    ("ignore-rule", Some("r")),
    ("select", Some("s")),
];
const SWITCHES: &[(&str, Option<&str>)] = &[("fail-on-warnings", None), ("fix", Some("f")), ("no-install", None)];
/// Flags that change nothing about what is checked.
const INERT: &[&str] = &["--help", "-h", "--version", "-V"];

#[derive(Debug, Default)]
struct Setting {
    values: Vec<String>,
    origins: Vec<String>,
}

impl Setting {
    /// Adds `values`. The setting only has an origin when something was given: a flag without values is no setting.
    fn add(&mut self, origin: &str, values: impl IntoIterator<Item = String>) {
        let mut given = false;
        for value in values {
            given = true;
            if !self.values.contains(&value) {
                self.values.push(value);
            }
        }
        if given {
            self.mark(origin);
        }
    }

    /// Records that a switch was on.
    fn mark(&mut self, origin: &str) {
        if !self.origins.iter().any(|known| known == origin) {
            self.origins.push(origin.to_owned());
        }
    }

    fn origin(&self) -> String {
        self.origins.join(", ")
    }

    fn is_empty(&self) -> bool {
        self.origins.is_empty()
    }
}

#[derive(Debug, Default)]
struct Settings {
    ignore_rule: Setting,
    ignore_package: Setting,
    ignore_dependency: Setting,
    fail_on_warnings: Setting,
    select: Setting,
    fix: Setting,
    no_install: Setting,
}

pub(crate) fn translate(input: &ToolInput, state: &mut State<'_>) -> Result<(), MigrateError> {
    state.keep(Tool::Sherif, tables::sherif_rules());
    let Some(package_json) = &input.package_json else {
        emit_scope(state, &Src::new(Tool::Sherif, "package.json"));
        return Ok(());
    };
    let src = Src::new(Tool::Sherif, package_json.path.clone());
    let package = value::read(package_json, Tool::Sherif)?;
    let mut settings = Settings::default();
    if let Some(config) = scripts::config_key(Tool::Sherif, &package) {
        read_config(state, &src, config, &mut settings);
    }
    let invocations = scripts::invocations(&package, Tool::Sherif);
    for invocation in &invocations {
        read_flags(state, &src, invocation, &mut settings);
    }
    scripts::note_unrecognised(state, &src, &package, Tool::Sherif);
    note_differing_runs(state, &src, &invocations);
    note_empty_root_dependencies(state, &src, &package, &settings);
    emit(state, &src, &settings);
    Ok(())
}

/// skif has one configuration, so the flags of separate sherif runs are merged; say so when the runs differ.
fn note_differing_runs(state: &mut State<'_>, src: &Src, invocations: &[Invocation]) {
    let values = [
        ("ignore-rule", Some("r")),
        ("ignore-package", Some("p")),
        ("ignore-dependency", Some("i")),
        ("select", Some("s")),
    ];
    let switches = [("fail-on-warnings", None)];
    let differing = values
        .iter()
        .filter_map(|(long, short)| scripts::differing_values(invocations, long, *short).map(|scripts| (long, scripts)))
        .chain(switches.iter().filter_map(|(long, short)| {
            scripts::differing_switch(invocations, long, *short).map(|scripts| (long, scripts))
        }));
    for (flag, scripts) in differing {
        state.note_with(
            src,
            NoteClass::Approximate,
            format!("{scripts}: --{flag}"),
            format!("these scripts run sherif with different --{flag} settings; skif has one configuration, so they are combined"),
            "check that every check should use the combined setting",
        );
    }
}

/// sherif reports a root `dependencies` key as `root-package-dependencies`, a warning, before it looks at what the
/// section holds; skif leaves an empty section to `empty-dependencies`, an error. Only the root manifest is read here,
/// so that is the one place the difference can be seen.
fn note_empty_root_dependencies(state: &mut State<'_>, src: &Src, package: &ConfigValue, settings: &Settings) {
    let empty = package
        .get("dependencies")
        .and_then(ConfigValue::as_object)
        .is_some_and(<[_]>::is_empty);
    if !empty || settings.ignore_rule.values.iter().any(|id| id == "empty-dependencies") {
        return;
    }
    state.note_with(
        src,
        NoteClass::Approximate,
        "dependencies",
        "the root `dependencies` is empty: sherif reported it as root-package-dependencies, a warning, and skif \
         reports it as empty-dependencies, an error",
        "remove the empty `dependencies` object; `skif --fix` does",
    );
}

fn rule_id(state: &mut State<'_>, src: &Src, path: &str, name: &str) -> Option<String> {
    if let Some(id) = tables::sherif_rule(name) {
        return Some(id.to_owned());
    }
    if tables::sherif_rules().contains(&name) {
        return Some(name.to_owned());
    }
    state.note_with(
        src,
        NoteClass::Human,
        path,
        format!("`{name}` is not a sherif rule"),
        "check the spelling against `skif rules`",
    );
    None
}

fn read_config(state: &mut State<'_>, src: &Src, config: &ConfigValue, settings: &mut Settings) {
    let root = "sherif";
    for (key, item) in value::entries(state, src, root, config) {
        let path = value::join(root, key);
        match key {
            "ignoreRule" => {
                let names = value::string_list(state, src, &path, item);
                let ids: Vec<String> = names
                    .iter()
                    .filter_map(|name| rule_id(state, src, &path, name))
                    .collect();
                settings.ignore_rule.add(&path, ids);
            }
            "ignorePackage" => {
                let names = value::string_list(state, src, &path, item);
                settings.ignore_package.add(&path, names);
            }
            "ignoreDependency" => {
                let names = value::string_list(state, src, &path, item);
                settings.ignore_dependency.add(&path, names);
            }
            "failOnWarnings" => {
                if value::boolean(state, src, &path, item) == Some(true) {
                    settings.fail_on_warnings.mark(&path);
                }
            }
            "select" => {
                if let Some(select) = value::string(state, src, &path, item) {
                    select_value(state, src, &path, &select, settings);
                }
            }
            "fix" => {
                if value::boolean(state, src, &path, item) == Some(true) {
                    settings.fix.mark(&path);
                }
            }
            "noInstall" => {
                if value::boolean(state, src, &path, item) == Some(true) {
                    settings.no_install.mark(&path);
                }
            }
            "$schema" => {}
            other => state.note(
                src,
                NoteClass::Unsupported,
                path,
                format!("`{other}` is not a sherif option"),
            ),
        }
    }
}

fn select_value(state: &mut State<'_>, src: &Src, path: &str, select: &str, settings: &mut Settings) {
    if matches!(select, "highest" | "lowest") {
        settings.select.values.clear();
        settings.select.add(path, [select.to_owned()]);
    } else {
        state.note(
            src,
            NoteClass::Human,
            path,
            format!("`{select}` is not `highest` or `lowest`"),
        );
    }
}

/// The values of a sherif flag. sherif takes each value whole: `-i a,b` names one dependency, `a,b`, which matches
/// none, so such a value is noted and left out.
fn flag_values(state: &mut State<'_>, src: &Src, invocation: &Invocation, long: &str, short: &str) -> Vec<String> {
    let (whole, joined): (Vec<String>, Vec<String>) = invocation
        .whole_values(long, Some(short))
        .into_iter()
        .partition(|value| !value.contains(','));
    for value in joined {
        state.note_with(
            src,
            NoteClass::Human,
            format!("{}: --{long}", invocation.path()),
            format!("sherif does not split values at commas, so `{value}` matched nothing; it was not written"),
            format!("pass each name with its own flag (`-{short} a -{short} b`) if every one should be ignored"),
        );
    }
    whole
}

fn read_flags(state: &mut State<'_>, src: &Src, invocation: &Invocation, settings: &mut Settings) {
    let origin = |flag: &str| format!("{}: {flag}", invocation.path());
    for name in invocation.whole_values("ignore-rule", Some("r")) {
        if let Some(id) = rule_id(state, src, &origin("--ignore-rule"), &name) {
            settings.ignore_rule.add(&origin("--ignore-rule"), [id]);
        }
    }
    let packages = flag_values(state, src, invocation, "ignore-package", "p");
    settings.ignore_package.add(&origin("--ignore-package"), packages);
    let dependencies = flag_values(state, src, invocation, "ignore-dependency", "i");
    settings
        .ignore_dependency
        .add(&origin("--ignore-dependency"), dependencies);
    if invocation.has("fail-on-warnings", None) {
        settings.fail_on_warnings.mark(&origin("--fail-on-warnings"));
    }
    if let Some(select) = invocation.whole_values("select", Some("s")).pop() {
        select_value(state, src, &origin("--select"), &select, settings);
    }
    if invocation.has("fix", Some("f")) {
        settings.fix.mark(&origin("--fix"));
    }
    if invocation.has("no-install", None) {
        settings.no_install.mark(&origin("--no-install"));
    }
    let known = |flag: &str| {
        INERT.contains(&flag)
            || VALUE_FLAGS.iter().chain(SWITCHES).any(|(long, short)| {
                flag == format!("--{long}") || short.is_some_and(|short| flag == format!("-{short}"))
            })
    };
    for flag in invocation.flags() {
        if !known(&flag) {
            state.note(
                src,
                NoteClass::Human,
                format!("{}: {flag}", invocation.path()),
                "this sherif flag is not translated",
            );
        }
    }
}

fn has_version(dependency: &str) -> bool {
    dependency.rfind('@').is_some_and(|index| index > 0)
}

fn emit(state: &mut State<'_>, src: &Src, settings: &Settings) {
    if !settings.ignore_rule.is_empty() {
        state.ignore_rules(src, settings.ignore_rule.origin(), settings.ignore_rule.values.clone());
    }
    if !settings.ignore_package.is_empty() {
        emit_ignored_packages(state, src, &settings.ignore_package);
    }
    let ignores = |id: &str| settings.ignore_rule.values.iter().any(|ignored| ignored == id);
    if !ignores(VERSION_RULE) {
        emit_scope(state, src);
    }
    if !ignores(FAMILY_RULE) {
        note_families(state, src, !ignores(VERSION_RULE));
    }
    emit_dependencies(state, src, &settings.ignore_dependency);
    if !settings.fail_on_warnings.is_empty() {
        state.doc.table(&["lint"]).set("fail-on-warnings", Val::Bool(true));
        state.note(
            src,
            NoteClass::Exact,
            settings.fail_on_warnings.origin(),
            "written to [lint] fail-on-warnings",
        );
    }
    if let Some(select) = settings.select.values.first() {
        state.doc.table(&["policy"]).set("prefer", Val::str(select.clone()));
        state.note(
            src,
            NoteClass::Exact,
            settings.select.origin(),
            "written to [policy] prefer",
        );
    }
    if !settings.fix.is_empty() {
        state.note_with(
            src,
            NoteClass::Default,
            settings.fix.origin(),
            "sherif applied its fixes on every run",
            "run `skif --fix`; skif never fixes without the flag",
        );
    }
    if !settings.no_install.is_empty() {
        state.note_with(
            src,
            NoteClass::Default,
            settings.no_install.origin(),
            "sherif skipped the install after fixing",
            "pass `--no-install` to `skif --fix` for the same effect",
        );
    }
}

/// The skif rule sherif's `multiple-dependency-versions` maps to.
const VERSION_RULE: &str = "version-mismatch";

/// The skif rule sherif's `unsync-similar-dependencies` maps to.
const FAMILY_RULE: &str = "unsynced-dependency-family";

/// `unsynced-dependency-family` reads the sections of `[policy] dependency-types`, so with the scope written it reads
/// dependencies and devDependencies as sherif did; what still differs is said here.
fn note_families(state: &mut State<'_>, src: &Src, scoped: bool) {
    let sections = if scoped {
        ""
    } else {
        "; and with no [policy] dependency-types written, optionalDependencies take part, which sherif never read"
    };
    state.note_with(
        src,
        NoteClass::Approximate,
        "unsync-similar-dependencies",
        format!(
            "skif compares a `catalog:` reference by the version of its catalog entry, which sherif left out{sections}"
        ),
        "align the family members, or set `unsynced-dependency-family` to \"off\" in [rules]",
    );
}

/// The sections sherif compares versions in. It never compares a dependency with a package's own `version`, so the
/// scope leaves `local` out (ADR 0072).
const SCOPE: &[&str] = &["prod", "dev"];

/// The specifier types sherif's `multiple-dependency-versions` never compares. sherif keeps a specifier only when the
/// `semver` crate reads it as a version or as a range with at least one comparator, so `*`, `latest`, dist-tags,
/// `||`, hyphen and space-separated ranges and every protocol (`workspace:`, `npm:`, git, file, link, url) take no
/// part. `catalog:` references are not listed: `version-mismatch` leaves them to the catalog rules anyway.
pub(crate) const UNPARSED_SPECIFIERS: &[&str] = &[
    "alias",
    "file",
    "git",
    "latest",
    "link",
    "missing",
    "range-complex",
    "tag",
    "unsupported",
    "url",
    "workspace-protocol",
];

/// The rule that reports a version group that owns no dependency.
pub(crate) const UNUSED_GROUP_RULE: &str = "policy-conflict";

/// The note path for what sherif compared.
const SPECIFIERS_PATH: &str = "multiple-dependency-versions: specifiers";

/// `[policy] dependency-types` as sherif's `multiple-dependency-versions` saw the workspace.
fn emit_scope(state: &mut State<'_>, src: &Src) {
    state.widen_scope(SCOPE.iter().map(|token| (*token).to_owned()).collect());
    state.sherif_versions = Some(src.clone());
    state.note(
        src,
        NoteClass::Exact,
        "multiple-dependency-versions",
        "[policy] dependency-types covers dependencies and devDependencies, the sections sherif compares; like \
         sherif, skif then does not compare a dependency with the version of the workspace package it names",
    );
    state.note_with(
        src,
        NoteClass::Approximate,
        "multiple-dependency-versions: dependencies and devDependencies",
        "sherif read a dependency that one package lists in both dependencies and devDependencies at its \
         devDependencies specifier only; skif compares both entries, so two different specifiers there are a \
         version-mismatch",
        "list the dependency in one of the two sections, or give both entries the same specifier",
    );
}

/// Leaves the specifiers sherif never compared out of `version-mismatch`, once every tool is read: syncpack compared
/// them, so a migration with syncpack keeps them. The group is written only when `silent`, that is when nothing
/// reports it for owning no dependency in a workspace without such specifiers; otherwise the note says how.
pub(crate) fn leave_out_unparsed(state: &mut State<'_>, src: &Src, silent: bool) {
    let types = UNPARSED_SPECIFIERS.join(", ");
    if state.tools.contains(&Tool::Syncpack) {
        state.note(
            src,
            NoteClass::Approximate,
            SPECIFIERS_PATH,
            format!(
                "sherif compared semver versions and ranges only; syncpack compared every specifier, so skif does too \
                 ({types})"
            ),
        );
        return;
    }
    if !silent {
        let quoted: Vec<String> = UNPARSED_SPECIFIERS.iter().map(|name| format!("\"{name}\"")).collect();
        state.note_with(
            src,
            NoteClass::Approximate,
            SPECIFIERS_PATH,
            format!("sherif compared semver versions and ranges only; skif also compares the specifier types {types}"),
            format!(
                "to leave them out as sherif did, add a last [[version-groups]] entry with specifier-types = [{}] and \
                 policy = \"ignored\"",
                quoted.join(", ")
            ),
        );
        return;
    }
    let element = state.doc.element(
        &["version-groups"],
        vec!["migrated from sherif, which compared semver versions and ranges only".to_owned()],
    );
    element.set("label", Val::str("not compared by sherif"));
    element.set("specifier-types", Val::strs(UNPARSED_SPECIFIERS.iter().copied()));
    element.set("policy", Val::str("ignored"));
    state.note_with(
        src,
        NoteClass::Approximate,
        SPECIFIERS_PATH,
        format!(
            "written to [[version-groups]] as an ignored group of the specifier types sherif did not compare \
             ({types}); a range sherif read but skif does not classify, such as `>= 1.0.0` or `1.x`, is \
             `unsupported` and is left out as well"
        ),
        "write such a range without the space (`>=1.0.0`) or in full (`^1.0.0`) to keep it compared",
    );
}

/// `ignorePackage` values for `[workspace] ignore-packages`. sherif matches a path with its `./` prefix
/// (`./apps/legacy`, `./integrations/*`) and skif matches the directory relative to the root, so the prefix goes.
fn emit_ignored_packages(state: &mut State<'_>, src: &Src, setting: &Setting) {
    let values: Vec<String> = setting.values.iter().map(|value| package_pattern(value)).collect();
    let bare_paths: Vec<&str> = setting
        .values
        .iter()
        .filter(|value| !value.starts_with("./") && !value.starts_with('@') && value.contains('/'))
        .map(String::as_str)
        .collect();
    state.doc.table(&["workspace"]).extend_strs("ignore-packages", values);
    state.note(
        src,
        NoteClass::Exact,
        setting.origin(),
        "written to [workspace] ignore-packages, paths without their `./` prefix",
    );
    if !bare_paths.is_empty() {
        state.note_with(
            src,
            NoteClass::Approximate,
            setting.origin(),
            format!(
                "{} look like paths without `./`, which sherif matched against no package; skif ignores the packages in \
                 those directories",
                bare_paths.join(", ")
            ),
            "remove the entries if those packages should still be checked",
        );
    }
}

/// A sherif package path as skif matches it: `./apps/*` is `apps/*`; a name stays as written.
fn package_pattern(value: &str) -> String {
    let mut rest = value;
    while let Some(stripped) = rest.strip_prefix("./") {
        rest = stripped;
    }
    if rest.is_empty() {
        ".".to_owned()
    } else {
        rest.to_owned()
    }
}

fn emit_dependencies(state: &mut State<'_>, src: &Src, setting: &Setting) {
    if setting.is_empty() || setting.values.is_empty() {
        return;
    }
    let element = state.doc.element(
        &["ignore"],
        vec!["migrated from sherif ignoreDependency, which only silenced its version check".to_owned()],
    );
    element.set("rules", Val::strs([VERSION_RULE]));
    element.set("dependencies", Val::strs(setting.values.iter().cloned()));
    state.note(
        src,
        NoteClass::Exact,
        setting.origin(),
        "written to [[ignore]] for version-mismatch, the only rule sherif applied it to",
    );
    let versioned: Vec<&String> = setting.values.iter().filter(|value| has_version(value)).collect();
    if !versioned.is_empty() {
        let names: Vec<&str> = versioned.iter().map(|value| value.as_str()).collect();
        state.note_with(
            src,
            NoteClass::Approximate,
            setting.origin(),
            format!(
                "`name@version` entries ({}) are matched per diagnosed instance",
                names.join(", ")
            ),
            "sherif left that version out of its comparison; skif only silences the instances at it, which still \
             count when skif picks the version to align on, so the other instances can be reported against it \
             (with `prefer = \"lowest\"`, for example); ignore the name instead if that version must not count",
        );
    }
}

#[cfg(test)]
mod tests {
    use skif_specifier::{CONFIG_IDENTIFIERS, Specifier};

    use super::*;

    #[test]
    fn the_unparsed_specifier_types_are_the_ones_sherif_leaves_out() {
        assert!(UNPARSED_SPECIFIERS.iter().all(|name| CONFIG_IDENTIFIERS.contains(name)));
        let left_out = [
            "*",
            "x",
            "latest",
            "next",
            "^1.0.0 || ^2.0.0",
            "1.0.0 - 2.0.0",
            ">=1.0.0 <2.0.0",
            "workspace:*",
            "npm:other@1.0.0",
            "git+https://example.com/a/b.git",
            "github:a/b",
            "file:../b",
            "link:../b",
            "https://example.com/b.tgz",
            "",
        ];
        for specifier in left_out {
            let kind = Specifier::create(specifier).get_config_identifier();
            assert!(UNPARSED_SPECIFIERS.contains(&kind), "{specifier:?} is {kind}");
        }
        for specifier in [
            "1.2.3",
            "=1.2.3",
            "1.2.3-beta.1",
            "^1.2.3",
            "~1.2",
            ">=1",
            "<2.0.0",
            "1",
            "1.2",
        ] {
            let kind = Specifier::create(specifier).get_config_identifier();
            assert!(!UNPARSED_SPECIFIERS.contains(&kind), "{specifier:?} is {kind}");
        }
        assert!(
            !UNPARSED_SPECIFIERS.contains(&"catalog"),
            "catalog references stay with the catalog rules"
        );
    }
}
