// Vendored from syncpack crates/syncpack-specifier/src/eq_hash_test.rs @958d306 (MIT, Copyright (c) 2017-2023 Jamie Mason). Modified for skif; see ATTRIBUTIONS.md.
use {
    crate::Specifier,
    std::{
        collections::{HashMap, HashSet},
        sync::Arc,
    },
};

#[test]
fn specifier_eq_by_raw_string() {
    // Upstream interned in `Specifier::new` via a thread-local cache; interning is now
    // opt-in through the caller-owned `SpecifierCache`.
    let cache = crate::SpecifierCache::new();
    let a = cache.get_or_parse("^1.2.3");
    let b = cache.get_or_parse("^1.2.3");
    assert_eq!(a, b);
    assert!(Arc::ptr_eq(&a, &b));
    assert_eq!(Specifier::new("^1.2.3"), a);
}

#[test]
fn specifier_neq_when_raw_differs() {
    let a = Specifier::new("^1.2.3");
    let b = Specifier::new("^1.2.4");
    assert_ne!(a, b);
}

#[test]
#[allow(clippy::mutable_key_type)]
fn specifier_hash_consistent_with_eq() {
    let a = Specifier::new("^1.2.3");
    let b = Specifier::new("^1.2.3");
    let mut map: HashMap<Arc<Specifier>, &str> = HashMap::new();
    map.insert(a, "first");
    assert_eq!(map.get(&b), Some(&"first"));
}

#[test]
#[allow(clippy::mutable_key_type)]
fn specifier_in_hashset() {
    let mut set: HashSet<Arc<Specifier>> = HashSet::new();
    set.insert(Specifier::new("1.0.0"));
    set.insert(Specifier::new("1.0.0"));
    set.insert(Specifier::new("2.0.0"));
    assert_eq!(set.len(), 2);
}

#[test]
#[allow(clippy::mutable_key_type)]
fn specifier_none_hashes() {
    let a = Specifier::new("");
    let b = Specifier::new("");
    let mut set: HashSet<Arc<Specifier>> = HashSet::new();
    set.insert(a);
    assert!(set.contains(&b));
    assert!(matches!(*b, Specifier::None));
}
