//! Opt-in phase timings for diagnosing slow runs. The library never reads the environment and never prints: a host
//! that wants the trace installs a sink with [`enable`] (or [`enable_stderr`]), and every instrumented phase then
//! reports its duration to that sink when it ends. Nothing is measured while no sink is installed.

use std::fmt::Display;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

type Sink = Box<dyn Fn(&str) + Send + Sync>;

/// The process-wide sink; once installed it stays for the life of the process.
static SINK: OnceLock<Sink> = OnceLock::new();

/// Switches the timings on and sends one line per finished phase to `sink`.
///
/// The sink is process-wide and set once: a second call changes nothing and returns `false`. Concurrent runs in one
/// process share it, and their phases are not told apart.
#[allow(
    clippy::must_use_candidate,
    reason = "installing the sink is the point; the flag is informational"
)]
pub fn enable(sink: impl Fn(&str) + Send + Sync + 'static) -> bool {
    SINK.set(Box::new(sink)).is_ok()
}

/// A line and its arrival number, so the per-thread buffers merge back into arrival order.
type Numbered = (u64, String);

/// Every thread's buffer of the stderr sink, each behind a mutex only its thread and [`flush_stderr`] ever lock.
static BUFFERS: Mutex<Vec<Arc<Mutex<Vec<Numbered>>>>> = Mutex::new(Vec::new());

/// The arrival number of the next buffered line.
static ARRIVAL: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static BUFFER: Arc<Mutex<Vec<Numbered>>> = {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        BUFFERS.lock().unwrap_or_else(PoisonError::into_inner).push(Arc::clone(&buffer));
        buffer
    };
}

/// Switches the timings on and collects each line in memory for [`flush_stderr`] to write to stderr; see [`enable`].
///
/// Writing a line to stderr as it arrives serialises every thread on one lock and costs a system call per line, which
/// distorts the very run being measured, so the lines wait in per-thread buffers instead. The host calls
/// [`flush_stderr`] when a run ends, and a long-running host (a watcher, a server) after each run or request, which
/// also bounds the buffers; lines still buffered at process exit without it are lost.
#[allow(
    clippy::must_use_candidate,
    reason = "installing the sink is the point; the flag is informational"
)]
pub fn enable_stderr() -> bool {
    enable(|line| {
        let number = ARRIVAL.fetch_add(1, Ordering::Relaxed);
        let _ = BUFFER.try_with(|buffer| {
            buffer
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((number, line.to_owned()));
        });
    })
}

/// Writes the lines collected by the [`enable_stderr`] sink to stderr in arrival order, in one write, and empties the
/// buffers. A closed or broken stderr is ignored: a trace is not worth failing a run for. Does nothing without the sink.
pub fn flush_stderr() {
    let mut lines: Vec<Numbered> = Vec::new();
    for buffer in BUFFERS.lock().unwrap_or_else(PoisonError::into_inner).iter() {
        lines.append(&mut buffer.lock().unwrap_or_else(PoisonError::into_inner));
    }
    if lines.is_empty() {
        return;
    }
    lines.sort_unstable_by_key(|(number, _)| *number);
    let mut text = String::with_capacity(lines.iter().map(|(_, line)| line.len() + 1).sum());
    for (_, line) in &lines {
        text.push_str(line);
        text.push('\n');
    }
    let _ = std::io::stderr().lock().write_all(text.as_bytes());
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

/// With timings on, sends `message` to the sink as one line: a count or another fact about a phase that is not a
/// duration.
pub fn note(message: impl Display) {
    if let Some(sink) = SINK.get() {
        sink(&format!("skif timing: {message}"));
    }
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
        self.add(start.elapsed());
        value
    }

    /// Adds `elapsed`, measured by the caller, when timings are on: for a step that learns which tally it belongs to
    /// only once it has run.
    pub fn add(&self, elapsed: Duration) {
        if enabled() {
            let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
            self.nanos.fetch_add(nanos, Ordering::Relaxed);
        }
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
        let tally = Tally::new();
        tally.add(Duration::from_millis(3));
        tally.add(Duration::from_millis(4));
        tally.report("added by the caller");
        tally.report("after the reset");
        let lines = LINES.lock().unwrap();
        assert!(
            lines.contains(&"skif timing: added by the caller (summed over threads): 7.0 ms".to_owned()),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"skif timing: after the reset (summed over threads): 0.0 ms".to_owned()),
            "{lines:?}"
        );
        assert_eq!(lines.iter().filter(|line| line.contains("before the sink")).count(), 0);
        let during: Vec<&String> = lines.iter().filter(|line| line.contains("during the sink")).collect();
        assert_eq!(during.len(), 1);
        assert!(during[0].starts_with("skif timing: during the sink: "), "{}", during[0]);
        assert!(during[0].ends_with(" ms"), "{}", during[0]);
    }

    #[test]
    fn stderr_lines_merge_across_threads_in_arrival_order() {
        BUFFERS.lock().unwrap().clear();
        let numbers: Vec<u64> = (0..3).map(|_| ARRIVAL.fetch_add(1, Ordering::Relaxed)).collect();
        for (offset, number) in numbers.iter().enumerate().rev() {
            let number = *number;
            std::thread::spawn(move || {
                BUFFER.with(|buffer| buffer.lock().unwrap().push((number, format!("line {offset}"))));
            })
            .join()
            .unwrap();
        }
        let mut lines: Vec<Numbered> = Vec::new();
        for buffer in BUFFERS.lock().unwrap().iter() {
            lines.append(&mut buffer.lock().unwrap());
        }
        lines.sort_unstable_by_key(|(number, _)| *number);
        let texts: Vec<&str> = lines.iter().map(|(_, line)| line.as_str()).collect();
        assert_eq!(texts, ["line 0", "line 1", "line 2"]);
        flush_stderr();
    }

    #[test]
    fn work_runs_and_returns_its_value_with_timings_off_or_on() {
        assert_eq!(phase("test", || 7), 7);
        let tally = Tally::new();
        assert_eq!(tally.time(|| 8), 8);
        tally.report("test");
    }
}
