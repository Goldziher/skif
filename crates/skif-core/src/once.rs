//! Values computed once per run that never make a caller wait for another caller's computation.

use std::sync::OnceLock;

/// The value of `cell`, computed with `init` when none is stored yet.
///
/// Callers that arrive while no value is stored each run `init`, and the first to finish stores its value, which
/// every caller then gets; `init` must compute the same value every time. Unlike [`OnceLock::get_or_init`], no
/// caller waits for another's `init`: a rayon worker waiting for a value whose `init` uses rayon can hold a job that
/// `init` needs, or be the very thread running that `init` lower on its stack, and the run deadlocks.
pub fn get_or_compute<T>(cell: &OnceLock<T>, init: impl FnOnce() -> T) -> &T {
    if let Some(value) = cell.get() {
        return value;
    }
    let computed = init();
    cell.get_or_init(|| computed)
}

#[cfg(test)]
mod tests {
    use super::get_or_compute;
    use rayon::prelude::*;
    use std::sync::{OnceLock, mpsc};
    use std::time::Duration;

    #[test]
    fn a_job_asking_for_a_value_its_own_worker_is_computing_gets_the_stored_value() {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(1)
                .build()
                .expect("a thread pool");
            let cell = OnceLock::new();
            // The only worker runs the first job, whose `init` lets the worker run the second job, which asks for
            // the same value: `OnceLock::get_or_init` would wait for the `init` below it on the same thread forever.
            let values: Vec<usize> = pool.install(|| {
                (0..2)
                    .into_par_iter()
                    .with_max_len(1)
                    .map(|_| {
                        let value: *const u32 = get_or_compute(&cell, || {
                            rayon::yield_now();
                            7_u32
                        });
                        value as usize
                    })
                    .collect()
            });
            sender.send((values, cell.get().copied())).expect("the test is waiting");
        });
        let (addresses, stored) = receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("both jobs got the value");
        assert_eq!(stored, Some(7));
        assert_eq!(addresses[0], addresses[1]);
    }
}
