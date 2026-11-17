//! `skif cache stats` and `skif cache clean` (ADR 0007): what the cache location of a project holds, and removing it.
//!
//! The location is the one a lint of the same path would use: `--cache-location`, else `SKIF_CACHE_DIR`, else
//! `[cache] location` of the root configuration, else the default. Neither command creates a file, and `clean`
//! removes only skif's own store directories.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use skif::{CachedFs, ConfigOverrides, OsFs, slash};
use skif_cache::{CacheStats, CleanReport, base_key};
use skif_report::escape_controls;

use crate::args::{CacheArgs, CacheCleanArgs, CacheCommand, CacheStatsArgs, Format};
use crate::cache::{CacheEnv, CacheFlags};
use crate::env::Env;
use crate::error::{CliError, Kind};
use crate::lint::canonical;
use crate::output::{emit_error, stderr_line, write_stdout};
use crate::roots;

const CLEAN: u8 = 0;
const COULD_NOT_RUN: u8 = 2;

/// Runs `skif cache` and returns the exit code.
pub fn run(args: &CacheArgs) -> u8 {
    let (result, format) = match &args.command {
        CacheCommand::Stats(args) => (stats(args), args.format.into()),
        CacheCommand::Clean(args) => (clean(args), args.format.into()),
    };
    match result.and_then(|text| write_stdout(text.as_bytes())) {
        Ok(()) => CLEAN,
        Err(error) => {
            emit_error(&error, format);
            COULD_NOT_RUN
        }
    }
}

/// The base a lint of `path` reports from and the cache location it would use; `None` when it would cache in memory.
fn located(path: &Path, location: Option<&Path>) -> Result<(PathBuf, Option<PathBuf>), CliError> {
    let start = canonical(path)?;
    let fs = CachedFs::new(OsFs);
    // A project that cannot be loaded still has a cache to show or remove: the one of its directory.
    let (base, targets) = match roots::resolve(&start, false, &fs, &ConfigOverrides::default()) {
        Ok(resolved) => (resolved.base, resolved.targets),
        Err(_) if start.is_dir() => (start, Vec::new()),
        Err(_) => (
            start.parent().map_or_else(|| start.clone(), Path::to_path_buf),
            Vec::new(),
        ),
    };
    let flags = CacheFlags {
        location: location.map(Path::to_path_buf),
        ..CacheFlags::default()
    };
    let (settings, warnings) = crate::cache::resolve(&flags, &CacheEnv::detect(Env::detect().ci), &base, &targets);
    for warning in warnings {
        stderr_line(&format!("warning: {warning}"));
    }
    Ok((base, settings.location))
}

/// A size for people: bytes below 1 KiB, else one decimal of the largest unit.
fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut unit = 0;
    let mut scaled = bytes;
    while scaled >= 1024 * 1024 && unit + 1 < UNITS.len() {
        scaled /= 1024;
        unit += 1;
    }
    format!("{}.{} {}", scaled / 1024, scaled % 1024 * 10 / 1024, UNITS[unit])
}

fn io_error(context: &str, location: &Path, error: &std::io::Error) -> CliError {
    CliError::io(context, location, error)
}

/// `skif cache stats`: the document or the text it prints.
///
/// # Errors
/// The path cannot be resolved, or the cache location exists and cannot be listed.
pub fn stats(args: &CacheStatsArgs) -> Result<String, CliError> {
    let (base, location) = located(&args.path, args.cache_location.as_deref())?;
    let found = match &location {
        Some(location) => {
            skif_cache::stats(location).map_err(|error| io_error("cannot read the cache in", location, &error))?
        }
        None => CacheStats::default(),
    };
    let current = base_key(&base);
    if Format::from(args.format) == Format::Json {
        let stores: Vec<Value> = found
            .stores
            .iter()
            .map(|store| {
                let namespaces: Vec<Value> = store
                    .namespaces
                    .iter()
                    .map(|namespace| {
                        json!({
                            "name": namespace.name,
                            "entries": namespace.entries,
                            "bytes": namespace.bytes,
                        })
                    })
                    .collect();
                json!({
                    "key": store.key,
                    "base": store.base,
                    "current": store.key == current,
                    "readable": store.readable,
                    "bytes": store.bytes,
                    "namespaces": namespaces,
                })
            })
            .collect();
        let document = json!({
            "location": location.as_deref().map(slash),
            "bytes": found.bytes,
            "stores": stores,
        });
        return serde_json::to_string_pretty(&document)
            .map(|text| text + "\n")
            .map_err(|error| CliError::new(Kind::Internal, error.to_string()));
    }
    let mut text = String::new();
    let Some(location) = location else {
        return Ok("no cache location: skif caches in memory for each run\n".to_owned());
    };
    let _ = writeln!(text, "cache location: {}", escape_controls(&slash(&location)));
    for store in &found.stores {
        let _ = writeln!(
            text,
            "  {}{}  {}{}",
            store.key,
            store
                .base
                .as_deref()
                .map_or_default(|base| format!(" ({})", escape_controls(base))),
            if store.readable {
                human_size(store.bytes)
            } else {
                "unreadable".to_owned()
            },
            if store.key == current { "  (this project)" } else { "" }
        );
        for namespace in &store.namespaces {
            let entries = namespace
                .entries
                .map_or_else(|| "unreadable".to_owned(), |entries| format!("{entries} entries"));
            let _ = writeln!(
                text,
                "    {}: {entries}, {}",
                escape_controls(&namespace.name),
                human_size(namespace.bytes)
            );
        }
    }
    let count = found.stores.len();
    let _ = writeln!(
        text,
        "total: {count} store{}, {}",
        if count == 1 { "" } else { "s" },
        human_size(found.bytes)
    );
    Ok(text)
}

/// `skif cache clean`: removes the store of the project, or with `--all` every store of the location, and returns
/// the document or the text it prints.
///
/// # Errors
/// The path cannot be resolved, or a store exists and cannot be removed.
pub fn clean(args: &CacheCleanArgs) -> Result<String, CliError> {
    let (base, location) = located(&args.path, args.cache_location.as_deref())?;
    let removed = match &location {
        Some(location) => skif_cache::clean(location, (!args.all).then_some(base.as_path()))
            .map_err(|error| io_error("cannot remove the cache in", location, &error))?,
        None => CleanReport::default(),
    };
    if Format::from(args.format) == Format::Json {
        let document = json!({
            "location": location.as_deref().map(slash),
            "stores": removed.stores,
            "bytes": removed.bytes,
        });
        return serde_json::to_string_pretty(&document)
            .map(|text| text + "\n")
            .map_err(|error| CliError::new(Kind::Internal, error.to_string()));
    }
    let Some(location) = location else {
        return Ok("no cache location: nothing to remove\n".to_owned());
    };
    Ok(format!(
        "removed {} store{} ({}) from {}\n",
        removed.stores,
        if removed.stores == 1 { "" } else { "s" },
        human_size(removed.bytes),
        escape_controls(&slash(&location))
    ))
}

#[cfg(test)]
mod tests {
    use super::human_size;

    #[test]
    fn sizes_are_shown_in_the_largest_unit() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KiB");
        assert_eq!(human_size(1536), "1.5 KiB");
        assert_eq!(human_size(5 << 20), "5.0 MiB");
        assert_eq!(human_size(3 << 30), "3.0 GiB");
    }
}
