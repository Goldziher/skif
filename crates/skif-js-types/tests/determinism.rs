//! The facts do not depend on how many threads the checker uses or how the programs are scheduled (ADR 0045).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::num::NonZeroUsize;

use common::{Generated, Options, generated, generated_programs, try_run};
use skif_js_types::{TypeFacts, TypeTimings};

fn analyze_at(threads: usize) -> TypeFacts {
    without_timings(analyze_with_timings(&generated(24), threads))
}

fn analyze_with_timings(generated: &Generated, threads: usize) -> TypeFacts {
    let facts = try_run(
        &generated.workspace,
        &generated.packages,
        &generated.query,
        Options {
            threads: NonZeroUsize::new(threads),
            ..Options::default()
        },
    )
    .expect("the analysis fails");
    assert_eq!(facts.reads, generated.reads, "reads at {threads} threads");
    facts
}

fn without_timings(mut facts: TypeFacts) -> TypeFacts {
    facts.timings = TypeTimings::default();
    facts
}

/// Programs of 2, 12, 3, 1 and 4 classes: the second holds most of the judged files.
const PROGRAM_SIZES: [u32; 5] = [2, 12, 3, 1, 4];

#[test]
fn type_facts_are_identical_at_one_and_eight_threads() {
    assert_eq!(analyze_at(1), analyze_at(8));
}

#[test]
fn programs_scheduled_by_size_give_the_facts_of_a_sequential_run() {
    let generated = generated_programs(&PROGRAM_SIZES);
    let sequential = without_timings(analyze_with_timings(&generated, 1));
    let scheduled = without_timings(analyze_with_timings(&generated, 16));
    assert_eq!(sequential.covered.len(), 44, "every judged file is covered");
    assert_eq!(sequential, scheduled);
}
