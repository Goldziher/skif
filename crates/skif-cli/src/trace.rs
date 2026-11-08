//! knip's run diagnostics on stderr: `--debug` (what was loaded), `--performance` (the phase timings), `--duration`
//! (the wall time of the run) and `--memory` (the peak resident set size, Linux only).

use std::path::Path;
use std::time::Instant;

use skif::{Target, slash};
use skif_report::escape_controls;

use crate::args::LintArgs;
use crate::output::stderr_line;

/// What the diagnostic flags measure over one run.
pub struct Trace {
    start: Instant,
    duration: bool,
    memory: bool,
}

impl Trace {
    /// Starts measuring; `--performance` (or `--debug`) switches the phase timings on, to stderr.
    pub fn start(args: &LintArgs) -> Self {
        if args.performance || args.debug {
            let _ = skif_core::timing::enable_stderr();
        }
        Self {
            start: Instant::now(),
            duration: args.duration,
            memory: args.memory,
        }
    }

    /// Prints the duration and the peak memory the flags asked for.
    pub fn finish(&self) {
        if self.duration {
            stderr_line(&format!(
                "skif: finished in {:.1} ms",
                self.start.elapsed().as_secs_f64() * 1000.0
            ));
        }
        if self.memory {
            stderr_line(&peak_memory().map_or_else(
                || "skif: peak memory is only measured on Linux".to_owned(),
                |kib| format!("skif: peak memory (RSS) {}.{} MiB", kib / 1024, kib % 1024 * 10 / 1024),
            ));
        }
    }
}

/// The peak resident set size of this process in KiB, from `VmHWM` in `/proc/self/status`.
#[cfg(target_os = "linux")]
fn peak_memory() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    parse_vm_hwm(&status)
}

#[cfg(not(target_os = "linux"))]
const fn peak_memory() -> Option<u64> {
    None
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_vm_hwm(status: &str) -> Option<u64> {
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse().ok())
}

/// `--debug`: every root of the run with the configuration files it is built from and its packages.
pub fn debug_targets(base: &Path, targets: &[Target]) {
    for target in targets {
        let root = target.root.strip_prefix(base).unwrap_or(&target.root);
        let root = if root.as_os_str().is_empty() {
            ".".to_owned()
        } else {
            slash(root)
        };
        stderr_line(&format!("skif debug: root {}", escape_controls(&root)));
        if target.sources.is_empty() {
            stderr_line("skif debug:   configuration: defaults (no file)");
        }
        for source in &target.sources {
            stderr_line(&format!(
                "skif debug:   configuration: {}",
                escape_controls(&slash(&source.path))
            ));
        }
        let manager = target
            .workspace
            .package_manager
            .and_then(|manager| serde_json::to_value(manager).ok())
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_else(|| "none detected".to_owned());
        stderr_line(&format!("skif debug:   package manager: {manager}"));
        for package in target.workspace.all_packages() {
            let dir = if package.dir.as_os_str().is_empty() {
                ".".to_owned()
            } else {
                slash(&package.dir)
            };
            stderr_line(&format!(
                "skif debug:   package {} ({})",
                escape_controls(package.name.as_deref().unwrap_or("<unnamed>")),
                escape_controls(&dir)
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_vm_hwm;

    #[test]
    fn the_peak_is_read_from_vm_hwm() {
        assert_eq!(
            parse_vm_hwm("Name:\tskif\nVmHWM:\t   12345 kB\nVmRSS:\t 1 kB\n"),
            Some(12345)
        );
        assert_eq!(parse_vm_hwm("Name:\tskif\n"), None);
    }
}
