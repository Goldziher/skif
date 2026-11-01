#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use common::{FixedRule, config, item, leak_meta, location, workspace};
use skif_core::{RuleMeta, Severity};
use skif_lint::{Emitter, LintContext, LintError, Registry, Rule, RulesProvider, RunOptions, diagnostic, run};

const RULES: usize = 8;
const PER_RULE: usize = 6;

fn many_rules() -> Registry {
    let rules: Vec<Box<dyn Rule>> = (0..RULES)
        .map(|i| {
            let meta = leak_meta(&format!("rule-{i}"), Some(Severity::Warning));
            Box::new(FixedRule {
                meta,
                items: (0..PER_RULE)
                    .rev()
                    .map(|j| item(&format!("m-{i}-{j}"), None, None))
                    .collect(),
            }) as Box<dyn Rule>
        })
        .collect();
    Registry::new(vec![Box::new(RulesProvider::new("p", rules))]).unwrap()
}

#[test]
fn report_order_is_deterministic_across_runs_and_pools() {
    let registry = many_rules();
    let cfg = config("");
    let ws = workspace(&cfg);
    let expected: Vec<String> = (0..RULES)
        .flat_map(|i| (0..PER_RULE).map(move |j| format!("m-{i}-{j}")))
        .collect();
    let pools: Vec<rayon::ThreadPool> = [1, 4, 8]
        .into_iter()
        .map(|n| rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap())
        .collect();
    let first = run(&registry, &ws, &cfg, &RunOptions::default()).unwrap();
    let got: Vec<String> = first.diagnostics.iter().map(|d| d.message.clone()).collect();
    assert_eq!(got, expected);
    for _ in 0..20 {
        assert_eq!(run(&registry, &ws, &cfg, &RunOptions::default()).unwrap(), first);
        for pool in &pools {
            let options = RunOptions {
                pool: Some(pool),
                cancel: None,
            };
            assert_eq!(run(&registry, &ws, &cfg, &options).unwrap(), first);
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
    let options = RunOptions {
        pool: None,
        cancel: Some(&cancel),
    };
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
    let options = RunOptions {
        pool: None,
        cancel: Some(&cancel),
    };
    assert!(matches!(run(&registry, &ws, &cfg, &options), Err(LintError::Cancelled)));
    assert_eq!(first_calls.load(Ordering::SeqCst), 1);
    assert_eq!(second_calls.load(Ordering::SeqCst), 0);
}
