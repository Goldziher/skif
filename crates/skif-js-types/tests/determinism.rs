//! The facts do not depend on how many threads the checker uses (ADR 0044).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::num::NonZeroUsize;

use common::{Options, generated, try_run};
use skif_js_types::{TypeFacts, TypeTimings};

fn analyze_at(threads: usize) -> TypeFacts {
    let generated = generated(24);
    let mut facts = try_run(
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
    facts.timings = TypeTimings::default();
    facts
}

#[test]
fn type_facts_are_identical_at_one_and_eight_threads() {
    assert_eq!(analyze_at(1), analyze_at(8));
}
