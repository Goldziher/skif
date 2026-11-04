//! Opt-in phase timings for diagnosing slow runs. The library never reads the environment and never prints: a host
//! that wants the trace installs a sink with [`enable`] (or [`enable_stderr`]), and every instrumented phase then
//! reports its duration to that sink when it ends. Nothing is measured while no sink is installed.

use std::fmt::Display;
use std::io::Write;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

type Sink = Box<dyn Fn(&str) + Send + Sync>;

/// The process-wide sink; once installed it stays for the life of the process.
static SINK: OnceLock<Sink> = OnceLock::new();

/// Switches the timings on and sends one line per finished phase to `sink`.
///
/// The sink is process-wide and set once: a second call changes nothing and returns `false`. Concurrent runs in one
/// process share it, and their phases are not told apart.
#[allow(clippy::must_use_candidate, reason = "installing the sink is the point; the flag is informational")]
pub fn enable(sink: impl Fn(&str) + Send + Sync + 'static) -> bool {
    SINK.set(Box::new(sink)).is_ok()
}

/// Switches the timings on and writes each line to stderr; see [`enable`].
///
/// A closed or broken stderr is ignored: a trace is not worth failing a run for.
#[allow(clippy::must_use_candidate, reason = "installing the sink is the point; the flag is informational")]
pub fn enable_stderr() -> bool {
    enable(|line| {
        let _ = writeln!(std::io::stderr().lock(), "{line}");
    })
}

/// Whether a sink is installed.
#[must_use]
pub fn enabled() -> bool {
    SINK.get().is_some()
}

fn print(label: &dyn Display, elapsed: Duration) {
    if let Some(sink) = SINK.get() {
        sink(&format!(
            "skif timing: {label}: {:.1} ms",
            elapsed.as_secs_f64() * 1000.0
        ));
    }
}

/// Runs `work` and, with timings on, reports how long it took under `label`.
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
            print(
                &format_args!("{label} (summed over threads)"),
                Duration::from_nanos(nanos),
            );
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

    #[test]
    fn phases_report_to_the_installed_sink_only() {
        assert_eq!(phase("before the sink", || 1), 1);
        assert!(enable(|line| LINES.lock().unwrap().push(line.to_owned())));
        assert!(!enable(|_| {}));
        assert!(enabled());
        assert_eq!(phase("during the sink", || 2), 2);
        let lines = LINES.lock().unwrap();
        assert_eq!(lines.iter().filter(|line| line.contains("before the sink")).count(), 0);
        let during: Vec<&String> = lines.iter().filter(|line| line.contains("during the sink")).collect();
        assert_eq!(during.len(), 1);
        assert!(during[0].starts_with("skif timing: during the sink: "), "{}", during[0]);
        assert!(during[0].ends_with(" ms"), "{}", during[0]);
    }

    #[test]
    fn work_runs_and_returns_its_value_with_timings_off_or_on() {
        assert_eq!(phase("test", || 7), 7);
        let tally = Tally::new();
        assert_eq!(tally.time(|| 8), 8);
        tally.report("test");
    }
}
