#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use common::{FixedRule, config, item, leak_meta, location, workspace};
use skif_core::{Report, RuleMeta, Severity};
use skif_lint::{
    Emitter, Executor, LintContext, LintError, Registry, Rule, RulesProvider, RunOptions, diagnostic, run, run_unsorted,
};

const PROVIDERS: usize = 2;
const RULES: usize = 6;
const PER_RULE: usize = 5;

/// Sleeps longer for earlier rules so that, under parallel scheduling, they finish last.
struct Jittery {
    inner: FixedRule,
    delay: Duration,
}

impl Rule for Jittery {
    fn meta(&self) -> &'static RuleMeta {
        self.inner.meta()
    }

    fn check(&self, cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        std::thread::sleep(self.delay);
        self.inner.check(cx, out);
    }
}

fn message(provider: usize, rule: usize, item: usize) -> String {
    format!("m-{provider}-{rule}-{item}")
}

/// Providers of interleaving rules; each rule emits its items in descending order, so neither the merge order nor
/// the sorted order equals the other.
fn many_rules() -> Registry {
    let providers = (0..PROVIDERS)
        .map(|p| {
            let rules: Vec<Box<dyn Rule>> = (0..RULES)
                .map(|i| {
                    let meta = leak_meta(&format!("p{p}-rule-{i}"), Some(Severity::Warning));
                    let items = (0..PER_RULE)
                        .rev()
                        .map(|j| item(&message(p, i, j), None, None))
                        .collect();
                    Box::new(Jittery {
                        inner: FixedRule { meta, items },
                        delay: Duration::from_millis(u64::try_from(RULES - i).unwrap() * 2),
                    }) as Box<dyn Rule>
                })
                .collect();
            Box::new(RulesProvider::new(Box::leak(format!("p{p}").into_boxed_str()), rules))
                as Box<dyn skif_lint::RuleProvider>
        })
        .collect();
    Registry::new(providers).unwrap()
}

fn merge_order() -> Vec<String> {
    (0..PROVIDERS)
        .flat_map(|p| (0..RULES).flat_map(move |i| (0..PER_RULE).rev().map(move |j| message(p, i, j))))
        .collect()
}

fn pools() -> Vec<Executor> {
    [1, 2, 4, 8]
        .into_iter()
        .map(|n| Executor::new(n).unwrap())
        .collect()
}

#[test]
fn merge_order_is_providers_then_rules_then_emission_regardless_of_scheduling() {
    let registry = many_rules();
    let cfg = config("");
    let ws = workspace(&cfg);
    let expected = merge_order();
    let mut sorted = expected.clone();
    sorted.sort();
    assert_ne!(
        expected, sorted,
        "the expectation must differ from a sorted list to detect merge bugs"
    );

    let pools = pools();
    let messages = |options: &RunOptions<'_>| -> Vec<String> {
        run_unsorted(&registry, &ws, &cfg, options)
            .unwrap()
            .into_iter()
            .map(|d| d.message)
            .collect()
    };
    for _ in 0..10 {
        assert_eq!(messages(&RunOptions::default()), expected);
        for pool in &pools {
            let options = RunOptions::default().with_executor(pool);
            assert_eq!(messages(&options), expected);
        }
    }
}

#[test]
fn report_is_the_sorted_merge_and_identical_across_runs_and_pools() {
    let registry = many_rules();
    let cfg = config("");
    let ws = workspace(&cfg);
    let unsorted = run_unsorted(&registry, &ws, &cfg, &RunOptions::default()).unwrap();
    let expected = Report::new(unsorted.clone(), ws.all_packages().count());
    assert_ne!(
        expected.diagnostics, unsorted,
        "sorting must change the merge order for this to mean anything"
    );
    let pools = pools();
    for _ in 0..5 {
        assert_eq!(run(&registry, &ws, &cfg, &RunOptions::default()).unwrap(), expected);
        for pool in &pools {
            let options = RunOptions::default().with_executor(pool);
            assert_eq!(run(&registry, &ws, &cfg, &options).unwrap(), expected);
        }
    }
}

struct Counting {
    meta: &'static RuleMeta,
    calls: Arc<AtomicUsize>,
    set_cancel: Option<Arc<AtomicBool>>,
}

impl Rule for Counting {
    fn meta(&self) -> &'static RuleMeta {
        self.meta
    }

    fn check(&self, _cx: &LintContext<'_>, out: &mut Emitter<'_>) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if let Some(flag) = &self.set_cancel {
            flag.store(true, Ordering::SeqCst);
        }
        out.emit(diagnostic(self.meta, "hit", location()).build());
    }
}

fn counting(id: &str, calls: &Arc<AtomicUsize>, set_cancel: Option<Arc<AtomicBool>>) -> Box<dyn Rule> {
    Box::new(Counting {
        meta: leak_meta(id, Some(Severity::Warning)),
        calls: Arc::clone(calls),
        set_cancel,
    })
}

#[test]
fn preset_cancel_flag_runs_nothing() {
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = Registry::new(vec![Box::new(RulesProvider::new(
        "p",
        vec![counting("c1", &calls, None)],
    ))])
    .unwrap();
    let cfg = config("");
    let ws = workspace(&cfg);
    let cancel = AtomicBool::new(true);
    let options = RunOptions::default().with_cancel(&cancel);
    assert!(matches!(run(&registry, &ws, &cfg, &options), Err(LintError::Cancelled)));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn cancel_during_a_provider_skips_later_providers() {
    let first_calls = Arc::new(AtomicUsize::new(0));
    let second_calls = Arc::new(AtomicUsize::new(0));
    let cancel = Arc::new(AtomicBool::new(false));
    let registry = Registry::new(vec![
        Box::new(RulesProvider::new(
            "one",
            vec![counting("c1", &first_calls, Some(Arc::clone(&cancel)))],
        )),
        Box::new(RulesProvider::new("two", vec![counting("c2", &second_calls, None)])),
    ])
    .unwrap();
    let cfg = config("");
    let ws = workspace(&cfg);
    let options = RunOptions::default().with_cancel(&cancel).sequential(true);
    assert!(matches!(run(&registry, &ws, &cfg, &options), Err(LintError::Cancelled)));
    assert_eq!(first_calls.load(Ordering::SeqCst), 1);
    assert_eq!(second_calls.load(Ordering::SeqCst), 0);
}
