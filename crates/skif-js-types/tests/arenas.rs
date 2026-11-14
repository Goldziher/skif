//! Repeated analyses free the memory of their programs (ADR 0044). The arena counter is process-wide, so this file
//! holds the only test of its binary: no other analysis runs beside it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{generated, run};
use skif_js_types::arena_bytes_in_use;

const RUNS: usize = 50;

#[test]
fn repeated_analyses_free_their_arenas() {
    let generated = generated(4);
    let start = arena_bytes_in_use();
    for run_index in 0..RUNS {
        let facts = run(&generated.workspace, &generated.packages, &generated.query);
        assert_eq!(facts.reads, generated.reads, "reads of run {run_index}");
        assert_eq!(arena_bytes_in_use(), start, "arena bytes in use after run {run_index}");
    }
}
