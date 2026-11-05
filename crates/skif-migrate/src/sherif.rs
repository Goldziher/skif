//! sherif: the `sherif` key of `package.json` and the flags of the scripts that run it.

use skif_js_syntax::ConfigValue;

use crate::doc::Val;
use crate::notes::{NoteClass, Tool};
use crate::scripts::{self, Invocation};
use crate::state::{Level, Src, State};
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
    fn add(&mut self, origin: &str, values: impl IntoIterator<Item = String>) {
        for value in values {
            if !self.values.contains(&value) {
                self.values.push(value);
            }
        }
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
    state.keep(tables::sherif_rules());
    let Some(package_json) = &input.package_json else {
        return Ok(());
    };
    let src = Src::new(Tool::Sherif, package_json.path.clone());
    let package = value::read(package_json, Tool::Sherif)?;
    let mut settings = Settings::default();
    if let Some(config) = scripts::config_key(Tool::Sherif, &package) {
        read_config(state, &src, config, &mut settings);
    }
    for invocation in scripts::invocations(&package, Tool::Sherif) {
        read_flags(state, &src, &invocation, &mut settings);
    }
    emit(state, &src, &settings);
    Ok(())
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
                    settings.fail_on_warnings.add(&path, []);
                }
            }
            "select" => {
                if let Some(select) = value::string(state, src, &path, item) {
                    select_value(state, src, &path, &select, settings);
                }
            }
            "fix" => {
                if value::boolean(state, src, &path, item) == Some(true) {
                    settings.fix.add(&path, []);
                }
            }
            "noInstall" => {
                if value::boolean(state, src, &path, item) == Some(true) {
                    settings.no_install.add(&path, []);
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

fn read_flags(state: &mut State<'_>, src: &Src, invocation: &Invocation, settings: &mut Settings) {
    let origin = |flag: &str| format!("{}: {flag}", invocation.path());
    for name in invocation.values("ignore-rule", Some("r")) {
        if let Some(id) = rule_id(state, src, &origin("--ignore-rule"), &name) {
            settings.ignore_rule.add(&origin("--ignore-rule"), [id]);
        }
    }
    settings.ignore_package.add(
        &origin("--ignore-package"),
        invocation.values("ignore-package", Some("p")),
    );
    settings.ignore_dependency.add(
        &origin("--ignore-dependency"),
        invocation.values("ignore-dependency", Some("i")),
    );
    if invocation.has("fail-on-warnings", None) {
        settings.fail_on_warnings.add(&origin("--fail-on-warnings"), []);
    }
    if let Some(select) = invocation.values("select", Some("s")).pop() {
        select_value(state, src, &origin("--select"), &select, settings);
    }
    if invocation.has("fix", Some("f")) {
        settings.fix.add(&origin("--fix"), []);
    }
    if invocation.has("no-install", None) {
        settings.no_install.add(&origin("--no-install"), []);
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
        for id in &settings.ignore_rule.values {
            state.set_level(id, Level::Off);
        }
        state.note(
            src,
            NoteClass::Exact,
            settings.ignore_rule.origin(),
            format!("turned off in [rules]: {}", settings.ignore_rule.values.join(", ")),
        );
    }
    if !settings.ignore_package.is_empty() {
        state
            .doc
            .table(&["workspace"])
            .extend_strs("ignore-packages", settings.ignore_package.values.iter().cloned());
        state.note(
            src,
            NoteClass::Exact,
            settings.ignore_package.origin(),
            "written to [workspace] ignore-packages",
        );
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

fn emit_dependencies(state: &mut State<'_>, src: &Src, setting: &Setting) {
    if setting.is_empty() || setting.values.is_empty() {
        return;
    }
    let element = state.doc.element(
        &["ignore"],
        vec!["migrated from sherif ignoreDependency, which only silenced its version check".to_owned()],
    );
    element.set("rules", Val::strs(["version-mismatch"]));
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
            "sherif ignored that version everywhere; skif ignores the instance it reports",
        );
    }
}
