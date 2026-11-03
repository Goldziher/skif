//! Opt-in phase timings for diagnosing slow runs: set `SKIF_TIMINGS=1` and every instrumented phase prints its
//! duration to stderr when it ends. Nothing is measured or printed otherwise.

use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The environment variable that switches the timings on (any value other than empty or `0`).
pub const ENV_VAR: &str = "SKIF_TIMINGS";

/// Whether timings are on.
#[must_use]
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os(ENV_VAR).is_some_and(|value| !value.is_empty() && value != "0"))
}

fn print(label: &dyn Display, elapsed: Duration) {
    eprintln!("skif timing: {label}: {:.1} ms", elapsed.as_secs_f64() * 1000.0);
}

/// Runs `work` and, with timings on, prints how long it took under `label`.
///
/// The label is only formatted when timings are on, so it may be built with `format_args!`.
pub fn phase<T>(label: impl Display, work: impl FnOnce() -> T) -> T {
    if !enabled() {
        return work();
    }
    let start = Instant::now();
    let value = work();
    print(&label, start.elapsed());
    value
}

/// Time spent in a step that runs many times, possibly on several threads, summed over all calls.
#[derive(Debug, Default)]
pub struct Tally {
    nanos: AtomicU64,
}

impl Tally {
    /// A tally at zero.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            nanos: AtomicU64::new(0),
        }
    }

    /// Runs `work`, adding its duration to the tally when timings are on.
    pub fn time<T>(&self, work: impl FnOnce() -> T) -> T {
        if !enabled() {
            return work();
        }
        let start = Instant::now();
        let value = work();
        let nanos = u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.nanos.fetch_add(nanos, Ordering::Relaxed);
        value
    }

    /// Prints the accumulated time under `label` (thread time, so it can exceed the wall time) and resets it.
    pub fn report(&self, label: impl Display) {
        if enabled() {
            let nanos = self.nanos.swap(0, Ordering::Relaxed);
            print(&format_args!("{label} (summed over threads)"), Duration::from_nanos(nanos));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_runs_and_returns_its_value_with_timings_off_or_on() {
        assert_eq!(phase("test", || 7), 7);
        let tally = Tally::new();
        assert_eq!(tally.time(|| 8), 8);
        tally.report("test");
    }
}
