//! MikroORM: the entities, migrations and seeders of `mikro-orm.config.*` are production entries, and the
//! `configPaths` of `package.json#mikro-orm` are further configs.
//!
//! A config may be one options object, a list of them, or a function the CLI calls with the context name; a function
//! is evaluated statically with its parameters unknown, so a path computed from the context name is not known. An
//! entity given as a class rather than a path is imported by the config, and so used already. Paths are relative to
//! `baseDir`, the workspace directory by default.

use skif_js_syntax::ConfigValue;

use crate::plugin::{ConfigAst, Enabler, PORTED_KNIP_COMMIT, Plugin, PluginContext, PluginInput, PluginSpec};
use crate::support::{config_unknown, entry_glob, entry_strings, extglob_to_braces, join_rel, objects, strings};

/// The `mikro-orm` plugin.
pub struct MikroOrm;

static SPEC: PluginSpec = PluginSpec {
    id: "mikro-orm",
    enablers: &[Enabler::DependencyPrefix("@mikro-orm/")],
    config: &[
        "src/mikro-orm.config.{ts,js}",
        "mikro-orm.config.{ts,js}",
        "dist/mikro-orm.config.js",
        "build/mikro-orm.config.js",
    ],
    package_json_key: Some("mikro-orm"),
    binaries: &["mikro-orm"],
    knip_commit: PORTED_KNIP_COMMIT,
    ..PluginSpec::DEFAULT
};

/// The files MikroORM loads from a migrations directory. The negation `!(*.d)` cannot be expressed as a glob and reads
/// as `*` ([`extglob_to_braces`]), so declaration files match too.
const MIGRATIONS_GLOB: &str = "!(*.d).{js,ts,cjs}";

/// The files MikroORM loads from a seeders directory.
const SEEDERS_GLOB: &str = "!(*.d).{js,ts}";

/// The files of an entity directory.
const ENTITY_FILES: &str = "**/*.{js,cjs,mjs,ts,cts,mts}";

impl Plugin for MikroOrm {
    fn spec(&self) -> &'static PluginSpec {
        &SPEC
    }

    fn resolve_config(&self, config: &ConfigAst<'_>, _cx: &PluginContext<'_>) -> Vec<PluginInput> {
        let mut out = Vec::new();
        if config.file_name() == "package.json" {
            for path in entry_strings(config.value, "configPaths", &mut out) {
                out.push(PluginInput::ConfigFile(join_rel("", path)));
            }
            return out;
        }
        if matches!(config.value, ConfigValue::Array(_)) && config_unknown(config.value) {
            out.push(PluginInput::EntriesIncomplete);
        }
        for options in objects(config.value) {
            add_options(options, &mut out);
        }
        out
    }
}

/// The production entries of one options object.
fn add_options(options: &ConfigValue, out: &mut Vec<PluginInput>) {
    let base = match options.get("baseDir") {
        None => String::new(),
        Some(ConfigValue::String(dir)) => join_rel("", dir),
        Some(_) => {
            out.push(PluginInput::EntriesIncomplete);
            return;
        }
    };
    for key in ["entities", "entitiesTs"] {
        for entity in options.get(key).map_or_default(strings) {
            out.extend(entity_glob(&base, entity).map(PluginInput::ProductionEntry));
        }
    }
    let migrations = options.get("migrations");
    if migrations.and_then(|m| m.get("migrationsList")).is_none() {
        add_folder(&base, migrations, "./migrations", MIGRATIONS_GLOB, out);
    }
    add_folder(&base, options.get("seeder"), "./seeders", SEEDERS_GLOB, out);
}

/// The files of the directories a `migrations` or `seeder` object names (`pathTs` over `path`, `default` without
/// either) that match its `glob`, as production entries.
fn add_folder(base: &str, folder: Option<&ConfigValue>, default: &str, glob: &str, out: &mut Vec<PluginInput>) {
    let mut paths: Vec<&str> = Vec::new();
    let mut glob = glob;
    if let Some(folder) = folder {
        paths = entry_strings(folder, "pathTs", out);
        if paths.is_empty() {
            paths = entry_strings(folder, "path", out);
        }
        match folder.get("glob") {
            None => {}
            Some(ConfigValue::String(own)) => glob = own.as_str(),
            Some(_) => out.push(PluginInput::EntriesIncomplete),
        }
    }
    if paths.is_empty() {
        paths.push(default);
    }
    let glob = extglob_to_braces(glob);
    for path in paths {
        out.extend(entry_glob(base, &join_rel(path, &glob)).map(PluginInput::ProductionEntry));
    }
}

/// The entry glob of an entity path: a glob or a file as written, a directory standing for every module below it.
fn entity_glob(base: &str, entity: &str) -> Option<String> {
    let is_pattern = entity.contains(['*', '?', '{', '}', '[']);
    let is_file = [".js", ".ts", ".cjs", ".cts", ".mjs", ".mts"]
        .iter()
        .any(|extension| entity.ends_with(extension));
    if is_pattern || is_file {
        entry_glob(base, entity)
    } else {
        entry_glob(base, &join_rel(entity, ENTITY_FILES))
    }
}
